#!/usr/bin/env node
/**
 * 启动桌面应用。
 * 用 node 起子进程而不是在 npm 脚本里直接写 target/...：
 * npm 在 Windows 下用 cmd 执行脚本，裸路径会被当成命令名（"target 不是内部或外部命令"）。
 */
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const isWin = process.platform === "win32";
const exe = isWin ? "zhihu-qa-helper.exe" : "zhihu-qa-helper";
const release = process.argv.includes("--release");

const order = release ? ["target/release", "target/debug"] : ["target/debug", "target/release"];
const dir = order.find((d) => existsSync(resolve(root, d, exe)));

if (!dir) {
  console.error("\n  还没编译出桌面壳。先执行：npm run app\n");
  process.exit(1);
}

console.log("\n  知乎答主助手 启动中…");
console.log("  ─────────────────────────────────────────");
console.log("  · 关闭窗口只是收进托盘，每日值守继续跑");
console.log("  · 真正退出：托盘图标右键 → 退出（或在此按 Ctrl+C）");
console.log("  ─────────────────────────────────────────\n");

const child = spawn(resolve(root, dir, exe), [], { cwd: root, stdio: "inherit" });

function stop() {
  try {
    if (isWin) spawn("taskkill", ["/pid", String(child.pid), "/f", "/t"], { stdio: "ignore" });
    else child.kill("SIGTERM");
  } catch {
    /* ignore */
  }
  process.exit(0);
}
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
child.on("exit", (code) => process.exit(code ?? 0));
