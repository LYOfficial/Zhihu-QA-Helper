import json
import os
import secrets
import time
from argparse import ArgumentParser
from dataclasses import dataclass
from http.cookies import CookieError, SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Lock, Thread
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


CALLBACK = "/oauth/zhihu/callback"
REDIRECT_URI = "https://zh.ijune.cn" + CALLBACK
OPENAPI = "https://openapi.zhihu.com"
USERINFO_URL = OPENAPI + "/user"
CONTENTS = "https://developer.zhihu.com/api/v1/user/contents"
COOKIE = "__Host-zhihu_qa_oauth"
CONTROL_URL = "http://127.0.0.1:33112"
LOGIN_TTL = 300
CAPACITY = 512


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_args):
        return None


class AuthorizationExpired(ValueError):
    pass


def upstream(url, *, form=None, headers=None):
    data = urlencode(form).encode() if form else None
    try:
        with build_opener(NoRedirect()).open(Request(url, data, headers or {}), timeout=8) as response:
            body = response.read(2 * 1024 * 1024 + 1)
        if len(body) <= 2 * 1024 * 1024:
            result = json.loads(body)
            if isinstance(result, dict):
                return result
    except HTTPError as error:
        error.close()
        if error.code == 401:
            raise AuthorizationExpired("知乎授权已失效") from None
    except (OSError, ValueError):
        pass
    raise ValueError("知乎接口请求失败")


def text_field(value, limit=300):
    return value.strip()[:limit] if isinstance(value, str) else ""


@dataclass
class Login:
    secret: str
    deadline: float
    diagnostic: bool = False
    browser: str = ""
    status: str = "pending"
    result: dict | None = None


@dataclass
class Session:
    token: str
    expires: int
    user: dict


