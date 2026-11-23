import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const desktop = resolve(root, "crates/desktop");
const python = process.env.ZHIHU_QA_BUILD_PYTHON || "python";
const buildDir = resolve(root, "target/nsis-build");
const artwork = resolve(buildDir, "artwork");

function run(command, args, cwd = root) {
  const result = spawnSync(command, args, {
    cwd, stdio: "inherit", windowsHide: true, env: { ...process.env, CARGO_TARGET_DIR: buildDir },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

if (process.platform !== "win32" || process.arch !== "x64") {
  throw new Error("请使用 Windows x64、x64 Node.js 与 x64 Python 构建此安装包。");
}
run(python, ["-c", "import struct; import PyInstaller, PIL; assert struct.calcsize('P') == 8"]);
run(python, ["scripts/generate-nsis-artwork.py", artwork]);
run("powershell.exe", ["-NoProfile", "-File", "scripts/build-nsis-skin.ps1", artwork]);
run(process.execPath, ["../../node_modules/next/dist/bin/next", "build"], resolve(root, "apps/web"));
run("cargo", ["build", "--locked", "--release", "-p", "zhihu-qa-core"]);
run(python, ["-m", "PyInstaller", "--noconfirm", "--onedir", "--exclude-module", "numpy",
  "--name", "zhihu-qa-agent", "--distpath", "target/agent-dist", "--workpath", "target/agent-work",
  "--specpath", "target", "agents/main.py"]);
const resources = {
  [relative(desktop, resolve(buildDir, "release/zhihu-qa-core.exe")).replaceAll("\\", "/")]: "runtime/zhihu-qa-core.exe",
  "../../target/agent-dist/zhihu-qa-agent/": "runtime/agent/",
  "../../apps/web/.build/": "runtime/web/",
};
for (const resource of Object.keys(resources)) {
  if (!existsSync(resolve(desktop, resource))) throw new Error(`缺少安装资源：${resource}`);
}
const nsis = {
  template: "installer/installer.nsi",
  installerHooks: "installer/appearance.nsh",
  headerImage: relative(desktop, resolve(artwork, "installer-header.bmp")).replaceAll("\\", "/"),
};
run(process.execPath, [resolve(root, "node_modules/@tauri-apps/cli/tauri.js"),
  "build", "--ci", "--features", "installed-runtime", "--bundles", "nsis", "--config",
  JSON.stringify({ bundle: { resources, windows: { nsis } } })], desktop);
