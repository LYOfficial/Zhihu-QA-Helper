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
import json
import os
import re
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from html.parser import HTMLParser

from PIL import Image, ImageOps

UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
)

BAD_IMG_HINTS = (
    "logo", "icon", "sprite", "avatar", "favicon", "banner", "button",
    "arrow", "loading", "placeholder", "1x1", "pixel",
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
        self._skip = 0
        self._in_title = False
        self._scope = ""
        self._depth = 0

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
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
            src = a.get("data-original") or a.get("data-src") or a.get("src") or ""
            if src and not any(
                (a.get(dim) or "").isdigit() and int(a[dim]) < 100 for dim in ("width", "height")
            ):
                self.images.append({"url": src, "caption": (a.get("alt") or a.get("title") or "")[:160]})
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


def parse_doc(html: str, base_url: str, image_markers=None):
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
    return p.title, p.text(prefer_content=True), imgs


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
        raw, ctype, charset = fetch(url, timeout=10)
        data = json.loads(_decode(raw, charset))
    except Exception:
        return []
    pages = []
    for p in (data.get("pages") or [])[:limit]:
        key = p.get("key") or urllib.parse.quote(p.get("title", ""))
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
        raw, ctype, charset = fetch(url, timeout=10)
    except Exception:
        return None
    if ctype and not any(kind in ctype.lower() for kind in ("html", "text/plain")):
        return None
    return _make_doc(_decode(raw, charset), url, source, authority, allow_image)


def _make_doc(html: str, url: str, source: dict, authority: bool, allow_image: bool,
              article_title: str = "") -> dict | None:
    title, text, imgs = parse_doc(html, url)
    if len(text) < 180:
        return None
    return {
        "sourceName": source.get("name") or _host(url),
        "sourceUrl": url,
        "title": article_title or title,
        "text": text[:9000],
        "images": imgs,
        "trust": source.get("trust") or 3,
        # authority=False 表示这一篇来自通用网页检索兜底，不是用户配置的权威平台。
        # 这类页面只能当背景资料，绝不能作为图源——否则图库里会混进没授权过的站点。
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
    hits = web_search(f"site:{root} {query}", limit=4)
    return [url for url in hits if source_matches(source, url)][:2]


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
        raw, _, charset = fetch(f"{origin}/api/v2/help_center/articles/search.json?{params}", timeout=6)
        hits = json.loads(_decode(raw, charset)).get("results") or []
    except (OSError, ValueError):
        return []
    docs = []
    for hit in hits[:4]:
        url, body, title = hit.get("html_url"), hit.get("body"), hit.get("title")
        if not all(isinstance(value, str) for value in (url, body, title)) or not source_matches(source, url):
            continue
        doc = _make_doc(f"<article>{body}</article>", url, source, True, _allow_image(source), title)
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
                          pages[: max_pages * 2], 6, deadline))
    return [doc for doc in docs if doc][:max_pages]


BING = "https://cn.bing.com/search?q={q}&setlang=zh-CN"


def web_search(query: str, limit: int = 4, timeout: int = 10, allowed=None) -> list[str]:
    """通用网页检索兜底：配置的权威平台都抓不到内容时（比如站点被墙或 403），
    至少让拟稿有真实页面可参考，而不是完全凭空写。"""
    try:
        raw, _, charset = fetch(BING.format(q=urllib.parse.quote(query)), timeout=timeout)
    except Exception:
        return []
    html = _decode(raw, charset)
    # 只取真正的自然结果块（li.b_algo），否则会把「Google 官网」这类导航链接当成资料
    links = re.findall(
        r'<li class="b_algo"[\s\S]{0,4000}?<h2[^>]*>\s*<a[^>]+href="(https?://[^"]+)"',
        html,
    )
    if not links:
        links = re.findall(r'<h2[^>]*>\s*<a[^>]+href="(https?://[^"]+)"', html)
    skip_hosts = (
        "bing.com", "microsoft.com", "google.com", "google.cn", "googleusercontent.com",
        "about.google", "search.google", "gstatic.com",
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
        if not host or any(bad in host for bad in skip_hosts):
            continue
        if u in out or (allowed is not None and not allowed(u)):
            continue
        out.append(u)
        if len(out) >= limit:
            break
    return out


def gather_web(query: str, max_pages: int = 3, deadline: float | None = None) -> list[dict]:
    urls = web_search(query, limit=max_pages * 2)
    if not urls:
        return []
    docs: list[dict] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=min(4, len(urls))) as pool:
        futures = [pool.submit(_fetch_doc, u, {"name": _host(u), "trust": 2}) for u in urls]
        for fut in concurrent.futures.as_completed(
            futures, timeout=None if not deadline else max(5, deadline - time.time())
        ):
            try:
                d = fut.result()
            except Exception:
                continue
            if d:
                docs.append(d)
            if len(docs) >= max_pages:
                break
    return docs


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
        raw, _, _ = fetch(item["url"], referer=item["page"], timeout=5, max_bytes=6_000_000,
                          allowed=item.get("allowed"))
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
