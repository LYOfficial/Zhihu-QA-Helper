from argparse import ArgumentParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


CALLBACK = "/oauth/zhihu/callback"


class Handler(BaseHTTPRequestHandler):
    timeout = 5

    def log_message(self, *_args):
        return

    def do_GET(self):
        path = self.path.partition("?")[0]
        status = 200
        content_type = "text/plain; charset=utf-8"
        if path == "/healthz":
            content_type = "application/json"
            text = '{"status":"ok","oauthEnabled":false}\n'
        elif path == "/":
            text = "知乎答主助手 · 授权服务\n知乎 OAuth 登录尚未启用。\n"
        elif path in {CALLBACK, CALLBACK + "/"}:
            text = "知乎 OAuth 回调预留地址。登录尚未启用，本服务不会处理授权码或交换令牌。\n"
        else:
            status = 404
            text = "未找到此页面。\n"
        body = text.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("X-Robots-Tag", "noindex")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    do_HEAD = do_GET


if __name__ == "__main__":
    parser = ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()
    with ThreadingHTTPServer((args.host, 33111), Handler) as server:
        server.serve_forever()
