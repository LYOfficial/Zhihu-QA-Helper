import io
import json
import os
import tempfile
import time
import unittest
import urllib.error
import urllib.parse
from pathlib import Path
from unittest.mock import patch

import investigation
import research
import zhihu_open_search as search


class ZhihuOpenSearchTests(unittest.TestCase):
    def test_private_file_and_environment_precedence(self):
        with tempfile.TemporaryDirectory() as folder, patch.dict(os.environ, {}, clear=True):
            self.assertFalse(search.access_secret())
            (Path(folder) / "zhihu-search.env").write_text("ZHIHU_ACCESS_SECRET=local-fixture\n", encoding="utf-8-sig")
            search.load_local_secret(folder)
            self.assertEqual(search.access_secret(), "local-fixture")
            os.environ[search.SECRET_NAME] = "env-fixture"
            search.load_local_secret(folder)
            self.assertEqual(search.access_secret(), "env-fixture")

    def test_official_contract_and_original_url(self):
        original = "https://www.zhihu.com/question/123?utm_source=openapi"
        for kind, limit in search.LIMITS.items():
            with self.subTest(kind=kind), patch.dict(os.environ, {search.SECRET_NAME: "fixture"}), \
                    patch.object(search.urllib.request, "build_opener") as opener:
                body = {"Code": 0, "Data": {"Items": [{"Url": original}, {"Url": original}, {}]}}
                opener.return_value.open.return_value = io.BytesIO(json.dumps(body).encode())
                self.assertEqual(search.search_urls(kind, "甲 & 乙", recent=True, count=100), [original])
                self.assertIsInstance(opener.call_args.args[0], search.NoRedirect)
                request = opener.return_value.open.call_args.args[0]
                url = urllib.parse.urlsplit(request.full_url)
                self.assertEqual((url.scheme, url.netloc, url.path),
                                 ("https", "developer.zhihu.com", f"/api/v1/content/{kind}"))
                self.assertEqual(request.get_method(), "GET")
                self.assertEqual(request.get_header("Authorization"), "Bearer fixture")
                self.assertLess(abs(int(request.get_header("X-request-timestamp")) - time.time()), 5)
                params = {"Query": ["甲 & 乙"], "Count": [str(limit)]}
                if kind == "global_search":
                    params["SearchDB"] = ["realtime"]
                self.assertEqual(urllib.parse.parse_qs(url.query), params)

    def test_no_key_network_or_bad_payload(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(search.urllib.request, "build_opener") as opener:
            self.assertEqual(search.search_urls("global_search", "测试"), [])
            opener.assert_not_called()
        with patch.dict(os.environ, {search.SECRET_NAME: "fixture"}), \
                patch.object(search.urllib.request, "build_opener") as opener:
            opened = opener.return_value.open
            for status in (401, 429, 500):
                opened.side_effect = urllib.error.HTTPError(search.ENDPOINT, status, "do-not-log", None, None)
                with self.assertRaisesRegex(search.SearchError, f"^HTTP {status}$"):
                    search.search_urls("global_search", "测试")
            opened.side_effect = TimeoutError("do-not-log")
            with self.assertRaisesRegex(search.SearchError, "^网络或响应格式异常$"):
                search.search_urls("global_search", "测试")
            opened.side_effect = None
            opened.return_value = io.BytesIO(b'{"Code":30001,"Message":"do-not-log"}')
            with self.assertRaisesRegex(search.SearchError, "^接口错误码 30001$"):
                search.search_urls("global_search", "测试")
            opened.return_value = io.BytesIO(b'{"Code":0,"Data":{}}')
            with self.assertRaisesRegex(search.SearchError, "^响应缺少搜索结果列表$"):
                search.search_urls("global_search", "测试")

    def test_research_keeps_original_sources_on_quota_failure(self):
        official = {"title": "信号研究", "text": "核查信号研究结论。", "trust": 4, "sourceUrl": "https://example.org/paper"}
        community = {"title": "信号研究讨论", "text": "社区讨论了信号研究。", "trust": 2,
                     "sourceUrl": "https://www.zhihu.com/question/123?utm_source=openapi"}
        with patch.dict(os.environ, {search.SECRET_NAME: "fixture"}), \
                patch.object(search, "search_urls", side_effect=lambda kind, *args, **kwargs:
                             [community["sourceUrl"]] if kind == "zhihu_search" else self.quota_error()) as provider, \
                patch.object(research, "gather", return_value=[official]) as directory, \
                patch.object(research, "gather_web", return_value=[]) as legacy, \
                patch.object(research, "_fetch_doc", return_value=community) as read:
            docs = investigation.gather_queries(["信号研究", "核查结论"], [], time.time() + 3)
        self.assertEqual(docs, [official, community])
        self.assertCountEqual([call.args[0] for call in provider.call_args_list], ["zhihu_search", "global_search"])
        self.assertEqual(read.call_args.args[0], community["sourceUrl"])
        self.assertFalse(read.call_args.kwargs["authority"])
        self.assertTrue(directory.called and legacy.called)

    @staticmethod
    def quota_error():
        raise search.SearchError("接口错误码 30001")
