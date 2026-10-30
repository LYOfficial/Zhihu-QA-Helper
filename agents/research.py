"""联网取材：抓权威平台页面、抽正文、筛图并落地保存。

只用标准库。设计取向上偏"保守"——宁可少抓，也不把不可信内容喂给模型。
"""

from __future__ import annotations

import gzip
import hashlib
import io
import json
import os
import re
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


def _decode(raw: bytes, charset: str | None) -> str:
    for enc in [charset, "utf-8", "gb18030", "latin-1"]:
        if not enc:
            continue
        try:
            return raw.decode(enc, "strict")
        except (UnicodeDecodeError, LookupError):
            continue
    return raw.decode("utf-8", "replace")


def fetch(url: str, referer: str = "", timeout: int = 20, max_bytes: int = 3_000_000):
    req = urllib.request.Request(url, method="GET")
    req.add_header("User-Agent", UA)
    req.add_header("Accept", "text/html,application/xhtml+xml,image/*,*/*;q=0.8")
    req.add_header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
    req.add_header("Accept-Encoding", "gzip, deflate")
    if referer:
        req.add_header("Referer", referer)
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


def mediawiki_search(base: str, query: str, limit: int = 4) -> list[dict]:
    """MediaWiki 的 REST 搜索接口，各类维基站点都开放了这个端点。"""
    base = base.rstrip("/")
    url = f"{base}/rest.php/v1/search/page?q={urllib.parse.quote(query)}&limit={limit}"
    try:
        raw, ctype, charset = fetch(url, timeout=15)
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


def gather(query: str, sources: list[dict], max_pages: int = 4) -> list[dict]:
    """按可信度从高到低检索，凑够 max_pages 篇正文就停。"""
    docs: list[dict] = []
    ordered = sorted(
        [s for s in sources if s.get("enabled", True)],
        key=lambda s: -int(s.get("trust") or 3),
    )
    for s in ordered:
        if len(docs) >= max_pages:
            break
        base = (s.get("baseUrl") or "").rstrip("/")
        if not base:
            continue

        if s.get("kind") == "wiki" or "wiki" in base:
            for hit in mediawiki_search(base, query, limit=2):
                if len(docs) >= max_pages:
                    break
                try:
                    raw, ctype, charset = fetch(hit["url"], timeout=18)
                except Exception:
                    continue
                title, text, imgs = parse_doc(_decode(raw, charset), hit["url"])
                if len(text) < 200:
                    continue
                docs.append(
                    {
                        "sourceName": s.get("name") or base,
                        "sourceUrl": hit["url"],
                        "title": title or hit["title"],
                        "text": text[:9000],
                        "images": imgs,
                        "trust": s.get("trust") or 3,
                    }
                )
            continue

        surl = _guess_search_url(s, query)
        if not surl:
            continue
        try:
            raw, ctype, charset = fetch(surl, timeout=18)
        except Exception:
            continue
        if "json" in (ctype or ""):
            try:
                data = json.loads(_decode(raw, charset))
                hits = (data.get("hits") or [])[:2]
                for h in hits:
                    url = f"https://modrinth.com/mod/{h.get('slug')}"
                    try:
                        r2, c2, ch2 = fetch(url, timeout=18)
                        title, text, imgs = parse_doc(_decode(r2, ch2), url)
                        if len(text) < 120:
                            continue
                        docs.append(
                            {
                                "sourceName": s.get("name") or base,
                                "sourceUrl": url,
                                "title": title,
                                "text": text[:6000],
                                "images": imgs,
                                "trust": s.get("trust") or 3,
                            }
                        )
                    except Exception:
                        continue
            except Exception:
                pass
            continue

        title, text, imgs = parse_doc(_decode(raw, charset), surl)
        # 搜索结果页正文价值低，取其摘要即可
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
                    want: int = 6, min_bytes: int = 6000) -> list[dict]:
    os.makedirs(out_dir, exist_ok=True)
    saved: list[dict] = []
    for url, source_name, page in candidates:
        if len(saved) >= want:
            break
        try:
            raw, ctype, _ = fetch(url, referer=page, timeout=25, max_bytes=8_000_000)
        except Exception:
            continue
        if len(raw) < min_bytes:
            continue
        ext = _sniff_ext(raw)
        if not ext:
            continue
        name = f"img_{len(saved) + 1}{ext}"
        try:
            with open(os.path.join(out_dir, name), "wb") as f:
                f.write(raw)
        except OSError:
            continue
        saved.append(
            {
                "filename": name,
                "sourceUrl": url,
                "sourceName": source_name,
                "caption": "",
                "bytes": len(raw),
            }
        )
    return saved
