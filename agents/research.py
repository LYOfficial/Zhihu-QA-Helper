"""联网取材：抓权威平台页面、抽正文、筛图并落地保存。

只用标准库。设计取向上偏"保守"——宁可少抓，也不把不可信内容喂给模型。
"""

from __future__ import annotations

import concurrent.futures
import base64
from html import unescape
import gzip
import hashlib
import io
import ipaddress
import json
import os
import re
import socket
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from html.parser import HTMLParser
from xml.etree import ElementTree

from PIL import Image, ImageOps

import zhihu_open_search

UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
)

BAD_IMG_HINTS = (
    "logo", "icon", "sprite", "avatar", "favicon", "banner", "button",
    "arrow", "loading", "placeholder", "1x1", "pixel", "/blank.",
)

TEXT_TAGS = {"p", "li", "td", "th", "h1", "h2", "h3", "h4", "dd", "dt", "blockquote"}
SKIP_TAGS = {"script", "style", "noscript", "svg", "nav", "footer", "form", "iframe", "header", "aside"}
CONTENT_MARKERS = {"mw-content-text", "mw-parser-output", "article-content", "article-body", "post-content", "main-content"}
FEATURE_QUALIFIERS = {"full", "complete", "support", "functionality", "gameplay", "addition", "new", "feature"}

_BAD_HOSTS: dict[str, float] = {}
_BAD_TTL = 600.0


def _host(url: str) -> str:
    try:
        return (urllib.parse.urlparse(url).hostname or "").lower()
    except ValueError:
        return ""


def public_url(url: str) -> bool:
    try:
        parts = urllib.parse.urlsplit(url)
        host = parts.hostname or ""
        if (parts.scheme not in {"http", "https"} or parts.port not in (None, 80, 443)
                or parts.username is not None or parts.password is not None or "." not in host):
            return False
        if host == "www.minecraft.net" and parts.scheme == "https" and parts.port in (None, 443):
            return True
        addresses = socket.getaddrinfo(host, None, type=socket.SOCK_STREAM)
        return bool(addresses) and all(ipaddress.ip_address(item[4][0]).is_global for item in addresses)
    except (ValueError, OSError):
        return False


def _mark_bad(url: str) -> None:
    host = _host(url)
    if host:
        _BAD_HOSTS[host] = time.time()


def _is_bad(url: str) -> bool:
    host = _host(url)
    stamp = _BAD_HOSTS.get(host)
    if stamp is None:
        return False
    if time.time() - stamp > _BAD_TTL:
        _BAD_HOSTS.pop(host, None)
        return False
    return True


def _decode(raw: bytes, charset: str | None) -> str:
    for enc in [charset, "utf-8", "gb18030", "latin-1"]:
        if not enc:
            continue
        try:
            return raw.decode(enc, "strict")
        except (UnicodeDecodeError, LookupError):
            continue
    return raw.decode("utf-8", "replace")


class _RestrictedRedirect(urllib.request.HTTPRedirectHandler):
    def __init__(self, allowed):
        self.allowed = allowed

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not self.allowed(newurl):
            raise urllib.error.URLError("不允许访问跳转后的地址")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def fetch(url: str, referer: str = "", timeout: int = 12, max_bytes: int = 3_000_000, allowed=None):
    if _is_bad(url):
        raise urllib.error.URLError("该站点刚才已失败，本次跳过")
    if allowed is not None and not allowed(url):
        raise ValueError("不允许访问此地址")
    req = urllib.request.Request(url, method="GET")
    req.add_header("User-Agent", UA)
    req.add_header("Accept", "text/html,application/xhtml+xml,image/*,*/*;q=0.8")
    req.add_header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
    req.add_header("Accept-Encoding", "identity" if allowed else "gzip, deflate")
    if referer:
        req.add_header("Referer", referer)
    try:
        opener = urllib.request.build_opener(_RestrictedRedirect(allowed)).open if allowed else urllib.request.urlopen
        with opener(req, timeout=timeout) as resp:
            raw = resp.read(max_bytes + 1)
            if len(raw) > max_bytes:
                raise ValueError("响应体过大")
            enc = resp.headers.get("Content-Encoding", "")
            if allowed and enc not in {"", "identity"}:
                raise ValueError("受限图源返回了意外的压缩格式")
            if "gzip" in enc:
                try:
                    raw = gzip.decompress(raw)
                except OSError:
                    pass
            elif "deflate" in enc:
                try:
                    raw = zlib.decompress(raw, -zlib.MAX_WBITS)
                except zlib.error:
                    pass
            ctype = resp.headers.get("Content-Type", "")
            charset = None
            m = re.search(r"charset=([\w-]+)", ctype or "", re.I)
            if m:
                charset = m.group(1)
            return raw, ctype, charset
    except urllib.error.HTTPError as e:
        if e.code == 451:
            _mark_bad(url)
        raise
    except (urllib.error.URLError, OSError):
        # DNS / 连接被重置：记住这个域名，别在同一轮里反复等超时
        _mark_bad(url)
        raise


