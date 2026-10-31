"""联网取材：抓权威平台页面、抽正文、筛图并落地保存。

只用标准库。设计取向上偏"保守"——宁可少抓，也不把不可信内容喂给模型。
"""

from __future__ import annotations

import concurrent.futures
import gzip
import hashlib
import io
import json
import os
import re
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from html.parser import HTMLParser

UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
)

BAD_IMG_HINTS = (
    "logo", "icon", "sprite", "avatar", "favicon", "banner", "button",
    "arrow", "loading", "placeholder", "1x1", "pixel",
)

TEXT_TAGS = {"p", "li", "td", "th", "h1", "h2", "h3", "h4", "dd", "dt", "blockquote"}
SKIP_TAGS = {"script", "style", "noscript", "svg", "nav", "footer", "form", "iframe"}

# 被站点拒绝（403/404/DNS 失败）后，短时间内不再重试同一域名。
# 否则三个检索词会各撞一次墙，白等好几轮超时。
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


def fetch(url: str, referer: str = "", timeout: int = 12, max_bytes: int = 3_000_000):
    if _is_bad(url):
        raise urllib.error.URLError("该站点刚才已失败，本次跳过")
    req = urllib.request.Request(url, method="GET")
    req.add_header("User-Agent", UA)
    req.add_header("Accept", "text/html,application/xhtml+xml,image/*,*/*;q=0.8")
    req.add_header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
    req.add_header("Accept-Encoding", "gzip, deflate")
    if referer:
        req.add_header("Referer", referer)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read(max_bytes)
            enc = resp.headers.get("Content-Encoding", "")
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
        if e.code in (401, 403, 404, 410, 451):
            _mark_bad(url)
        raise
    except (urllib.error.URLError, OSError):
        # DNS / 连接被重置：记住这个域名，别在同一轮里反复等超时
        _mark_bad(url)
        raise


class _DocParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.chunks: list[str] = []
        self.images: list[str] = []
        self.title = ""
        self._skip = 0
        self._in_title = False

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag in SKIP_TAGS:
            self._skip += 1
        if tag == "title":
            self._in_title = True
        if tag == "img":
            src = a.get("src") or a.get("data-src") or ""
            if src:
                self.images.append(src)
        if tag in TEXT_TAGS:
            self.chunks.append("\n")

    def handle_endtag(self, tag):
        if tag in SKIP_TAGS and self._skip > 0:
            self._skip -= 1
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

    def text(self) -> str:
        joined = " ".join(self.chunks)
        joined = re.sub(r"[ \t]+", " ", joined)
        joined = re.sub(r"\n\s*\n+", "\n", joined)
        return joined.strip()


def parse_doc(html: str, base_url: str):
    p = _DocParser()
    try:
        p.feed(html)
    except Exception:  # 坏 HTML 不该让整条流水线崩掉
        pass
    imgs = []
    for src in p.images:
        if src.startswith("data:"):
            continue
        full = urllib.parse.urljoin(base_url, src)
        if full.startswith("http"):
            imgs.append(full)
    return p.title, p.text(), imgs


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


def _guess_search_url(source: dict, query: str) -> str | None:
    base = (source.get("baseUrl") or "").rstrip("/")
    kind = source.get("kind") or ""
    if not base:
        return None
    q = urllib.parse.quote(query)
    if "mcmod.cn" in base:
        return f"https://search.mcmod.cn/s?key={q}"
    if kind == "wiki" or "wiki" in base:
        return None  # 走 mediawiki_search
    if "modrinth.com" in base:
        return f"https://api.modrinth.com/v2/search?query={q}&limit=3"
    if "curseforge.com" in base:
        return None
    return None


def _fetch_doc(url: str, source: dict) -> dict | None:
    try:
        raw, ctype, charset = fetch(url, timeout=10)
    except Exception:
        return None
    title, text, imgs = parse_doc(_decode(raw, charset), url)
    if len(text) < 200:
        return None
    return {
        "sourceName": source.get("name") or _host(url),
        "sourceUrl": url,
        "title": title,
        "text": text[:9000],
        "images": imgs,
        "trust": source.get("trust") or 3,
    }


