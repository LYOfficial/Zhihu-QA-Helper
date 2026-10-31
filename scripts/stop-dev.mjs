#!/usr/bin/env node
import { execFileSync } from "node:child_process";

if (process.platform !== "win32") {
  console.log("dev:stop 当前仅对 Windows 端口清理做了专门处理。");
  process.exit(0);
}

const output = execFileSync("cmd.exe", ["/d", "/s", "/c", "netstat -ano -p tcp"], { encoding: "utf8" });
const pids = new Set();
for (const line of output.split(/\r?\n/)) {
  const match = line.match(/LISTENING\s+(\d+)\s*$/i);
  if (!match) continue;
  const columns = line.trim().split(/\s+/);
  if (columns[1]?.endsWith(":31873")) pids.add(match[1]);
}

if (pids.size === 0) {
  console.log("31873 端口当前没有占用。");
  process.exit(0);
}

for (const pid of pids) {
  try {
    execFileSync("taskkill", ["/PID", pid, "/T", "/F"], { stdio: "inherit" });
  } catch {
    console.error(`无法结束占用 31873 的进程 ${pid}`);
  }
}
console.log("已释放 31873 端口。");
