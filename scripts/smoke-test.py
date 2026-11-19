import argparse
import base64
import json
import os
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections import Counter
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
QUESTION = "如何配置铁路信号？"
FIRST_DRAFT = "先看入口是否有信号，再检查区段边界。"
MANUAL_DRAFT = "手工补充：先在测试线路上验证区段边界。"


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def wait_for(check, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.08)
    raise AssertionError("等待集成检查超时")


class Agent(BaseHTTPRequestHandler):
    calls = Counter()
    review_release = threading.Event()

    def send(self, payload):
        data = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        try:
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, *args):
        pass

    def do_GET(self):
        self.send({"ok": True})

    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        stage = self.path.rsplit("/", 1)[-1]
        self.calls[stage] += 1
        if self.path == "/v1/zhihu-open/test":
            assert payload == {"zhihuAccessSecret": "local-smoke-fixture"}
            self.send({"user_contents": {"ok": True, "message": "连接成功"},
                       "zhihu_search": {"ok": True, "message": "连接成功"},
                       "global_search": {"ok": False, "message": "HTTP 429"}})
        elif stage == "collect":
            self.send({"docs": [], "plan": {}, "warnings": ["回归测试：未连接外部资料站"]})
        elif stage == "qualify":
            self.send({**payload["material"], "findings": []})
        elif stage == "draft":
            if self.calls[stage] == 1:
                self.send({"ok": False, "error": "回归测试：模拟模型暂时不可用"})
            else:
                self.send({"draft": {"markdown": FIRST_DRAFT, "doubts": [
                    {"text": "是否必须设置出口信号", "status": "unverified", "verdict": "尚未核查"},
                ], "citations": []}, "warnings": payload["material"]["warnings"]})
        elif stage == "review":
            self.review_release.wait(50)
            self.send({"doubts": [{**doubt, "verdict": "现有资料不足", "confidence": 25, "evidence": [{
                "url": "https://example.org/manual", "label": "Railroad signalling reference: operating conditions and section boundaries in the test configuration", "sourceName": "发布方", "kind": "official",
                "publishedAt": "2025-06-01", "quote": "仅适用于演示线路，不覆盖其他设置。", "stance": "context",
            }]} for doubt in payload["doubts"]]})
        elif stage == "images":
            if payload.get("supplement"):
                self.send({"images": []})
                return
            filename = "b" * 32 + ".png"
            folder = Path(payload["imagesDir"])
            folder.mkdir(parents=True, exist_ok=True)
            (folder / filename).write_bytes((ROOT / "crates/desktop/icons/128x128.png").read_bytes())
            self.send({"images": [{"filename": filename, "caption": "铁路信号示意", "autoInsert": True,
                                   "sourceName": "发布方", "sourceUrl": "https://example.org/illustrations"}]})
        elif stage == "import":
            filename = "a" * 32 + ".png"
            if filename in payload["existingImages"]:
                self.send({"images": []})
                return
            folder = Path(payload["imagesDir"])
            folder.mkdir(parents=True, exist_ok=True)
            (folder / filename).write_bytes(base64.b64decode(payload["imageData"]))
            self.send({"images": [{"filename": filename, "caption": payload["caption"],
                                   "sourceName": "手动导入", "sourceUrl": payload["sourceUrl"]}]})
        else:
            self.send({"ok": True})


