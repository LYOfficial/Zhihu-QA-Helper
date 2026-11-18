from __future__ import annotations

import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


SECRET_NAME = "ZHIHU_ACCESS_SECRET"
ENDPOINT = "https://developer.zhihu.com/api/v1/content"
LIMITS = {"zhihu_search": 10, "global_search": 20}
MAX_RESPONSE = 4 * 1024 * 1024


class SearchError(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def load_local_secret(data_dir: str) -> None:
    if SECRET_NAME in os.environ:
        return
    try:
        lines = (Path(data_dir) / "zhihu-search.env").read_text(encoding="utf-8-sig").splitlines()
    except (OSError, UnicodeError):
        return
    for line in lines:
        key, separator, value = line.partition("=")
        if separator and key.strip() == SECRET_NAME:
            os.environ.setdefault(SECRET_NAME, value.strip())
            return


def access_secret() -> str:
    return os.environ.get(SECRET_NAME, "").strip()


def search_urls(kind: str, query: str, *, count=6, timeout=5, recent=False) -> list[str]:
    secret = access_secret()
    if not secret or not query.strip():
        return []
    if any(not 33 <= ord(character) <= 126 for character in secret):
        raise SearchError("凭证格式无效")
    params = {"Query": query.strip(), "Count": min(max(count, 1), LIMITS[kind])}
    if kind == "global_search" and recent:
        params["SearchDB"] = "realtime"
    request = urllib.request.Request(f"{ENDPOINT}/{kind}?{urllib.parse.urlencode(params)}", headers={
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
    items = data.get("Items") if isinstance(data, dict) else None
    if not isinstance(items, list):
        raise SearchError("响应缺少搜索结果列表")
    return list(dict.fromkeys(item["Url"].strip() for item in items[:params["Count"]]
                              if isinstance(item, dict) and isinstance(item.get("Url"), str) and item["Url"].strip()))


def check_credentials() -> int:
    if not access_secret():
        print("未配置知乎开放平台搜索凭证，跳过联网验证。")
        return 0
    failed = False
    for kind in LIMITS:
        try:
            search_urls(kind, "知乎开放平台", count=1)
            print(f"{kind}：接口调用成功。")
        except SearchError as error:
            failed = True
            print(f"{kind}：{error}。")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(check_credentials())