class _DocParser(HTMLParser):
    def __init__(self, image_markers=None) -> None:
        super().__init__(convert_charrefs=True)
        self.image_markers = image_markers
        self.chunks: list[str] = []
        self.content: list[str] = []
        self.images: list[dict] = []
        self.title = ""
        self.published = ""
        self.preview = ""
        self.description = ""
        self._skip = 0
        self._in_title = False
        self._scope = ""
        self._depth = 0

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "meta":
            name, content = a.get("property") or a.get("name"), a.get("content") or ""
            if name in {"article:published_time", "datePublished", "pubdate"} and not self.published:
                self.published = content
            if name in {"og:image", "twitter:image"} and not self.preview:
                self.preview = content
            if name in {"og:description", "description"} and not self.description:
                self.description = content.strip()
        if tag in SKIP_TAGS:
            self._skip += 1
        if self._skip:
            return
        if tag == "title":
            self._in_title = True
        markers = set((a.get("class") or "").split()) | {a.get("id")}
        if self._scope == tag:
            self._depth += 1
        elif not self._scope and (
            bool(self.image_markers & markers) if self.image_markers is not None else
            (tag in {"article", "main", "figure"} or CONTENT_MARKERS & markers)
        ):
            self._scope, self._depth = tag, 1
        if tag == "img" and self._scope:
            variants = [item.split() for item in (a.get("data-srcset") or a.get("srcset") or "").split(",") if item.strip()]
            def width(variant):
                size = re.fullmatch(r"(\d+(?:\.\d+)?)([wx])", variant[1]) if len(variant) > 1 else None
                return float(size[1]) * (1000 if size[2] == "x" else 1) if size else 0

            responsive = max(variants, key=width)[0] if variants and not variants[0][0].startswith("data:") else ""
            urls = dict.fromkeys((a.get("data-original"), a.get("data-actualsrc"), a.get("data-src"), responsive, a.get("src")))
            if not any(
                (a.get(dim) or "").isdigit() and int(a[dim]) < 100 for dim in ("width", "height")
            ):
                caption = (a.get("alt") or a.get("title") or "")[:160]
                self.images.extend({"url": src, "caption": caption} for src in urls if src and not src.startswith("data:"))
        if tag in TEXT_TAGS or tag == "br":
            self.chunks.append("\n")
            if self._scope:
                self.content.append("\n")

    def handle_endtag(self, tag):
        if self._skip:
            if tag in SKIP_TAGS:
                self._skip -= 1
            return
        if tag == self._scope:
            self._depth -= 1
            if not self._depth:
                self._scope = ""
        if tag in TEXT_TAGS:
            self.chunks.append("\n")
            if self._scope:
                self.content.append("\n")
        if tag == "title":
            self._in_title = False

    def handle_data(self, data):
        if self._in_title:
            self.title += data.strip()
            return
        if self._skip:
            return
        t = data.strip()
        if t:
            self.chunks.append(t)
            if self._scope:
                self.content.append(t)

    def text(self, prefer_content: bool = False) -> str:
        content = " ".join(self.content).strip()
        joined = content if prefer_content and len(content) >= 180 else " ".join(self.chunks)
        joined = re.sub(r"[ \t]+", " ", joined)
        joined = re.sub(r"\n\s*\n+", "\n", joined)
        return joined.strip()