class OAuth:
    def __init__(self):
        self.app_id = os.environ.get("ZHIHU_OAUTH_APP_ID", "").strip()
        self.app_key = os.environ.get("ZHIHU_OAUTH_APP_KEY", "").strip()
        self.userinfo = os.environ.get("ZHIHU_OAUTH_USERINFO_URL", "").strip() or USERINFO_URL
        self.userinfo_path = os.environ.get("ZHIHU_OAUTH_USERINFO_PATH", "").strip()
        self.id_field = os.environ.get("ZHIHU_OAUTH_USERINFO_ID_FIELD", "id").strip()
        self.name_field = os.environ.get("ZHIHU_OAUTH_USERINFO_NAME_FIELD", "name").strip()
        self.access_secret = os.environ.get("ZHIHU_OAUTH_ACCESS_SECRET", "").strip()
        try:
            url = urlsplit(self.userinfo)
        except ValueError:
            url = urlsplit("")
        self.missing_credentials = [
            name for name, value in (("ZHIHU_OAUTH_APP_ID", self.app_id), ("ZHIHU_OAUTH_APP_KEY", self.app_key))
            if not value
        ]
        self.identity_ready = bool(
            self.id_field and self.name_field and url.scheme == "https"
            and url.netloc in {"openapi.zhihu.com", "developer.zhihu.com"}
            and url.path and not url.fragment
            and (not self.userinfo_path or self.userinfo_path.startswith("/"))
        )
        self.enabled = not self.missing_credentials and self.identity_ready
        self.lock = Lock()
        self.logins = {}
        self.sessions = {}

    def prune(self):
        self.logins = {key: value for key, value in self.logins.items() if value.deadline > time.monotonic()}
        self.sessions = {key: value for key, value in self.sessions.items() if value.expires > time.time()}

    def start(self, diagnostic=False):
        secret, state = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
        self.logins[state] = Login(secret, time.monotonic() + LOGIN_TTL, diagnostic)
        url = REDIRECT_URI.rsplit("/", 1)[0] + "/launch?" + urlencode({"state": state})
        return {"url": url, "state": state, "secret": secret}

    def identity(self, token):
        info = upstream(self.userinfo, headers={"Authorization": f"Bearer {token}"})
        if self.userinfo_path:
            for key in self.userinfo_path[1:].split("/"):
                if not isinstance(info, dict):
                    raise ValueError("知乎用户信息格式有误")
                info = info.get(key.replace("~1", "/").replace("~0", "~"))
        if not isinstance(info, dict):
            raise ValueError("知乎用户信息格式有误")
        default_endpoint = self.userinfo == USERINFO_URL
        account_id = info.get("hash_id" if default_endpoint else self.id_field)
        if type(account_id) is int:
            account_id = str(account_id)
        name = text_field(info.get("fullname" if default_endpoint else self.name_field))
        if not isinstance(account_id, str) or not account_id.strip() or len(account_id) > 256 or not name:
            raise ValueError("知乎没有返回稳定账号标识和昵称")
        avatar = text_field(info.get("avatar_path" if default_endpoint else "avatar_url"), 2048)
        url_token = text_field(info.get("url_token"))
        if default_endpoint:
            try:
                url = urlsplit(text_field(info.get("url"), 2048))
            except ValueError:
                url = urlsplit("")
            if url.netloc in {"", "www.zhihu.com", "zhihu.com"} and url.scheme in {"", "http", "https"}:
                prefix, separator, slug = url.path.rstrip("/").rpartition("/")
                if separator and prefix == "/people":
                    url_token = text_field(slug)
                elif not separator and not url.netloc and slug.isascii() and all(c.isalnum() or c in "-_" for c in slug):
                    url_token = text_field(slug)
        return {
            "id": account_id.strip(),
            "name": name,
            "urlToken": url_token,
            "avatarUrl": avatar if avatar.startswith("https://") else "",
            "headline": text_field(info.get("headline")),
            "answerCount": self.count(info.get("answer_count")),
            "articlesCount": self.count(info.get("articles_count")),
            "followerCount": self.count(info.get("follower_count")),
        }

    @staticmethod
    def count(value):
        return value if type(value) is int and 0 <= value < 2 ** 63 else 0

    def exchange(self, code):
        payload = upstream(OPENAPI + "/access_token", form={
            "app_id": self.app_id, "app_key": self.app_key,
            "grant_type": "authorization_code", "redirect_uri": REDIRECT_URI, "code": code,
        })
        token, expires = payload.get("access_token"), payload.get("expires_in")
        if (
            not isinstance(token, str) or not 0 < len(token) <= 4096
            or not token.isascii() or not token.isprintable() or " " in token
            or str(payload.get("token_type", "")).lower() != "bearer"
            or type(expires) is not int or not 0 < expires <= 366 * 86400
        ):
            raise ValueError("知乎没有返回有效的访问令牌")
        return token, int(time.time()) + expires

    def profile(self, session):
        headers = {
            "Authorization": f"Bearer {self.access_secret}",
            "X-Request-Timestamp": str(int(time.time())),
            "X-OAuth-Token": session.token,
            "Content-Type": "application/json",
        }
        samples = []
        for kind, limit in (("answer", 24), ("article", 16)):
            offset = 0
            while limit > 0:
                url = CONTENTS + "?" + urlencode({"ContentType": kind, "Limit": min(limit, 20), "Offset": offset})
                body = upstream(url, headers=headers)
                data = body.get("Data")
                if body.get("Code") != 0 or not isinstance(data, dict) or not isinstance(data.get("Items"), list):
                    raise ValueError("知乎未返回公开创作内容，请检查应用授权和接口额度")
                items = data["Items"][:min(limit, 20)]
                for row in items:
                    if not isinstance(row, dict) or row.get("ContentType") != kind:
                        continue
                    title = text_field(row.get("Title"))
                    summary = text_field(row.get("Summary"), 32000)
                    if title or summary:
                        samples.append({"kind": kind, "title": title, "text": summary,
                                        "voteups": self.count(row.get("LikeCount")), "summaryOnly": True})
                limit -= len(items)
                paging = data.get("Paging")
                if not items or not isinstance(paging, dict) or paging.get("IsEnd") is not False:
                    break
                next_offset = str(paging.get("NextOffset", ""))
                if not next_offset.isascii() or not next_offset.isdecimal():
                    break
                if len(next_offset) > 18 or int(next_offset) <= offset:
                    break
                offset = int(next_offset)
        return samples


