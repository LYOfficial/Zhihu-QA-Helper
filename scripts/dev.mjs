#!/usr/bin/env node
/**
 * 桌面开发模式：窗口里热更新，不是浏览器。
 *
 *   1. next dev        → localhost:31873（带 HMR）
 *   2. cargo build 内核 → 确保 target/debug/zhihu-qa-core.exe 存在
 *   3. cargo run 桌面壳 → 窗口加载 http://localhost:31873
 *
 * 窗口内的界面调 API 走 http://127.0.0.1:8787（内核自己由桌面壳拉起）。
 * 只想在浏览器里调样式，用 `npm run dev:web`。
 */
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const isWin = process.platform === "win32";
const DEV_URL = "http://localhost:31873";

const procs = [];
let shuttingDown = false;

function run(name, cmd, args, cwd, color, extraEnv = {}) {
  const child = spawn(cmd, args, {
    cwd,
    shell: isWin && cmd === "npm",
    windowsHide: true,
    env: { ...process.env, FORCE_COLOR: "1", ...extraEnv },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const tag = `\x1b[${color}m[${name}]\x1b[0m `;
  const pipe = (stream) => {
    stream.setEncoding("utf8");
    stream.on("data", (d) => {
      for (const line of String(d).split(/\r?\n/)) {
        if (line.trim()) console.log(tag + line);
      }
    });
  };
  pipe(child.stdout);
  pipe(child.stderr);
  child.on("error", (error) => {
    console.error(tag + `启动失败：${error.message}`);
    if (!shuttingDown) shutdown();
  });
  child.on("exit", (code) => {
    if (shuttingDown) return;
    console.log(tag + `退出（code=${code}）`);
    shutdown();
  });
  procs.push(child);
  return child;
}

function shutdown() {
  if (shuttingDown) return;
  shuttingDown = true;
  for (const p of procs) {
    try {
      if (isWin) spawn("taskkill", ["/pid", String(p.pid), "/f", "/t"], { stdio: "ignore" });
      else p.kill("SIGTERM");
    } catch {
      /* ignore */
    }
  }
  process.exit(0);
}
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
if (isWin) process.on("SIGBREAK", shutdown);
process.on("exit", () => {
  if (!shuttingDown) shutdown();
});

async function waitFor(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const ctl = new AbortController();
      const t = setTimeout(() => ctl.abort(), 1200);
      const res = await fetch(url, { signal: ctl.signal });
      clearTimeout(t);
      if (res.ok || res.status === 404) return true;
    } catch {
      /* 还没起来 */
    }
    await new Promise((r) => setTimeout(r, 400));
  }
  return false;
}

if (!existsSync(resolve(root, "node_modules/next"))) {
  console.error("还没装依赖，先在仓库根目录执行 npm install");
  process.exit(1);
}

console.log("\n  知乎答主助手 · 桌面开发模式");
console.log("  Next dev server 起来之后会自动打开桌面窗口\n");

// 桌面壳的 tauri-build 会在编译期校验 frontendDist 目录存在。
// 全新克隆时 .build 还没生成，这里先导出一份，只做一次。
if (!existsSync(resolve(root, "apps/web/.build/index.html"))) {
  console.log("  首次运行：先导出一次前端（tauri 编译期需要该目录）…\n");
  await new Promise((res) => {
    const c = spawn(isWin ? "npm" : "npm", ["run", "build", "--workspace", "apps/web"], {
      cwd: root,
      shell: isWin,
      windowsHide: true,
      stdio: "inherit",
    });
    c.on("exit", (code) => res(code === 0));
  });
}

run("web", "npm", ["run", "dev", "--workspace", "apps/web"], root, "36");

const ready = await waitFor(DEV_URL, 90_000);
if (!ready) {
  console.error(`\n  ${DEV_URL} 在 90 秒内没有起来，请检查上面的 Next 输出\n`);
  shutdown();
}

console.log("  Next 就绪，准备内核与桌面窗口…\n");

// 内核必须先编译好：桌面壳只会去 target/debug 找现成的可执行文件
const coreOk = await new Promise((res) => {
  const c = spawn("cargo", ["build", "-p", "zhihu-qa-core"], {
    cwd: root,
    shell: false,
    windowsHide: true,
    stdio: "inherit",
  });
  c.on("exit", (code) => res(code === 0));
});
if (!coreOk) {
  console.error("  内核编译失败，桌面窗口可能拿不到数据");
}

console.log("\n  启动桌面壳（首次编译约需一两分钟）…\n");
run("app", "cargo", ["run", "-p", "zhihu-qa-desktop"], root, "35", {
  ZHIHU_QA_DEV_WEB: DEV_URL,
});