def _parsed_doc(html: str, base_url: str, image_markers=None):
    include_preview = image_markers is None
    if include_preview and _host(base_url) in {"www.minecraft.net", "minecraft.net"}:
        image_markers = CONTENT_MARKERS | {"article-grid-a", "article-head", "article-head__image", "article-media", "MC_Carousel"}
    p = _DocParser(image_markers)
    try:
        p.feed(html)
    except Exception:  # 坏 HTML 不该让整条流水线崩掉
        pass
    imgs = []
    for image in p.images:
        try:
            full = urllib.parse.urljoin(base_url, image["url"])
        except ValueError:
            continue
        if full.startswith(("https://", "http://")):
            imgs.append({**image, "url": full})
    if p.preview and include_preview:
        try:
            preview = urllib.parse.urljoin(base_url, p.preview)
            if preview.startswith(("https://", "http://")):
                imgs.append({"url": preview, "caption": p.title[:160], "cover": True})
        except ValueError:
            pass
    dates = re.search(r'"datePublished"\s*:\s*"(\d{4}-\d{2}-\d{2}[^"<]{0,30})"', html, re.I)
    text = p.text(prefer_content=True)
    if _host_under(_host(base_url), ("bilibili.com",)) and urllib.parse.urlsplit(base_url).path.startswith("/video/"):
        text = f"【视频页面简介，不是视频转录】\n{p.description}" if len(p.description) >= 40 else ""
    return p.title, text, imgs, p.published or (dates[1] if dates else "")


def parse_doc(html: str, base_url: str, image_markers=None):
    return _parsed_doc(html, base_url, image_markers)[:3]


def clean_sample(text: str) -> str:
    if re.search(r"</?[a-z][^>]*>", text, re.I):
        parser = _DocParser()
        parser.feed(text[:32000])
        text = parser.text()
    return "\n\n".join(part.strip() for part in text.splitlines() if part.strip())


def mediawiki_search(base: str, query: str, limit: int = 3) -> list[dict]:
    """MediaWiki 的 REST 搜索接口，各类维基站点都开放了这个端点。"""
    base = base.rstrip("/")
    url = f"{base}/rest.php/v1/search/page?q={urllib.parse.quote(query)}&limit={limit}"
    try:
        raw, ctype, charset = fetch(url, timeout=5, allowed=lambda link: source_matches({"baseUrl": base}, link) and public_url(link))
        data = json.loads(_decode(raw, charset))
    except (OSError, ValueError):
        params = urllib.parse.urlencode({"action": "query", "generator": "search", "gsrsearch": query,
                                       "gsrlimit": limit, "format": "json", "formatversion": 2})
        try:
            raw, _, charset = fetch(f"{base}/api.php?{params}", timeout=4, allowed=lambda link: public_url(link) and _host(link) == _host(base))
            data = json.loads(_decode(raw, charset)).get("query") or {}
        except (OSError, ValueError):
            return []
    pages = []
    for p in (data.get("pages") or [])[:limit]:
        key = urllib.parse.quote(p.get("key") or p.get("title", "").replace(" ", "_"), safe="%()/,_-")
        if not key:
            continue
        pages.append(
            {
                "title": p.get("title") or key,
                "url": f"{base}/wiki/{key}",
                "excerpt": re.sub(r"<[^>]+>", "", p.get("excerpt") or "")[:400],
                "thumb": (p.get("thumbnail") or {}).get("url", ""),
            }
        )
    return pages


def _allow_image(source: dict) -> bool:
    use_for = source.get("useFor")
    return isinstance(use_for, list) and "image" in use_for


def source_matches(source: dict, url: str) -> bool:
    base = source.get("baseUrl") or ""
    host = _host(base).removeprefix("www.")
    target = _host(url)
    if not host or not (target == host or target.endswith("." + host)):
        return False
    path = urllib.parse.urlsplit(base).path.rstrip("/")
    target_path = urllib.parse.urlsplit(url).path
    return not path or target_path == path or target_path.startswith(path + "/")


