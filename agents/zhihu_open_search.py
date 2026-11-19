from __future__ import annotations

import json
import time
import urllib.error
import urllib.parse
import urllib.request


ENDPOINT = "https://developer.zhihu.com/api/v1"
LIMITS = {"zhihu_search": 10, "global_search": 20}
MAX_RESPONSE = 4 * 1024 * 1024


class SearchError(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def request_items(path: str, params: dict, secret: str, timeout: float) -> list:
    if not isinstance(secret, str) or not secret or len(secret) > 4096 or any(not 33 <= ord(character) <= 126 for character in secret):
        raise SearchError("凭证格式无效")
    request = urllib.request.Request(f"{ENDPOINT}/{path}?{urllib.parse.urlencode(params)}", headers={
        "Authorization": f"Bearer {secret}",
        "X-Request-Timestamp": str(int(time.time())),
        "Content-Type": "application/json",
    })
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=timeout) as response:
            raw = response.read(MAX_RESPONSE + 1)
        if len(raw) > MAX_RESPONSE:
            raise SearchError("响应过大")
        body = json.loads(raw)
    except urllib.error.HTTPError as error:
        error.close()
        raise SearchError(f"HTTP {error.code}") from None
    except (OSError, ValueError):
        raise SearchError("网络或响应格式异常") from None
    if not isinstance(body, dict) or type(body.get("Code")) is not int:
        raise SearchError("响应缺少状态码")
    if body["Code"] != 0:
        raise SearchError(f"接口错误码 {body['Code']}")
    data = body.get("Data")
    if not isinstance(data, dict) or not isinstance(data.get("Items"), list):
        raise SearchError("响应缺少结果列表")
    return data["Items"]


def search_urls(kind: str, query: str, *, secret="", count=6, timeout=5, recent=False) -> list[str]:
    if not secret or not query.strip():
        return []
    params = {"Query": query.strip(), "Count": min(max(count, 1), LIMITS[kind])}
    if kind == "global_search" and recent:
        params["SearchDB"] = "realtime"
    items = request_items(f"content/{kind}", params, secret, timeout)
    return list(dict.fromkeys(item["Url"].strip() for item in items[:params["Count"]]
                              if isinstance(item, dict) and isinstance(item.get("Url"), str) and item["Url"].strip()))


def user_samples(secret: str, kind: str, count: int) -> list[dict]:
    items = request_items("user/contents", {"ContentType": kind, "Limit": count}, secret, 5)
    return [{"kind": kind, "title": item["Title"][:300], "text": item["Summary"][:32000],
             "voteups": item["LikeCount"] if type(item.get("LikeCount")) is int else 0, "summaryOnly": True}
            for item in items[:count]
            if isinstance(item, dict) and item.get("ContentType") == kind
            and isinstance(item.get("Title"), str) and isinstance(item.get("Summary"), str)
            and (item["Title"].strip() or item["Summary"].strip())]


def profile_samples(secret: str) -> dict:
    samples = {}
    for kind, limit in (("answer", 24), ("article", 16)):
        try:
            samples[kind] = user_samples(secret, kind, limit)
        except SearchError:
            samples[kind] = []
    return samples


def check_credentials(secret: str) -> dict:
    checks = {
        "user_contents": lambda: user_samples(secret, "answer", 1),
        "zhihu_search": lambda: search_urls("zhihu_search", "知乎开放平台", secret=secret, count=1),
        "global_search": lambda: search_urls("global_search", "知乎开放平台", secret=secret, count=1),
    }
    results = {}
    for kind, check in checks.items():
        try:
            check()
            results[kind] = {"ok": True, "message": "连接成功"}
        except SearchError as error:
            results[kind] = {"ok": False, "message": str(error)}
    return results