class Handler(BaseHTTPRequestHandler):
    timeout = 10

    def log_message(self, *_args):
        return

    @property
    def oauth(self):
        return self.server.oauth

    def respond(self, status, value, headers=()):
        is_json = isinstance(value, dict)
        body = (json.dumps(value, ensure_ascii=False) if is_json else value).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8" if is_json else "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("X-Robots-Tag", "noindex")
        self.send_header("Content-Security-Policy", "default-src 'none'; frame-ancestors 'none'")
        for name, content in headers:
            self.send_header(name, content)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def do_HEAD(self):
        if self.path.partition("?")[0] in {"/", "/healthz"}:
            self.do_GET()
        else:
            self.respond(405, "请使用浏览器打开授权页面。")

    def read_json(self):
        size = int(self.headers.get("Content-Length", "0"))
        if not 0 < size <= 4096:
            raise ValueError("无效请求")
        body = json.loads(self.rfile.read(size))
        if not isinstance(body, dict):
            raise ValueError("无效请求")
        return body

    def do_GET(self):
        path, _, query = self.path.partition("?")
        if path == "/healthz":
            return self.respond(200, {"status": "ok", "oauthEnabled": self.oauth.enabled})
        if path == "/":
            return self.respond(200, "知乎答主助手 · 知乎授权服务\n")
        results = {
            "/oauth/zhihu/result/complete": (200, "知乎授权成功，可以返回桌面应用。"),
            "/oauth/zhihu/result/cancelled": (200, "已取消知乎授权，可以返回桌面应用。"),
            "/oauth/zhihu/result/invalid": (400, "无法校验本次知乎授权，请从桌面端重试。"),
            "/oauth/zhihu/result/failed": (502, "知乎授权或账号验证失败，请返回桌面应用重试。"),
            "/oauth/zhihu/result/expired": (410, "授权已过期，请从桌面端重试。"),
            "/oauth/zhihu/result/diagnostic": (200, "本次授权诊断已结束，请回到服务器终端查看结果。"),
        }
        if path in results:
            return self.respond(*results[path])
        if path not in {CALLBACK, "/oauth/zhihu/launch"}:
            return self.respond(404, "未找到此页面。")
        if not self.oauth.enabled:
            with self.oauth.lock:
                self.oauth.prune()
                if not any(login.diagnostic for login in self.oauth.logins.values()):
                    return self.respond(503, "知乎 OAuth 登录尚未启用。")
        try:
            params = parse_qs(query, max_num_fields=12, keep_blank_values=True)
            state, = params.get("state", [""])
            code, = params.get("authorization_code", [""])
        except ValueError:
            return self.result("invalid")
        if path == CALLBACK:
            return self.callback(state, code, "error" in params)
        with self.oauth.lock:
            self.oauth.prune()
            login = self.oauth.logins.get(state)
            if login is None or login.browser or login.status != "pending" or not (login.diagnostic or self.oauth.enabled):
                return self.respond(400, "授权链接已失效，请从桌面端重新发起。")
            login.browser = secrets.token_urlsafe(32)
            cookie = f"{COOKIE}_{state}={login.browser}; Path=/; Max-Age={LOGIN_TTL}; Secure; HttpOnly; SameSite=Lax"
        url = OPENAPI + "/authorize?" + urlencode({
            "redirect_uri": REDIRECT_URI, "app_id": self.oauth.app_id, "response_type": "code", "state": state,
        })
        self.respond(302, "正在打开知乎授权页面。", [("Location", url), ("Set-Cookie", cookie)])

    def callback(self, state, code, denied):
        cookies = SimpleCookie()
        try:
            cookies.load(self.headers.get("Cookie", ""))
        except CookieError:
            pass
        with self.oauth.lock:
            self.oauth.prune()
            login = self.oauth.logins.get(state)
            if not login or not self.browser_matches(state, login, cookies):
                diagnostic = [
                    (ticket, flow) for ticket, flow in self.oauth.logins.items()
                    if flow.diagnostic and self.browser_matches(ticket, flow, cookies)
                ]
                if len(diagnostic) == 1 and code:
                    ticket, flow = diagnostic[0]
                    flow.status = "state_mismatch" if state else "state_missing"
                    return self.result("diagnostic", f"{COOKIE}_{ticket}")
                return self.result("invalid")
            cookie_name = f"{COOKIE}_{state}"
            if denied or not code:
                login.status = "code_missing" if login.diagnostic and not denied else "cancelled"
                return self.result("diagnostic" if login.diagnostic else "cancelled", cookie_name)
            login.status = "exchanging"
        if login.diagnostic:
            self.diagnose_callback(login, code)
            return self.result("diagnostic", cookie_name)
        try:
            if len(code) > 4096:
                raise ValueError("无效的授权码")
            token, deadline = self.oauth.exchange(code)
            session = Session(token, deadline, self.oauth.identity(token))
        except ValueError:
            with self.oauth.lock:
                login.status = "failed"
            return self.result("failed", cookie_name)
        with self.oauth.lock:
            self.oauth.prune()
            if self.oauth.logins.get(state) is not login or session.expires <= time.time():
                return self.result("expired", cookie_name)
            if len(self.oauth.sessions) >= CAPACITY:
                login.status = "failed"
                return self.result("failed", cookie_name)
            session_key = secrets.token_urlsafe(32)
            self.oauth.sessions[session_key] = session
            login.result = {"status": "complete", "session": session_key, "user": session.user,
                            "expiresAt": session.expires}
            login.status = "complete"
        self.result("complete", cookie_name)

    @staticmethod
    def browser_matches(state, login, cookies):
        cookie = cookies.get(f"{COOKIE}_{state}")
        return bool(cookie and login.browser and login.status == "pending"
                    and cookie.value.isascii() and secrets.compare_digest(cookie.value, login.browser))

    def diagnose_callback(self, login, code):
        try:
            if len(code) > 4096:
                raise ValueError("无效的授权码")
            token, _ = self.oauth.exchange(code)
        except ValueError:
            login.status = "exchange_failed"
            return
        if not self.oauth.identity_ready:
            login.status = "identity_unconfigured"
            return
        try:
            self.oauth.identity(token)
        except ValueError:
            login.status = "identity_failed"
            return
        login.status = "verified"

    def result(self, status, cookie_name=None):
        headers = [("Location", f"/oauth/zhihu/result/{status}")]
        if cookie_name:
            headers.append(("Set-Cookie", f"{cookie_name}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax"))
        self.respond(303, "", headers)

    def do_POST(self):
        path = self.path.partition("?")[0]
        if path not in {"/oauth/zhihu/start", "/oauth/zhihu/poll", "/oauth/zhihu/cancel", "/oauth/zhihu/session",
                        "/oauth/zhihu/logout", "/oauth/zhihu/profile"}:
            return self.respond(404, {"error": "未找到接口"})
        try:
            body = self.read_json()
        except (ValueError, OSError):
            return self.respond(400, {"error": "无效请求"})
        if not self.oauth.enabled:
            message = ("知乎授权服务尚未配置用户信息接口" if not self.oauth.missing_credentials
                       else "知乎 OAuth 登录尚未启用，请联系服务管理员")
            return self.respond(503, {"error": message})
        with self.oauth.lock:
            self.oauth.prune()
            if path.endswith("/start"):
                if len(self.oauth.logins) >= CAPACITY:
                    return self.respond(503, {"error": "授权服务繁忙，请稍后重试"})
                return self.respond(200, self.oauth.start())
            if path.endswith("/poll"):
                return self.poll(body)
            if path.endswith("/cancel"):
                state, login = self.check_ticket(body)
                if login:
                    self.logins_revoke(state, login)
                return self.respond(200, {"ok": True})
            key = text_field(body.get("session"))
            session = self.oauth.sessions.get(key)
            if path.endswith("/logout"):
                self.oauth.sessions.pop(key, None)
                return self.respond(200, {"ok": True})
            if not session:
                return self.respond(401, {"error": "知乎授权已失效，请重新登录"})
        if path.endswith("/session"):
            try:
                user = self.oauth.identity(session.token)
            except AuthorizationExpired:
                with self.oauth.lock:
                    self.oauth.sessions.pop(key, None)
                return self.respond(401, {"error": "知乎授权已失效，请重新登录"})
            except ValueError:
                return self.respond(502, {"error": "知乎账号资料刷新失败"})
            with self.oauth.lock:
                if self.oauth.sessions.get(key) is not session or session.expires <= time.time() or user["id"] != session.user["id"]:
                    return self.respond(401, {"error": "知乎授权已失效，请重新登录"})
            return self.respond(200, {"user": user})
        if not self.oauth.access_secret:
            return self.respond(503, {"error": "授权服务尚未配置公开创作内容权限"})
        try:
            samples = self.oauth.profile(session)
        except ValueError:
            return self.respond(502, {"error": "知乎公开创作内容读取失败，请检查授权和接口额度"})
        with self.oauth.lock:
            if self.oauth.sessions.get(key) is not session or session.expires <= time.time():
                return self.respond(401, {"error": "知乎授权已失效，请重新登录"})
        self.respond(200, {"samples": samples})

    def check_ticket(self, body, diagnostic=False):
        state = text_field(body.get("state"))
        secret = text_field(body.get("secret"))
        login = self.oauth.logins.get(state)
        if not login or login.diagnostic != diagnostic or not secret.isascii() or not secrets.compare_digest(secret, login.secret):
            return state, None
        return state, login

    def logins_revoke(self, state, login):
        del self.oauth.logins[state]
        if login.result:
            self.oauth.sessions.pop(login.result["session"], None)

    def poll(self, body):
        state, login = self.check_ticket(body)
        if not login:
            return self.respond(200, {"status": "expired"})
        if login.status in {"complete", "cancelled", "failed"}:
            del self.oauth.logins[state]
        self.respond(200, login.result or {"status": login.status})