def gather(query: str, sources: list[dict], max_pages: int = 4, deadline: float | None = None) -> list[dict]:
    """按可信度从高到低检索，凑够 max_pages 篇正文就停。
    所有抓取并行，且受 deadline 约束——慢站点不会把整次拟稿拖死。"""
    docs: list[dict] = []
    ordered = sorted(
        [s for s in sources if s.get("enabled", True)],
        key=lambda s: -int(s.get("trust") or 3),
    )
    urls: list[tuple[str, dict]] = []
    json_hits: list[tuple[str, str, dict]] = []

    for s in ordered:
        if len(urls) + len(json_hits) >= max_pages * 2:
            break
        if deadline and time.time() > deadline:
            break
        base = (s.get("baseUrl") or "").rstrip("/")
        if not base or _is_bad(base):
            continue

        if s.get("kind") == "wiki" or "wiki" in base:
            for hit in mediawiki_search(base, query, limit=2):
                urls.append((hit["url"], s))
            continue

        surl = _guess_search_url(s, query)
        if not surl:
            continue
        try:
            raw, ctype, charset = fetch(surl, timeout=10)
        except Exception:
            continue
        if "json" in (ctype or ""):
            try:
                data = json.loads(_decode(raw, charset))
                for h in (data.get("hits") or [])[:2]:
                    json_hits.append((f"https://modrinth.com/mod/{h.get('slug')}", h.get("slug", ""), s))
            except Exception:
                pass
            continue
        title, text, imgs = parse_doc(_decode(raw, charset), surl)
        if len(text) > 400:
            docs.append(
                {
                    "sourceName": s.get("name") or base,
                    "sourceUrl": surl,
                    "title": title,
                    "text": text[:5000],
                    "images": imgs,
                    "trust": (s.get("trust") or 3) - 1,
                }
            )
        if len(docs) >= max_pages:
            return docs

    urls.extend((u, s) for u, _slug, s in json_hits)
    if not urls:
        return docs

    limit = max(1, max_pages - len(docs))
    with concurrent.futures.ThreadPoolExecutor(max_workers=min(6, len(urls))) as pool:
        futures = [pool.submit(_fetch_doc, u, s) for u, s in urls[: limit * 2]]
        for fut in concurrent.futures.as_completed(futures, timeout=25 if not deadline else max(5, deadline - time.time())):
            try:
                d = fut.result()
            except Exception:
                continue
            if d:
                docs.append(d)
            if len(docs) >= max_pages:
                break
    return docs


BING = "https://cn.bing.com/search?q={q}&setlang=zh-CN"


def web_search(query: str, limit: int = 4) -> list[str]:
    """通用网页检索兜底：配置的权威平台都抓不到内容时（比如站点被墙或 403），
    至少让拟稿有真实页面可参考，而不是完全凭空写。"""
    try:
        raw, ctype, charset = fetch(BING.format(q=urllib.parse.quote(query)), timeout=10)
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
        host = _host(u)
        if not host or any(bad in host for bad in skip_hosts):
            continue
        if u in out:
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


_MAGIC = {
    b"\x89PNG\r\n\x1a\n": ".png",
    b"\xff\xd8\xff": ".jpg",
    b"GIF8": ".gif",
    b"RIFF": ".webp",
}


def _sniff_ext(raw: bytes) -> str | None:
    for sig, ext in _MAGIC.items():
        if raw.startswith(sig):
            if ext == ".webp" and b"WEBP" not in raw[:16]:
                return None
            return ext
    head = raw[:400].lstrip()
    if head.startswith(b"<svg") or head.startswith(b"<?xml"):
        return ".svg"
    return None


def pick_image_urls(docs: list[dict], want: int) -> list[tuple[str, str, str]]:
    """返回 [(url, sourceName, 所在页面)]，按可信度优先，去重。"""
    out: list[tuple[str, str, str]] = []
    seen: set[str] = set()
    for d in docs:
        for u in d.get("images", []):
            low = u.lower()
            if any(b in low for b in BAD_IMG_HINTS):
                continue
            if any(low.split("?")[0].endswith(e) for e in (".svg", ".png", ".jpg", ".jpeg", ".webp", ".gif")):
                pass
            elif "/images/" not in low:
                continue
            key = hashlib.md5(u.encode()).hexdigest()
            if key in seen:
                continue
            seen.add(key)
            out.append((u, d.get("sourceName", ""), d.get("sourceUrl", "")))
            if len(out) >= want * 3:
                return out
    return out


def download_images(candidates: list[tuple[str, str, str]], out_dir: str,
                    want: int = 6, min_bytes: int = 6000,
                    deadline: float | None = None) -> list[dict]:
    os.makedirs(out_dir, exist_ok=True)
    if not candidates:
        return []

    def one(idx_url: tuple[int, tuple[str, str, str]]) -> tuple[int, bytes, str, str, str] | None:
        idx, (url, source_name, page) = idx_url
        if deadline and time.time() > deadline:
            return None
        try:
            raw, ctype, _ = fetch(url, referer=page, timeout=12, max_bytes=8_000_000)
        except Exception:
            return None
        if len(raw) < min_bytes:
            return None
        ext = _sniff_ext(raw)
        if not ext:
            return None
        return idx, raw, ext, url, source_name

    ordered = list(enumerate(candidates[: max(want * 3, 6)]))
    got: list[tuple[int, dict]] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        futures = [pool.submit(one, item) for item in ordered]
        for fut in concurrent.futures.as_completed(
            futures, timeout=None if not deadline else max(5, deadline - time.time())
        ):
            try:
                res = fut.result()
            except Exception:
                continue
            if not res:
                continue
            idx, raw, ext, url, source_name = res
            got.append(
                (
                    idx,
                    {
                        "filename": "",
                        "ext": ext,
                        "sourceUrl": url,
                        "sourceName": source_name,
                        "caption": "",
                        "bytes": len(raw),
                        "_raw": raw,
                    },
                )
            )

    saved: list[dict] = []
    for idx, meta in sorted(got, key=lambda x: x[0]):
        if len(saved) >= want:
            break
        name = f"img_{len(saved) + 1}{meta['ext']}"
        try:
            with open(os.path.join(out_dir, name), "wb") as f:
                f.write(meta["_raw"])
        except OSError:
            continue
        saved.append(
            {
                "filename": name,
                "sourceUrl": meta["sourceUrl"],
                "sourceName": meta["sourceName"],
                "caption": "",
                "bytes": meta["bytes"],
            }
        )
    return saved
