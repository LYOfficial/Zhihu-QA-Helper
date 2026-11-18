import base64
import binascii
import json
import os
import re
import time
import urllib.parse
from html.parser import HTMLParser

import research
import investigation


MEDIA = (
    ("百度贴吧", "tieba.baidu.com", r"/p/\d+/?", {"d_post_content", "j_d_post_content"},
     ("tiebapic.baidu.com", "imgsa.baidu.com", "imgsrc.baidu.com")),
    ("知乎", "zhihu.com", r"/(?:question/\d+(?:/answer/\d+)?|p/\d+)/?", {"RichText", "Post-RichText"},
     ("zhimg.com",)),
    ("小黑盒", "xiaoheihe.cn", r"/(?:app/bbs/(?:link|post)/[^/]+/?|bb[sx]/[^/]+/?)",
     {"article-content", "post-content", "bbs-content", "ql-editor"}, ("max-c.com", "xiaoheihe.cn")),
)

IMAGE_SITES = (
    ("Minecraft 官网", ("minecraft.net",), ("minecraft.net", "mojang.com")),
    ("Minecraft Wiki", ("minecraft.wiki",), ("minecraft.wiki",)),
    ("Wikimedia Commons", ("commons.wikimedia.org",), ("upload.wikimedia.org",)),
    ("Pexels", ("pexels.com",), ("images.pexels.com",)),
    ("Unsplash", ("unsplash.com",), ("images.unsplash.com",)),
    ("Pixabay", ("pixabay.com",), ("cdn.pixabay.com",)),
    ("机核", ("gcores.com",), ("image.gcores.com",)),
    ("GameRes", ("gameres.com",), ("di.gameres.com",)),
    ("中国日报", ("chinadaily.com.cn",), ("chinadaily.com.cn",)),
    ("中国铁路", ("china-railway.com.cn",), ("china-railway.com.cn",)),
    ("新浪", ("sina.com.cn", "sina.cn"), ("sinaimg.cn",)),
)


def matches_host(url, domains):
    try:
        parts = urllib.parse.urlsplit(url)
        host = parts.hostname or ""
        return (parts.scheme == "https" and parts.port in (None, 443) and not parts.username
                and not parts.password and any(host == base or host.endswith("." + base) for base in domains))
    except ValueError:
        return False


def original_url(url, domains):
    if not matches_host(url, domains):
        return False
    suffix = urllib.parse.unquote(urllib.parse.urlsplit(url).path + "?" + urllib.parse.urlsplit(url).query).lower()
    return not any(hint in suffix for hint in (*research.BAD_IMG_HINTS, "watermark", "水印", "thumbnail"))


def relevant(phrase, title):
    visual_noise = {"我的", "世界", "游戏", "图片", "配图", "截图", "photo", "image", "screenshot", "minecraft", "mc"}
    def terms(text):
        normalized = re.sub(r"\boff[ -]?hand\b|副手", " offhand ", text, flags=re.I)
        normalized = re.sub(r"\b(?:boomerang|throwing[ -]di[sc]c?)\b|回旋飞盘|回旋镖", " boomerang ", normalized, flags=re.I)
        normalized = re.sub(r"\bbedrock\b|基岩版", " bedrock ", normalized, flags=re.I)
        return research.keywords(normalized) - visual_noise

    query, image = terms(phrase), terms(title)
    required = query & {"boomerang", "offhand"}
    if required and not required <= image:
        return False
    matches = query & image
    return bool(matches) if len(query) <= 1 else len(matches) >= min(2, len(query))


def media_page(spec, url):
    _, domain, path, _, _ = spec
    return matches_host(url, (domain,)) and bool(re.fullmatch(path, urllib.parse.urlsplit(url).path))


def media_urls(spec, phrase):
    return research.web_search(f"{phrase} site:{spec[1]}", limit=3, timeout=4,
                               allowed=lambda url: media_page(spec, url))