class DiagnosticHandler(Handler):
    def do_GET(self):
        self.respond(405, {"status": "invalid"})

    def do_HEAD(self):
        self.respond(405, {"status": "invalid"})

    def do_POST(self):
        try:
            body = self.read_json()
        except (ValueError, OSError):
            return self.respond(400, {"status": "invalid"})
        with self.oauth.lock:
            self.oauth.prune()
            if self.path == "/start":
                if self.oauth.missing_credentials:
                    return self.respond(200, {"missing": self.oauth.missing_credentials})
                if len(self.oauth.logins) >= CAPACITY or any(flow.diagnostic for flow in self.oauth.logins.values()):
                    return self.respond(200, {"status": "busy"})
                return self.respond(200, self.oauth.start(diagnostic=True))
            if self.path in {"/poll", "/cancel"}:
                state, login = self.check_ticket(body, diagnostic=True)
                if not login:
                    return self.respond(200, {"status": "expired"})
                if self.path == "/cancel" or login.status not in {"pending", "exchanging"}:
                    del self.oauth.logins[state]
                return self.respond(200, {"status": login.status})
        self.respond(404, {"status": "invalid"})


def diagnose():
    outcomes = {
        "state_missing": "知乎回传了授权码，但没有回传 state。上线前需要知乎确认如何开启 state 回传。",
        "state_mismatch": "知乎回传了授权码，但 state 与发出的值不同。请将结果反馈给知乎。",
        "code_missing": "state 校验通过，回调缺少 authorization_code。",
        "exchange_failed": "state 校验通过，但用授权码换取令牌失败。请核对 APP 凭证和登记的回调地址。",
        "identity_unconfigured": "state 校验和令牌交换均通过。请检查用户信息接口配置。",
        "identity_failed": "state 校验和令牌交换均通过。已配置的用户信息接口未返回可验证的账号资料。",
        "verified": "state 校验、令牌交换、用户身份查询均通过。现在可以从桌面端验证真实登录。",
        "cancelled": "浏览器已取消授权。",
        "expired": "未收到可确认的浏览器回调，诊断已过期。",
    }

    opener = build_opener(ProxyHandler({}))

    def request(path, payload):
        req = Request(CONTROL_URL + path, json.dumps(payload).encode(), {"Content-Type": "application/json"})
        with opener.open(req, timeout=4) as response:
            return json.load(response)

    ticket = None
    try:
        start = request("/start", {})
        if "missing" in start:
            print("容器缺少配置 " + "、".join(start["missing"]))
            return 1
        if start.get("status") == "busy":
            print("已有诊断在进行。请先在原终端结束它，或等候五分钟后重试。")
            return 1
        ticket = {key: start[key] for key in ("state", "secret")}
        print("请在五分钟内用浏览器打开此链接并完成知乎授权。", flush=True)
        print(start["url"], flush=True)
        while True:
            time.sleep(2)
            status = request("/poll", ticket)["status"]
            if status in outcomes:
                print(outcomes[status])
                return int(status not in {"verified", "identity_unconfigured"})
    except KeyboardInterrupt:
        print("\n诊断已取消。")
        return 1
    except (OSError, ValueError, KeyError):
        print("无法连接容器内诊断服务。请用更新后的部署文件重跑 ./deploy.sh。")
        return 1
    finally:
        if ticket:
            try:
                request("/cancel", ticket)
            except (OSError, ValueError):
                pass


if __name__ == "__main__":
    parser = ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--diagnose", action="store_true")
    args = parser.parse_args()
    if args.diagnose:
        raise SystemExit(diagnose())
    oauth = OAuth()
    with ThreadingHTTPServer((args.host, 33111), Handler) as server, \
            ThreadingHTTPServer(("127.0.0.1", 33112), DiagnosticHandler) as control:
        server.oauth = control.oauth = oauth
        Thread(target=control.serve_forever, daemon=True).start()
        server.serve_forever()
