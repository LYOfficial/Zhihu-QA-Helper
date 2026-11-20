import io
import json
import os
import time
import unittest
import urllib.error
import urllib.parse
from unittest.mock import patch

import investigation
import main
import research
import zhihu_open_search as search


class ZhihuOpenSearchTests(unittest.TestCase):
    def test_question_discovery_falls_back_on_quota_and_excludes_other_content(self):
        expected = "https://www.zhihu.com/question/123/answer/456"

        def fallback(query, **options):
            self.assertEqual(query, "site:zhihu.com/question 铁路信号")
            self.assertTrue(options["recent"])
            return [url for url in [expected, "https://zhuanlan.zhihu.com/p/123", "https://example.org/question/123"]
                    if options["allowed"](url)]

        with patch.object(search, "search_urls", side_effect=search.SearchError("HTTP 429")), \
                patch.object(research, "web_search", side_effect=fallback):
            self.assertEqual(main.discover_questions({"topics": ["铁路信号"], "zhihuAccessSecret": "fixture"}), {"urls": [expected]})

    def test_disabled_path_ignores_legacy_environment_secret(self):
        with patch.dict(os.environ, {"ZHIHU_ACCESS_SECRET": "old-fixture"}), \
                patch.object(search, "search_urls") as provider, \
                patch.object(research, "gather", return_value=[]), \
                patch.object(research, "gather_web", return_value=[]) as legacy:
            self.assertEqual(investigation.gather_queries(["测试"], [], time.time() + 3), [])
        provider.assert_not_called()
        legacy.assert_called()

    def test_official_contract_and_original_url(self):
        original = "https://www.zhihu.com/question/123?utm_source=openapi"
        for kind, limit in search.LIMITS.items():
            with self.subTest(kind=kind), patch.object(search.urllib.request, "build_opener") as opener:
                body = {"Code": 0, "Data": {"Items": [{"Url": original}, {"Url": original}, {}]}}
                opener.return_value.open.return_value = io.BytesIO(json.dumps(body).encode())
                self.assertEqual(search.search_urls(kind, "甲 & 乙", secret="fixture", recent=True, count=100), [original])
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
        with patch.object(search.urllib.request, "build_opener") as opener:
            opened = opener.return_value.open
            for status in (401, 429, 500):
                opened.side_effect = urllib.error.HTTPError(search.ENDPOINT, status, "do-not-log", None, None)
                with self.assertRaisesRegex(search.SearchError, f"^HTTP {status}$"):
                    search.search_urls("global_search", "测试", secret="fixture")
            opened.side_effect = TimeoutError("do-not-log")
            with self.assertRaisesRegex(search.SearchError, "^网络或响应格式异常$"):
                search.search_urls("global_search", "测试", secret="fixture")
            opened.side_effect = None
            opened.return_value = io.BytesIO(b'{"Code":30001,"Message":"do-not-log"}')
            with self.assertRaisesRegex(search.SearchError, "^接口错误码 30001$"):
                search.search_urls("global_search", "测试", secret="fixture")
            opened.return_value = io.BytesIO(b'{"Code":0,"Data":{}}')
            with self.assertRaisesRegex(search.SearchError, "^响应缺少结果列表$"):
                search.search_urls("global_search", "测试", secret="fixture")

    def test_user_contents_contract(self):
        item = {"ContentType": "answer", "Title": "问题", "Summary": "摘要", "LikeCount": 12}
        with patch.object(search.urllib.request, "build_opener") as opener:
            opener.return_value.open.return_value = io.BytesIO(json.dumps({"Code": 0, "Data": {"Items": [item]}}).encode())
            self.assertEqual(search.user_samples("fixture", "answer", 24), [
                {"kind": "answer", "title": "问题", "text": "摘要", "voteups": 12, "summaryOnly": True},
            ])
            request = opener.return_value.open.call_args.args[0]
        self.assertEqual(request.full_url, f"{search.ENDPOINT}/user/contents?ContentType=answer&Limit=24")
        self.assertEqual(request.get_header("Authorization"), "Bearer fixture")
        self.assertIsNone(request.get_header("X-oauth-token"))

    def test_credentials_report_each_interface_without_content(self):
        with patch.object(search, "user_samples", return_value=[{"text": "private-fixture"}]) as profile, \
                patch.object(search, "search_urls", side_effect=[[], search.SearchError("HTTP 429")]) as provider:
            results = search.check_credentials("fixture")
        profile.assert_called_once_with("fixture", "answer", 1)
        self.assertEqual(provider.call_count, 2)
        self.assertTrue(results["user_contents"]["ok"] and results["zhihu_search"]["ok"])
        self.assertEqual(results["global_search"], {"ok": False, "message": "HTTP 429"})
        self.assertNotIn("private-fixture", json.dumps(results))

    def test_research_keeps_original_sources_on_quota_failure(self):
        official = {"title": "信号研究", "text": "核查信号研究结论。", "trust": 4, "sourceUrl": "https://example.org/paper"}
        community = {"title": "信号研究讨论", "text": "社区讨论了信号研究。", "trust": 2,
                     "sourceUrl": "https://www.zhihu.com/question/123?utm_source=openapi"}
        with patch.object(search, "search_urls", side_effect=lambda kind, *args, **kwargs:
                             [community["sourceUrl"]] if kind == "zhihu_search" else self.quota_error()) as provider, \
                patch.object(research, "gather", return_value=[official]) as directory, \
                patch.object(research, "gather_web", return_value=[]) as legacy, \
                patch.object(research, "_fetch_doc", return_value=community) as read:
            docs = investigation.gather_queries(["信号研究", "核查结论"], [], time.time() + 3, zhihu_secret="fixture")
        self.assertEqual(docs, [official, community])
        self.assertCountEqual([call.args[0] for call in provider.call_args_list], ["zhihu_search", "global_search"])
        self.assertTrue(all(call.kwargs["secret"] == "fixture" for call in provider.call_args_list))
        self.assertEqual(read.call_args.args[0], community["sourceUrl"])
        self.assertFalse(read.call_args.kwargs["authority"])
        self.assertTrue(directory.called and legacy.called)

    @staticmethod
    def quota_error():
        raise search.SearchError("接口错误码 30001")