def _fetch_doc(url: str, source: dict, authority: bool = False, allow_image: bool = False) -> dict | None:
    try:
        raw, ctype, charset = fetch(url, timeout=7,
                                   allowed=lambda link: public_url(link) and (not authority or source_matches(source, link)))
    except Exception:
        return None
    if ctype and not any(kind in ctype.lower() for kind in ("html", "text/plain")):
        return None
    return _make_doc(_decode(raw, charset), url, source, authority, allow_image)


def _make_doc(html: str, url: str, source: dict, authority: bool, allow_image: bool,
              article_title: str = "", published_at: str = "") -> dict | None:
    title, text, imgs, published = _parsed_doc(html, url)
    if len(text) < (50 if source.get("kind") == "community" else 180):
        return None
    return {
        "sourceName": source.get("name") or _host(url),
        "sourceUrl": url,
        "title": article_title or title,
        "text": text[:28000],
        "images": imgs,
        "trust": source.get("trust") or 3,
        "sourceKind": source.get("kind") or "web",
        "publishedAt": published[:40] or published_at[:40],
        "authority": authority,
        "allowImage": allow_image,
    }


def _parallel(function, inputs, workers: int, deadline: float):
    inputs = list(inputs)
    if not inputs:
        return []
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=min(workers, len(inputs)))
    try:
        futures = [pool.submit(function, item) for item in inputs]
        done, _ = concurrent.futures.wait(futures, timeout=max(0, deadline - time.time()))
        return [future.result() for future in futures if future in done and future.exception() is None]
    finally:
        pool.shutdown(wait=False, cancel_futures=True)


def _source_pages(source: dict, query: str) -> list[str]:
    base = (source.get("baseUrl") or "").rstrip("/")
    host = _host(base)
    if not host or _is_bad(base):
        return []
    if host == "www.minecraft.net":
        pages = _minecraft_news_pages()
        if pages:
            target = keywords(query)
            return sorted(pages, key=lambda page: len(keywords(page) & target), reverse=True)[:6]
    if source.get("kind") == "wiki" or "wiki" in host:
        hits = mediawiki_search(base, query, limit=2)
        if hits:
            return [hit["url"] for hit in hits]
    if host in {"modrinth.com", "www.modrinth.com"}:
        url = f"https://api.modrinth.com/v2/search?query={urllib.parse.quote(query)}&limit=3"
        try:
            raw, _, charset = fetch(url, timeout=6)
            hits = json.loads(_decode(raw, charset)).get("hits") or []
            return [f"https://modrinth.com/{hit['project_type']}/{hit['slug']}" for hit in hits[:2]
                    if hit.get("project_type") in {"mod", "plugin", "resourcepack", "shader", "modpack", "datapack"}
                    and re.fullmatch(r"[a-zA-Z0-9_-]+", str(hit.get("slug", "")))]
        except (OSError, ValueError):
            return []
    root = host.removeprefix("www.")
    return web_search(f"site:{root} {query}", limit=2, timeout=6,
                      allowed=lambda url: source_matches(source, url))


_MINECRAFT_NEWS: tuple[float, list[str]] = (0, [])


def _minecraft_news_pages() -> list[str]:
    global _MINECRAFT_NEWS
    stamp, pages = _MINECRAFT_NEWS
    if time.monotonic() - stamp < 240 and pages:
        return pages
    origin = "https://www.minecraft.net"
    try:
        raw, _, charset = fetch(origin + "/en-us/articles", timeout=5, allowed=lambda url: _host(url) == "www.minecraft.net" and public_url(url))
    except (OSError, ValueError):
        return []
    paths = re.findall(r'href="(/en-us/article/[a-z0-9-]+)"', _decode(raw, charset), flags=re.I)
    pages = [origin + path for path in dict.fromkeys(paths)][:12]
    _MINECRAFT_NEWS = (time.monotonic(), pages)
    return pages


def focused_phrase(query: str) -> str:
    feature = re.search(r'"([^"\r\n]{2,80})"', query)
    key = " ".join(word for word in feature.group(1).split()
                   if word.lower() not in FEATURE_QUALIFIERS) if feature else ""
    return re.sub(r"\boff ?hand\b", "off-hand", key, flags=re.I)