def media_candidates(spec, url, topic):
    label, _, _, markers, cdns = spec
    raw, _, charset = research.fetch(url, timeout=4, allowed=lambda link: media_page(spec, link))
    title, _, images = research.parse_doc(research._decode(raw, charset), url, markers)
    allowed = lambda link: original_url(link, cdns)
    if label == "知乎":
        allowed = lambda link: original_url(link, cdns) and not re.search(
            r"_(?:xs|s|m|l|b|hd|720w|1440w)(?:\.[^/]+)$", urllib.parse.urlsplit(link).path
        )
    images = [img for img in images if allowed(img["url"])]
    if not topic:
        return [{"url": item["url"], "page": url, "caption": item["caption"] or title[:160],
                 "sourceName": label, "allowed": allowed} for item in images[:24]]
    docs = [{"sourceUrl": url, "sourceName": label, "title": title, "trust": 2,
             "authority": True, "allowImage": True, "images": images}]
    return [{**item, "allowed": allowed} for item in research.pick_image_urls(docs, 4, topic)]


class _ImageResults(HTMLParser):
    def __init__(self):
        super().__init__()
        self.results = []

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if tag != "a" or "iusc" not in (values.get("class") or "").split():
            return
        try:
            result = json.loads(values.get("m") or "{}")
        except ValueError:
            return
        if isinstance(result, dict):
            self.results.append(result)


def bing_image_candidates(phrase):
    endpoint = "https://cn.bing.com/images/search?" + urllib.parse.urlencode({"q": phrase})
    raw, _, charset = research.fetch(endpoint, timeout=5, allowed=lambda link: matches_host(link, ("bing.com",)))
    parser = _ImageResults()
    parser.feed(research._decode(raw, charset))
    candidates, media = [], []
    for hit in parser.results[:45]:
        page = str(hit.get("purl") or "")
        url = str(hit.get("murl") or "")
        title = str(hit.get("t") or "")[:160]
        if not relevant(phrase, title):
            continue
        spec = next((spec for spec in MEDIA if media_page(spec, page)), None)
        if spec:
            media.append((spec, page))
            continue
        for name, pages, cdn in IMAGE_SITES:
            if matches_host(page, pages) and original_url(url, cdn):
                candidates.append({"url": url, "page": page, "caption": title,
                                   "sourceName": name, "allowed": lambda link, domains=cdn: original_url(link, domains)})
                break
    return candidates[:10], media[:5]


COMMONS_ENDPOINT = "https://commons.wikimedia.org/w/api.php"


def commons_candidates(phrase):
    query = urllib.parse.urlencode({
        "action": "query", "generator": "search", "gsrnamespace": 6, "gsrlimit": 7,
        "gsrsearch": f"{phrase} filetype:bitmap", "prop": "imageinfo",
        "iiprop": "url|size|extmetadata", "iiurlwidth": 1200,
        "iiextmetadatafilter": "LicenseShortName", "format": "json", "formatversion": 2,
    })
    raw, _, _ = research.fetch(f"{COMMONS_ENDPOINT}?{query}", timeout=5,
                               allowed=lambda link: matches_host(link, ("commons.wikimedia.org",)))
    pages = json.loads(raw).get("query", {}).get("pages", [])
    allowed = lambda url: matches_host(url, ("upload.wikimedia.org",))
    candidates = []
    for page in sorted(pages, key=lambda item: item.get("index", 999)):
        info = next(iter(page.get("imageinfo") or []), {})
        url, source = info.get("thumburl", info.get("url", "")), info.get("descriptionurl", "")
        title = page.get("title", "").removeprefix("File:")
        license_name = info.get("extmetadata", {}).get("LicenseShortName", {}).get("value", "")
        if (allowed(url) and matches_host(source, ("commons.wikimedia.org",)) and license_name
                and min(info.get("width", 0), info.get("height", 0)) >= 320
                and relevant(phrase, title)):
            candidates.append({"url": url, "page": source, "caption": title[:160],
                               "sourceName": f"Wikimedia Commons · {license_name[:36]}", "allowed": allowed})
    return candidates


