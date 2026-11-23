import argparse
import io
import json
import os
import sqlite3
import subprocess
import tempfile
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from PIL import Image, ImageDraw
from playwright.sync_api import sync_playwright
from importlib import import_module

smoke = import_module("smoke-test")
BASE = "http://127.0.0.1:18787"
VERSION = json.loads((Path(__file__).resolve().parents[1] / "crates/desktop/tauri.conf.json").read_text(encoding="utf-8"))["version"]


class LegacyCore(BaseHTTPRequestHandler):
    def do_GET(self):
        response = "内核已启动，但还没找到前端产物。".encode()
        self.send_response(200)
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, *args):
        pass


def request(path, data=None):
    encoded = None if data is None else json.dumps(data).encode()
    query = urllib.request.Request(BASE + path, data=encoded, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(query, timeout=30 if encoded else 3) as response:
        return json.load(response)


def test_nsis(extracted, screenshots, live_search, live_sources):
    desktop = extracted / "zhihu-qa-helper.exe"
    assert desktop.is_file() and (extracted / "runtime/web/index.html").is_file()
    old = None
    try:
        old = ThreadingHTTPServer(("127.0.0.1", 8787), LegacyCore)
    except OSError:
        pass
    if old:
        threading.Thread(target=old.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="zhihu-qa-installed-", ignore_cleanup_errors=True) as name:
            root = Path(name)
            debug_port = smoke.free_port()
            data = root / "data"
            env = {**os.environ, "ZHIHU_QA_DATA": str(data), "ZHIHU_QA_PORT": "8787",
                   "WEBVIEW2_USER_DATA_FOLDER": str(root / "webview"),
                   "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS": f"--remote-debugging-port={debug_port}"}
            process = subprocess.Popen([str(desktop)], cwd=extracted, env=env)
            try:
                health = smoke.wait_for(lambda: (value if (value := request("/api/health"))["agent"] else None), 35)
                assert health["version"] == VERSION and Path(health["dataDir"]) == data, health
                with sqlite3.connect(data / "zhihu-qa.db") as db:
                    db.execute("UPDATE settings SET auto_sync=0")
                    db.execute("""INSERT INTO account(id,name,zhihu_id,cookie,oauth_session,oauth_expires_at)
                               VALUES (1,'离线测试账号','fixture-user','fixture','fixture-session',unixepoch()+3600)""")
                    db.execute("INSERT INTO onboarding_finished VALUES ('fixture-user')")
                    db.execute("INSERT INTO questions(id,title,domain) VALUES ('test','Steam locomotive','铁路')")
                    db.execute("INSERT INTO drafts(question_id,markdown) VALUES ('test',?)", (smoke.MANUAL_DRAFT,))
                    if live_sources:
                        citation = {"url": "https://www.minecraft.net/en-us/article/twitchcon-gamedrop-features",
                                    "label": "Minecraft TwitchCon 公告", "sourceName": "Minecraft 官网", "snippet": "官方公告"}
                        db.execute("UPDATE drafts SET citations=? WHERE question_id='test'", (json.dumps([citation]),))
                screenshots.mkdir(parents=True, exist_ok=True)
                with sync_playwright() as playwright:
                    debug_url = f"http://127.0.0.1:{debug_port}"
                    def webview_ready():
                        with urllib.request.urlopen(debug_url + "/json/list", timeout=2) as response:
                            return any(target.get("type") == "page" and target.get("url", "").startswith(BASE)
                                       for target in json.load(response))
                    smoke.wait_for(webview_ready, 18)
                    browser = playwright.chromium.connect_over_cdp(debug_url)
                    cdp = browser.new_browser_cdp_session()
                    def attached_page():
                        cdp.send("Target.getTargets")
                        return next((page for context in browser.contexts for page in context.pages if page.url.startswith(BASE)), None)
                    page = smoke.wait_for(attached_page, 12)
                    page.goto(BASE + "/desk/?id=test")
                    editor = page.get_by_role("textbox", name="回答正文")
                    editor.wait_for()
                    smoke.wait_for(lambda: editor.input_value() == smoke.MANUAL_DRAFT)
                    if live_sources:
                        page.get_by_role("button", name="从来源提图", exact=True).click()
                        page.get_by_role("status").filter(has_text="正文未改动").wait_for(timeout=35_000)
                        official = request("/api/images/test")
                        assert len(official) >= 3 and all(asset["sourceUrl"] == citation["url"] for asset in official), official
                        page.screenshot(path=screenshots / "nsis-source-images.png")
                        page.locator(".modal").get_by_role("button", name="关闭", exact=True).click()
                        print(f"PASS 来源提图：安装版从 Minecraft 官网抓到 {len(official)} 张图，保留原文出处")
                    page.get_by_role("button", name="找配图", exact=True).click()
                    if live_search:
                        page.get_by_role("button", name="自动搜图").click()
                        status = page.get_by_role("status")
                        status.filter(has_text="正文未改动").wait_for(timeout=30_000)
                        assets = request("/api/images/test")
                        print("PASS 真实搜图：", [(asset["sourceName"], asset["sourceUrl"]) for asset in assets])
                    image = Image.new("RGB", (360, 280), "#f2eee3")
                    draw = ImageDraw.Draw(image)
                    draw.ellipse((50, 22, 210, 216), fill="#347765")
                    draw.line((55, 250, 324, 44), fill="#3d72b1", width=19)
                    buffer = io.BytesIO()
                    image.save(buffer, format="PNG")
                    page.get_by_label("导入本地图片").set_input_files({"name": "测试图.png", "mimeType": "image/png", "buffer": buffer.getvalue()})
                    page.get_by_label("来源网页（可选）").fill("https://example.org/source")
                    page.get_by_role("button", name="加入备选").click()
                    page.get_by_role("status").filter(has_text="已加入 1 张备选图").wait_for()
                    page.get_by_role("button", name="加入备选").click()
                    page.get_by_role("status").filter(has_text="未重复添加").wait_for()
                    page.screenshot(path=screenshots / "nsis-illustration.png")
                    page.locator(".modal").get_by_role("button", name="关闭", exact=True).click()
                    page.screenshot(path=screenshots / "nsis-workbench.png")
                    assert editor.input_value() == smoke.MANUAL_DRAFT
                    draft = request("/api/drafts/test")
                    assert draft["markdown"] == smoke.MANUAL_DRAFT and draft["images"][-1]["sourceUrl"] == "https://example.org/source"
                    assert request("/api/questions/test")["status"] == "inbox"
                    print("PASS 安装包解出的真实桌面：旧 8787 不干扰、加载安装版页面、智能体就绪、补图且不改正文")
            except Exception:
                for log in sorted((data / "logs").glob("*.log")):
                    print(log.name, log.read_text(encoding="utf-8", errors="replace")[-3500:])
                raise
            finally:
                process.terminate()
                process.wait(timeout=5)
                def core_exited():
                    try:
                        request("/api/health")
                    except OSError:
                        return True
                    return False
                smoke.wait_for(core_exited, 10)
    finally:
        if old:
            old.shutdown()
            old.server_close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("extracted", type=Path)
    parser.add_argument("--screenshots", type=Path, required=True)
    parser.add_argument("--live-search", action="store_true")
    parser.add_argument("--live-sources", action="store_true")
    args = parser.parse_args()
    test_nsis(args.extracted.resolve(), args.screenshots.resolve(), args.live_search, args.live_sources)