def _help_center_docs(source: dict, query: str) -> list[dict]:
    base = source.get("baseUrl") or ""
    parts = urllib.parse.urlsplit(base)
    if _host(base) != "feedback.minecraft.net" and not parts.path.startswith("/hc/"):
        return []
    origin = parts._replace(path="", query="", fragment="").geturl()
    key = focused_phrase(query)
    search = f'"{key}"' if key else query
    params = urllib.parse.urlencode({"query": search, "per_page": 4})
    try:
        raw, _, charset = fetch(f"{origin}/api/v2/help_center/articles/search.json?{params}", timeout=6,
                                allowed=lambda link: public_url(link) and _host(link) == _host(origin))
        hits = json.loads(_decode(raw, charset)).get("results") or []
    except (OSError, ValueError):
        return []
    docs = []
    for hit in hits[:4]:
        url, body, title = hit.get("html_url"), hit.get("body"), hit.get("title")
        if not all(isinstance(value, str) for value in (url, body, title)) or not source_matches(source, url):
            continue
        doc = _make_doc(f"<article>{body}</article>", url, source, True, _allow_image(source), title,
                        str(hit.get("created_at") or ""))
        if doc:
            docs.append(doc)
    return docs[:2]


def gather(query: str, sources: list[dict], max_pages: int = 4, deadline: float | None = None) -> list[dict]:
    deadline = deadline or time.time() + 18
    ordered = sorted([source for source in sources if source.get("enabled", True)],
                     key=lambda source: -int(source.get("trust") or 3))[:6]
    def lookup(source):
        docs = _help_center_docs(source, query)
        return source, docs, [] if docs else _source_pages(source, query)

    results = _parallel(lookup, ordered, 6, min(deadline, time.time() + 9))
    pages, docs, seen = [], [], set()
    for source, articles, links in results:
        docs.extend(articles)
        for link in links:
            key = canonical_url(link)
            if key not in seen:
                pages.append((link, source))
                seen.add(key)
    docs.extend(_parallel(lambda page: _fetch_doc(page[0], page[1], True, _allow_image(page[1])),
                          pages[: max_pages * 2], 8, deadline))
    target = keywords(query)
    return sorted((doc for doc in docs if doc), key=lambda doc: (
        4 * len(target & keywords(doc["title"])) + len(target & keywords(doc["text"])),
        doc.get("publishedAt", ""), doc.get("trust", 0),
    ), reverse=True)[:max_pages]


BING = "https://cn.bing.com/search?{params}"


def _search_links(provider, query, timeout, recent):
    params = {"q": query, "setlang": "zh-CN" if re.search(r"[\u4e00-\u9fff]", query) else "en-US"}
    if recent:
        params["filters"] = 'ex1:"ez3"'
    if provider == "rss":
        params["format"] = "rss"
    if provider == "so":
        endpoint = "https://www.so.com/s?" + urllib.parse.urlencode({"q": query})
        domain = "so.com"
    elif provider == "duckduckgo":
        endpoint = "https://html.duckduckgo.com/html/?" + urllib.parse.urlencode({"q": query, **({"df": "m"} if recent else {})})
        domain = "duckduckgo.com"
    else:
        endpoint = BING.format(params=urllib.parse.urlencode(params))
        domain = "bing.com"
    raw, _, charset = fetch(endpoint, timeout=timeout, max_bytes=1_500_000,
                            allowed=lambda url: _host(url).endswith("." + domain))
    html = _decode(raw, charset)
    if provider == "rss":
        return [item.text or "" for item in ElementTree.fromstring(html).findall("./channel/item/link")]
    if provider == "so":
        return re.findall(r'\bdata-mdurl="(https?://[^"\s]+)"', html)
    if provider == "duckduckgo":
        links = re.findall(r'<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"', html)
        return [urllib.parse.parse_qs(urllib.parse.urlsplit(unescape(link)).query).get("uddg", [link])[0] for link in links]
    return re.findall(r'<li\b[^>]*class="[^"]*\bb_algo\b[^"]*"[\s\S]{0,4500}?<h2[^>]*>\s*<a[^>]+href="([^"]+)"', html)


