import json
import os
import threading
import unittest
from http.client import HTTPConnection
from http.server import ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit
from unittest.mock import patch

import server


class OAuthTest(unittest.TestCase):
    def setUp(self):
        env = {
            "ZHIHU_OAUTH_APP_ID": "fixture-app",
            "ZHIHU_OAUTH_APP_KEY": "fixture-app-key",
            "ZHIHU_OAUTH_USERINFO_URL": f"{server.OPENAPI}/fixture-user",
            "ZHIHU_OAUTH_ACCESS_SECRET": "fixture-access-secret",
        }
        with patch.dict(os.environ, env, clear=True):
            self.oauth = server.OAuth()
        http = ThreadingHTTPServer(("127.0.0.1", 0), server.Handler)
        http.oauth = self.oauth
        self.port = http.server_port
        self.addCleanup(http.server_close)
        threading.Thread(target=http.serve_forever, daemon=True).start()
        self.addCleanup(http.shutdown)

    def request(self, path, body=None, cookie=None):
        connection = HTTPConnection("127.0.0.1", self.port, timeout=3)
        prefix = "/oauth/zhihu"
        headers = {"Content-Type": "application/json"} if body is not None else {}
        if cookie:
            headers["Cookie"] = cookie
        try:
            connection.request("POST" if body is not None else "GET", prefix + path,
                               json.dumps(body) if body is not None else None, headers)
            response = connection.getresponse()
            data = response.read().decode()
            if response.headers["Content-Type"].startswith("application/json"):
                data = json.loads(data)
            return response.status, response.headers, data
        finally:
            connection.close()

    def start(self):
        code, _, data = self.request("/start", {})
        self.assertEqual(code, 200)
        self.assertNotIn(data["secret"], data["url"])
        ticket = {"state": data["state"], "secret": data["secret"]}
        code, headers, _ = self.request("/launch?" + urlsplit(data["url"]).query)
        self.assertEqual(code, 302)
        self.assertIn("Secure", headers["Set-Cookie"])
        auth = parse_qs(urlsplit(headers["Location"]).query)
        self.assertEqual(auth, {
            "state": [ticket["state"]], "app_id": ["fixture-app"],
            "redirect_uri": [server.REDIRECT_URI], "response_type": ["code"],
        })
        return ticket, headers["Set-Cookie"].split(";", 1)[0]

    def test_authorize_once_and_keep_tokens_on_server(self):
        ticket, cookie = self.start()
        path = f"/callback?state={ticket['state']}&authorization_code=fixture-code"
        with patch.object(server, "upstream", side_effect=[
            {"access_token": "fixture-token", "token_type": "Bearer", "expires_in": 3600},
            {"id": "zhihu-user", "name": "测试答主"},
            {"Code": 0, "Data": {"Items": [
                {"ContentType": "answer", "Title": "示例", "Summary": "样本"},
            ], "Paging": {"IsEnd": True}}},
            {"Code": 0, "Data": {"Items": [], "Paging": {"IsEnd": True}}},
        ]) as upstream:
            self.assertEqual(self.request(path)[1]["Location"], "/oauth/zhihu/result/invalid")
            self.assertEqual(self.request("/callback?authorization_code=fixture-code", cookie=cookie)[1]["Location"],
                             "/oauth/zhihu/result/invalid")
            self.assertEqual(self.request(path, cookie=cookie)[1]["Location"], "/oauth/zhihu/result/complete")
            self.assertEqual(self.request(path, cookie=cookie)[1]["Location"], "/oauth/zhihu/result/invalid")
            self.assertEqual(self.request("/poll", {**ticket, "secret": "wrong"})[2], {"status": "expired"})
            authorized = self.request("/poll", ticket)[2]
            self.assertEqual(authorized["user"]["id"], "zhihu-user")
            self.assertNotIn("fixture-token", json.dumps(authorized))
            self.assertEqual(self.request("/poll", ticket)[2], {"status": "expired"})
            session = {"session": authorized["session"]}
            self.assertEqual(self.request("/profile", session)[2]["samples"][0]["text"], "样本")
            self.assertEqual(upstream.call_args_list[0].kwargs["form"], {
                "app_id": "fixture-app", "app_key": "fixture-app-key", "code": "fixture-code",
                "redirect_uri": server.REDIRECT_URI, "grant_type": "authorization_code",
            })
            self.assertEqual(upstream.call_args_list[2].kwargs["headers"]["X-OAuth-Token"], "fixture-token")
            self.request("/logout", session)
            self.assertEqual(self.request("/session", session)[0], 401)

    def test_cancellation_and_expiry(self):
        ticket, cookie = self.start()
        denied = f"/callback?state={ticket['state']}&error=access_denied"
        self.assertEqual(self.request(denied, cookie=cookie)[1]["Location"], "/oauth/zhihu/result/cancelled")
        self.assertEqual(self.request("/poll", ticket)[2], {"status": "cancelled"})
        ticket, _ = self.start()
        self.request("/cancel", ticket)
        self.assertEqual(self.request("/poll", ticket)[2], {"status": "expired"})
        ticket, _ = self.start()
        self.oauth.logins[ticket["state"]].deadline = 0
        self.assertEqual(self.request("/poll", ticket)[2], {"status": "expired"})