def find_candidates(plans, sources, docs, deadline):
    routes = [(plan, phrase) for plan in plans[:3] for phrase in plan["queries"][:2]]
    requests = [(kind, plan, phrase) for plan, phrase in routes for kind in ("directory", "bing", "commons", *MEDIA)]

    def from_docs(pages, phrase):
        candidates = []
        for doc in pages:
            if not (doc.get("authority") and doc.get("allowImage")):
                continue
            source_title = doc.get("title", "")
            for image in doc.get("images", [])[:50]:
                uri = image["url"]
                path = urllib.parse.unquote(urllib.parse.urlsplit(uri).path).rsplit("/", 1)[-1]
                caption = image.get("caption") or ""
                if any(hint in path.lower() for hint in research.BAD_IMG_HINTS):
                    continue
                matched = relevant(phrase, f"{caption} {path}")
                if not matched and (not caption or not relevant(phrase, source_title)):
                    continue
                auto = matched and doc.get("sourceKind") in {"official", "docs", "wiki"}
                candidates.append({"url": uri, "page": doc["sourceUrl"], "sourceName": doc["sourceName"],
                                   "caption": caption or source_title, "autoInsert": auto})
        return candidates

    def search(request):
        spec, plan, phrase = request
        if spec == "directory":
            approved = [source for source in sources if research._allow_image(source)]
            hosts = set(plan.get("hosts") or [])
            scoped = [source for source in approved if research._host(source["baseUrl"]) in hosts or
                      research._host(source["baseUrl"]).removeprefix("www.") in hosts]
            pages = research.gather(phrase, scoped or approved, 5, min(deadline, time.time() + 12))
            return from_docs([*docs, *pages], phrase), []
        if spec == "bing":
            images, pages = bing_image_candidates(phrase)
            return images, [(source, url, phrase) for source, url in pages]
        if spec == "commons":
            return commons_candidates(phrase), []
        return [], [(spec, link, phrase) for link in media_urls(spec, phrase)[:2]]

    groups = research._parallel(search, requests, 18, min(deadline, time.time() + 13))
    pages, files, seen = [], [], set()
    for hits, located in groups:
        files.extend(hits)
        for spec, link, phrase in located:
            key = research.canonical_url(link)
            if key not in seen:
                seen.add(key)
                pages.append((spec, link, phrase))
    for group in research._parallel(lambda entry: media_candidates(*entry), pages[:14], 10, deadline):
        files.extend(group)
    return sorted(files, key=lambda item: bool(item.get("autoInsert")), reverse=True)


def page_candidates(url):
    spec = next((spec for spec in MEDIA if media_page(spec, url)), None)
    if not spec:
        raise ValueError("请填写百度贴吧帖子、知乎问题/回答/文章或小黑盒公开帖子的 HTTPS 链接")
    try:
        return media_candidates(spec, url, "")
    except (OSError, ValueError) as error:
        raise ValueError("无法打开该公开页面，可在浏览器检查后导入本地图片") from error


def import_image(payload):
    encoded = payload.get("imageData", "")
    if not isinstance(encoded, str) or len(encoded) > 8_000_000:
        raise ValueError("图片文件不能超过 6 MB")
    try:
        image = research._normal_image(base64.b64decode(encoded, validate=True))
    except (ValueError, OSError, binascii.Error) as error:
        raise ValueError("请选择尺寸合适、清晰的 PNG、JPEG 或 WebP 图片") from error
    folder = payload["imagesDir"]
    existing = payload.get("existingImages") or []
    os.makedirs(folder, exist_ok=True)
    source = str(payload.get("sourceUrl") or "")
    item = {"page": source, "sourceName": "手动导入", "caption": str(payload.get("caption") or "")[:160]}
    asset = research._save_image(item, image, folder, set(existing), research.existing_signatures(folder, existing))
    return {"images": [asset] if asset else []}