def web_search(query: str, limit: int = 4, timeout: int = 7, allowed=None, *, recent=False) -> list[str]:
    methods = ("so", "html", "rss", "duckduckgo")
    groups = _parallel(lambda method: _search_links(method, query, timeout, recent), methods, 4, time.time() + timeout)
    links = [group[index] for index in range(max(map(len, groups), default=0)) for group in groups if index < len(group)]
    skip_hosts = (
        "bing.com", "microsoft.com", "google.com", "google.cn", "googleusercontent.com",
        "about.google", "search.google", "gstatic.com",
        "360.com", "360kan.com", "so.com", "duckduckgo.com",
    )
    out: list[str] = []
    for u in links:
        u = unescape(u)
        if _host(u) in {"cn.bing.com", "www.bing.com"}:
            redirect = urllib.parse.parse_qs(urllib.parse.urlsplit(u).query).get("u", [""])[0]
            if redirect.startswith("a1"):
                try:
                    encoded = redirect[2:]
                    u = base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4)).decode()
                except (ValueError, UnicodeError):
                    continue
        host = _host(u)
        if not u.startswith(("https://", "http://")) or not host or any(host == bad or host.endswith("." + bad) for bad in skip_hosts):
            continue
        if u in out or (allowed is not None and not allowed(u)):
            continue
        out.append(u)
        if len(out) >= limit:
            break
    return out


COMMUNITY_SITES = {"bilibili.com": "哔哩哔哩", "xiaoheihe.cn": "小黑盒", "zhihu.com": "知乎",
                   "tieba.baidu.com": "百度贴吧", "weibo.com": "微博", "reddit.com": "Reddit", "x.com": "X"}
NEWS_SITES = {"xinhuanet.com": "新华网", "news.cn": "新华网", "people.com.cn": "人民网", "cctv.com": "央视网",
              "thepaper.cn": "澎湃新闻", "caixin.com": "财新", "cyol.com": "中青在线", "reuters.com": "Reuters",
              "apnews.com": "AP", "bbc.com": "BBC", "bbc.co.uk": "BBC"}


def _host_under(host, sites):
    return any(host == site or host.endswith("." + site) for site in sites)


def gather_web(query: str, max_pages: int = 3, deadline: float | None = None, sources=(), *, recent=False,
               lane="web") -> list[dict]:
    deadline = deadline or time.time() + 14
    if lane in {"discussion", "news"}:
        chinese = lane == "discussion" or bool(re.search(r"[\u4e00-\u9fff]", query))
        sites = tuple(COMMUNITY_SITES)[:5] if lane == "discussion" else (
            ("news.cn", "people.com.cn", "cctv.com", "thepaper.cn", "caixin.com") if chinese else ("reuters.com", "apnews.com", "bbc.com"))
        groups = _parallel(lambda site: web_search(f"{query} site:{site}", limit=2, timeout=5, recent=recent,
                                                     allowed=lambda url: _host_under(_host(url), (site,))),
                           sites, 5, min(deadline, time.time() + 5.5))
        urls = [url for group in groups for url in group]
    else:
        urls = web_search(query, limit=max_pages * 2, timeout=6, recent=recent)
    return _read_web_pages(urls, query, sources, max_pages, deadline)


def gather_zhihu_open(kind: str, query: str, deadline: float, sources=(), *, recent=False) -> list[dict]:
    remaining = deadline - time.time()
    if remaining <= 0:
        return []
    try:
        urls = zhihu_open_search.search_urls(kind, query, timeout=min(5, remaining), recent=recent)
    except zhihu_open_search.SearchError as error:
        print(f"知乎开放平台 {kind}：{error}，继续使用原有来源。", file=sys.stderr)
        return []
    if kind == "zhihu_search":
        urls = [url for url in urls if _host_under(_host(url), ("zhihu.com",))]
    return _read_web_pages(urls, query, sources, 3, deadline)