class Smoke:
    def __init__(self, binary, directory, agent):
        self.binary = binary
        self.directory = directory
        self.port = free_port()
        self.agent = agent
        self.base = f"http://127.0.0.1:{self.port}"

    def request(self, path, method="GET", data=None):
        body = None if data is None else json.dumps(data).encode()
        req = urllib.request.Request(self.base + "/api" + path, method=method, data=body,
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=4) as response:
            return json.load(response)

    @contextmanager
    def core(self):
        env = {**os.environ, "ZHIHU_QA_DATA": str(self.directory), "ZHIHU_QA_PORT": str(self.port),
               "ZHIHU_QA_AGENT_PORT": str(self.agent.server_port)}
        process = subprocess.Popen([str(self.binary)], cwd=ROOT, env=env, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL)
        try:
            wait_for(lambda: self.request("/health")["agent"])
            yield process
        finally:
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)

    def seed(self):
        with sqlite3.connect(self.directory / "zhihu-qa.db") as database:
            database.execute("UPDATE settings SET auto_sync=0")
            database.execute("INSERT INTO account (id, name, cookie, zhihu_id) VALUES (1, '本机测试答主', 'fixture', 'first-account')")
            for qid, title in (("rail", QUESTION), ("other", "另一条测试问题")):
                database.execute("INSERT INTO questions (id,title,domain,tags) VALUES (?,?,?,?)",
                                 (qid, title, "测试资料", '["铁路"]'))

    def stage(self):
        return self.request("/questions/rail/research")

    def start(self):
        return self.request("/questions/rail/research", "POST", {})["jobId"]

    def draft(self):
        return self.request("/drafts/rail")

    def test_workflow(self):
        with self.core():
            self.seed()
            assert self.request("/settings")["verifyTimeoutSeconds"] == 180
            self.request("/settings", "PUT", {"verifyTimeoutSeconds": 90})
            assert self.request("/settings")["verifyTimeoutSeconds"] == 90
            self.start()
            wait_for(lambda: self.stage()["job"]["state"] == "failed")
            assert self.stage()["resumable"] and not self.stage()["draftSaved"]
            job = self.start()
            assert job == self.start(), "重复点击不能创建第二份稿子"
            wait_for(lambda: self.stage()["draftSaved"])
            timing = wait_for(lambda: self.stage().get("verification"))
            assert timing["limitSeconds"] == 90 and timing["startedMs"] > 0
            assert self.draft()["markdown"] == FIRST_DRAFT + "\n\n[IMG:" + "b" * 32 + ".png]"
            assert self.draft()["images"][0]["sourceName"] == "发布方"
            try:
                self.request("/drafts/rail/reset", "POST")
            except urllib.error.HTTPError as error:
                assert error.code == 400
            else:
                raise AssertionError("拟稿任务进行中不能清空稿件")
            self.request("/drafts/rail", "PUT", {"markdown": MANUAL_DRAFT})
        with self.core():
            assert self.request("/settings")["verifyTimeoutSeconds"] == 90
            interrupted = self.stage()
            assert interrupted["draftSaved"] and interrupted["job"]["state"] == "failed"
            assert self.draft()["markdown"] == MANUAL_DRAFT
            Agent.review_release.set()
            self.start()
            wait_for(lambda: self.stage()["job"]["state"] == "done")
            assert self.draft()["markdown"] == MANUAL_DRAFT
            assert not self.stage()["resumable"]
            assert Agent.calls["collect"] == 1 and Agent.calls["qualify"] == 1 and Agent.calls["draft"] == 2
            assert self.draft()["doubts"][0]["evidence"][0]["url"] == "https://example.org/manual"
            print("PASS API：前置核查、查证出处、自动配图、失败续跑、重启恢复及保留手工正文")

    def check_zhihu_settings(self, page, output):
        page.goto(self.base + "/settings/")
        zhihu = page.get_by_role("region", name="知乎开放平台")
        secret = zhihu.get_by_label("Access Secret 值")
        enabled = zhihu.get_by_role("checkbox", name="启用知乎 API")
        save = zhihu.get_by_role("button", name="保存", exact=True)
        test = zhihu.get_by_role("button", name="测试连接")
        credentials_path = "/settings/zhihu-open"
        initial = {"enabled": False, "configured": False, "profileEnabled": False}
        wait_for(lambda: enabled.is_enabled() and "本机测试答主" in zhihu.inner_text())
        assert self.request(credentials_path) == initial
        assert secret.get_attribute("type") == "password"
        page.route("**/api/open-url", lambda route: route.fulfill(json={"ok": True}))
        with page.expect_request("**/api/open-url") as external:
            zhihu.get_by_role("button", name="前往创建 Access Secret").click()
        assert external.value.post_data_json == {"url": "https://developer.zhihu.com/profile"}
        page.unroute("**/api/open-url")
        secret.fill("local-smoke-fixture")
        test.click()
        zhihu.get_by_text("全网搜索：失败（HTTP 429）", exact=True).wait_for()
        assert self.request(credentials_path) == initial
        enabled.check()
        save.click()
        expected = {"enabled": True, "configured": True, "profileEnabled": True}
        wait_for(lambda: self.request(credentials_path) == expected)
        page.reload()
        wait_for(lambda: enabled.is_checked() and "已保存" in (secret.get_attribute("placeholder") or ""))
        assert secret.input_value() == ""
        zhihu.screenshot(path=output / "zhihu-open-settings.png")
        with sqlite3.connect(self.directory / "zhihu-qa.db") as database:
            database.execute("UPDATE account SET zhihu_id='other-account' WHERE id=1")
        assert self.request(credentials_path) == {**expected, "profileEnabled": False}
        enabled.uncheck()
        save.click()
        wait_for(lambda: self.request(credentials_path) == {**expected, "enabled": False, "profileEnabled": False})
        test.click()
        zhihu.get_by_text("知乎搜索：连接成功", exact=True).wait_for()
        zhihu.get_by_role("button", name="移除密钥").click()
        wait_for(lambda: self.request(credentials_path) == initial)
        print("PASS 知乎开放平台设置：外链、凭证测试、保存、重载不回显、账户隔离、关闭、移除")

    def test_browser(self, output):
        from playwright.sync_api import sync_playwright

        output.mkdir(parents=True, exist_ok=True)
        with self.core(), sync_playwright() as playwright:
            browser = playwright.chromium.launch(channel="msedge", headless=True)
            page = browser.new_page(viewport={"width": 1280, "height": 820})
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.route("**/api/session", lambda route: route.fulfill(json={"loggedIn": False}))
            page.goto(self.base)
            page.get_by_role("button", name="在应用内登录知乎").click()
            retry = page.get_by_role("button", name="重新打开登录窗口")
            with page.expect_response(lambda response: response.url.endswith("/api/login/browser")) as reopened:
                retry.click()
            assert reopened.value.ok
            page.unroute("**/api/session")
            self.check_zhihu_settings(page, output)
            page.goto(self.base + "/desk/?id=rail")
            editor = page.get_by_role("textbox", name="回答正文")
            editor.wait_for()
            logo = page.locator(".app-titlebar-brand img")
            assert logo.evaluate("img => img.complete && img.naturalWidth > 0")
            assert 16 <= logo.bounding_box()["x"] <= 18
            assert editor.input_value() == MANUAL_DRAFT
            updated = MANUAL_DRAFT + "\n\n离开页面前输入的文字。"
            editor.fill(updated)
            page.reload()
            editor.wait_for()
            wait_for(lambda: editor.input_value() == updated)
            wait_for(lambda: self.draft()["markdown"] == updated)
            page.get_by_role("button", name="预览", exact=True).click()
            page.get_by_text("离开页面前输入的文字。", exact=True).wait_for()
            evidence = page.get_by_text("背景 · 发布方", exact=True)
            evidence.click()
            page.get_by_text("仅适用于演示线路，不覆盖其他设置。", exact=True).wait_for()
            assert evidence.locator("..").evaluate("element => element.scrollWidth <= element.clientWidth + 1")
            page.get_by_role("button", name="找配图", exact=True).click()
            search = page.get_by_role("button", name="自动搜图")
            search.click()
            page.get_by_role("status").filter(has_text="没有新增合适的配图").wait_for()
            page.get_by_label("导入本地图片").set_input_files(ROOT / "crates/desktop/icons/128x128.png")
            page.get_by_label("来源网页（可选）").fill("https://example.org/source")
            page.get_by_role("button", name="加入备选").click()
            page.get_by_role("status").filter(has_text="已加入 1 张备选图").wait_for()
            page.get_by_role("button", name="加入备选").click()
            page.get_by_role("status").filter(has_text="未重复添加").wait_for()
            page.screenshot(path=output / "illustration-tools.png")
            page.locator(".modal").get_by_role("button", name="关闭", exact=True).click()
            assert self.draft()["markdown"] == updated
            assets = self.request("/images/rail")
            assert len(assets) == 2 and any(asset["sourceUrl"] == "https://example.org/source" for asset in assets)
            page.screenshot(path=output / "workbench.png")
            page.set_viewport_size({"width": 820, "height": 600})
            page.screenshot(path=output / "workbench-narrow.png")
            assert 16 <= logo.bounding_box()["x"] <= 18
            assert page.locator(".page-scroll").evaluate("el => el.scrollWidth <= el.clientWidth + 1")
            page.get_by_role("combobox", name="切换问题").select_option("other")
            page.get_by_role("heading", name="另一条测试问题").wait_for()
            assert editor.input_value() == ""

            page.get_by_role("button", name="弃用", exact=True).click()
            page.get_by_role("button", name="确认弃用").click()
            wait_for(lambda: self.request("/questions/other")["status"] == "dismissed")
            page.goto(self.base + "/inbox/")
            page.get_by_role("button", name="已弃用").click()
            page.get_by_role("button", name="恢复到待解答").click()
            page.get_by_role("button", name="恢复到待解答").wait_for(state="detached")
            page.get_by_role("button", name="待阅").click()
            page.get_by_role("button", name="另一条测试问题").wait_for()
            restored = self.request("/questions/other")
            assert (restored["status"], restored["bucket"]) == ("inbox", "queue")

            page.goto(self.base + "/desk/?id=rail")
            reset = page.get_by_role("button", name="清空并重新拟稿")
            reset.click()
            page.get_by_role("button", name="取消").click()
            assert self.draft()["markdown"] == updated
            editor.fill(updated + "\n\n清空前的最新改动。")
            reset.click()
            page.get_by_role("button", name="确认清空").click()
            page.get_by_role("button", name="生成初稿").wait_for()
            assert self.request("/questions/rail")["status"] == "inbox"
            blank = self.draft()
            assert blank["markdown"] == "" and blank["wordCount"] == 0
            assert not any(blank[field] for field in ("doubts", "citations", "warnings"))
            assert blank["images"] == assets and not self.stage()["resumable"]
            page.reload()
            page.get_by_role("button", name="生成初稿").wait_for()
            assert editor.input_value() == "" and self.draft()["markdown"] == ""
            previous_job = self.stage()["job"]["id"]
            collect_count = Agent.calls["collect"]
            Agent.review_release.clear()
            try:
                page.get_by_role("button", name="生成初稿").click()
                timer = page.get_by_role("progressbar", name="本轮核查已用时间")
                timer.wait_for(timeout=20_000)
                assert timer.get_attribute("max") == "90"
                wait_for(lambda: int(timer.get_attribute("value")) >= 31, timeout=37)
                assert self.stage()["job"]["state"] == "running"
                page.screenshot(path=output / "verification-timer.png")
            finally:
                Agent.review_release.set()
            wait_for(lambda: self.stage()["job"]["id"] != previous_job and self.stage()["job"]["state"] == "done")
            assert Agent.calls["collect"] == collect_count + 1 and FIRST_DRAFT in self.draft()["markdown"]
            assert not errors, errors
            browser.close()
            print("PASS 界面：登录可重试、Logo 对齐、刷新恢复、搜图、导入去重、窄窗口、切换问题")
            print("PASS 界面：恢复弃用问题、清空重拟稿，以及补充核查超过旧 29 秒仍显示剩余时间")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--core", type=Path, default=ROOT / "target/nsis-build/release/zhihu-qa-core.exe")
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="zhihu-qa-smoke-") as name, ThreadingHTTPServer(("127.0.0.1", 0), Agent) as agent:
        threading.Thread(target=agent.serve_forever, daemon=True).start()
        try:
            smoke = Smoke(args.core.resolve(), Path(name), agent)
            smoke.test_workflow()
            if args.screenshots:
                smoke.test_browser(args.screenshots)
        finally:
            Agent.review_release.set()
            agent.shutdown()


if __name__ == "__main__":
    main()