def _read_web_pages(urls, query, sources, max_pages, deadline):
    def read(url):
        host = _host(url)
        if not host or not url.startswith(("http://", "https://")):
            return None
        community = next((label for site, label in COMMUNITY_SITES.items() if _host_under(host, (site,))), "")
        media = next((label for site, label in NEWS_SITES.items() if _host_under(host, (site,))), "")
        source = next((source for source in sources if source_matches(source, url)), None)
        if source and not (community or media):
            return _fetch_doc(url, source, True, _allow_image(source))
        kind = "community" if community else "web"
        if _host_under(host, ("gov.cn", "gov.uk", "europa.eu", "who.int", "un.org", "esa.int", "cas.cn", "cnsa.gov.cn")) or host.endswith(".gov"):
            kind = "official"
        elif media:
            kind = "media"
        elif _host_under(host, ("wikipedia.org", "minecraft.wiki")):
            kind = "wiki"
        parts = urllib.parse.urlsplit(url)
        origin = parts._replace(path="", query="", fragment="").geturl()
        return _fetch_doc(url, {"name": community or media or host, "trust": {"official": 4, "media": 3}.get(kind, 2), "kind": kind, "baseUrl": origin},
                          authority=kind in {"official", "media", "wiki"})

    target = keywords(query)
    docs = [doc for doc in _parallel(read, urls, 8, deadline) if doc]
    return sorted(docs, key=lambda doc: (4 * len(target & keywords(doc["title"])) + len(target & keywords(doc["text"])),
                                        doc.get("trust", 0), doc.get("publishedAt", "")), reverse=True)[:max_pages]


def keywords(text: str) -> set[str]:
    lower = text.lower()
    words = set(re.findall(r"[a-z][a-z0-9]{1,}", lower))
    words.update(left + right for left, right in re.findall(r"\b([a-z]+)-([a-z]+)\b", lower))
    words.update(re.findall(r"(?=([\u4e00-\u9fff]{2}))", lower))
    return words - {"how", "what", "the", "for", "and", "什么", "怎么", "如何", "问题", "知乎", "官方", "可以", "哪个"}


def canonical_url(url: str) -> str:
    parts = urllib.parse.urlsplit(url)
    return urllib.parse.urlunsplit((parts.scheme.lower(), parts.netloc.lower(), parts.path, "", ""))


def pick_image_urls(docs: list[dict], want: int, topic: str) -> list[dict]:
    target = keywords(topic) - {"beta", "preview", "edition", "update", "version", "gameplay", "support", "full", "完整", "功能", "玩法", "版本"}
    ranked = []
    for doc in docs:
        if not (doc.get("authority") and doc.get("allowImage")):
            continue
        page = doc["sourceUrl"]
        specific = target - keywords(doc.get("sourceName", ""))
        page_matches = specific & keywords(doc.get("title", ""))
        for image in doc.get("images", [])[:40]:
            if not isinstance(image, dict):
                continue
            url = image["url"]
            path = urllib.parse.unquote(urllib.parse.urlsplit(url).path).lower()
            caption = image.get("caption", "")
            if any(hint in path for hint in BAD_IMG_HINTS) or path.endswith((".svg", ".svgz", ".gif")):
                continue
            filename = path.rsplit("/", 1)[-1]
            image_matches = specific & keywords(caption + " " + filename)
            if not image_matches and len(page_matches) < 2:
                continue
            if not image_matches and not caption and not keywords(filename):
                continue
            ranked.append((
                4 * len(image_matches) + len(page_matches), int(doc.get("trust") or 3),
                {"url": url, "caption": caption or doc.get("title", ""), "page": page,
                 "sourceName": doc.get("sourceName", "")},
            ))
    ranked.sort(key=lambda item: item[:2], reverse=True)
    out, seen, per_document = [], set(), {}
    for _, _, item in ranked:
        key = canonical_url(item["url"])
        page = item["page"]
        if key in seen or per_document.get(page, 0) >= max(4, want):
            continue
        seen.add(key)
        per_document[page] = per_document.get(page, 0) + 1
        out.append(item)
        if len(out) >= min(30, want * 4):
            break
    return out


def _normal_image(raw: bytes) -> tuple[bytes, int, int, float]:
    with Image.open(io.BytesIO(raw), formats=("PNG", "JPEG", "WEBP")) as original:
        width, height = original.size
        if (min(width, height) < 160 or width * height > 24_000_000
                or max(width, height) / min(width, height) > 5 or getattr(original, "is_animated", False)):
            raise ValueError("不是合适的正文配图")
        image = ImageOps.exif_transpose(original).convert("RGBA")
    image.thumbnail((1800, 1800))
    white = Image.new("RGBA", image.size, "white")
    pixels = Image.alpha_composite(white, image).convert("RGB")
    fingerprint, brightness, ratio = _image_signature(pixels)
    target = io.BytesIO()
    pixels.save(target, format="PNG", optimize=False)
    return target.getvalue(), fingerprint, brightness, ratio


def _image_signature(pixels):
    gray = pixels.convert("L").resize((9, 9), Image.Resampling.LANCZOS)
    values = gray.get_flattened_data()
    if max(values) - min(values) < 28:
        raise ValueError("空白或占位图")
    fingerprint = 0
    for index, value in enumerate(values):
        if index % 9 < 8:
            fingerprint = (fingerprint << 1) | (value > values[index + 1])
        if index < 72:
            fingerprint = (fingerprint << 1) | (value > values[index + 9])
    return fingerprint, sum(values) // len(values), pixels.width / pixels.height


def _save_image(item, image, out_dir, seen, fingerprints):
    content, fingerprint, brightness, ratio = image
    digest = hashlib.sha256(content).hexdigest()
    filename = digest[:32] + ".png"
    if filename in seen or any(
        abs(ratio / previous_ratio - 1) < 0.12 and abs(brightness - previous_brightness) < 22
        and (fingerprint ^ previous).bit_count() <= 10
        for previous, previous_brightness, previous_ratio in fingerprints
    ):
        return None
    path = os.path.join(out_dir, filename)
    if not os.path.exists(path):
        with tempfile.NamedTemporaryFile(dir=out_dir, delete=False) as temporary:
            temporary.write(content)
        try:
            os.replace(temporary.name, path)
        finally:
            if os.path.exists(temporary.name):
                os.unlink(temporary.name)
    seen.add(filename)
    fingerprints.append((fingerprint, brightness, ratio))
    return {
        "filename": filename, "sourceUrl": item["page"], "imageUrl": item.get("url", ""),
        "sourceName": item["sourceName"], "caption": item["caption"], "bytes": len(content),
        "autoInsert": item.get("autoInsert", False),
    }


def existing_signatures(out_dir, existing):
    signatures = []
    for filename in existing[:80]:
        if not re.fullmatch(r"[a-f0-9]{32}\.png", filename):
            continue
        try:
            with Image.open(os.path.join(out_dir, filename), formats=("PNG",)) as image:
                if image.width * image.height <= 4_000_000:
                    signatures.append(_image_signature(image))
        except (OSError, ValueError, Image.DecompressionBombError):
            pass
    return signatures


def download_images(candidates: list[dict], out_dir: str, want: int = 6,
                    deadline: float | None = None, existing=None) -> list[dict]:
    if not candidates or want <= 0:
        return []
    os.makedirs(out_dir, exist_ok=True)

    def download(item: dict):
        allowed = item.get("allowed")
        raw, _, _ = fetch(item["url"], referer=item["page"], timeout=5, max_bytes=6_000_000,
                          allowed=lambda link: public_url(link) and (allowed is None or allowed(link)))
        return _normal_image(raw)

    existing = existing or []
    saved, seen, fingerprints = [], set(existing), existing_signatures(out_dir, existing)
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=4)
    futures = [pool.submit(download, item) for item in candidates]
    try:
        for item, future in zip(candidates, futures):
            try:
                image = future.result(
                    timeout=None if deadline is None else max(0, deadline - time.time())
                )
            except (ValueError, OSError, Image.DecompressionBombError, concurrent.futures.TimeoutError):
                continue
            asset = _save_image(item, image, out_dir, seen, fingerprints)
            if asset is None:
                continue
            saved.append(asset)
            if len(saved) >= want:
                break
    finally:
        pool.shutdown(wait=False, cancel_futures=True)
    return saved
