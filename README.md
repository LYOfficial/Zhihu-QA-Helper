<p align="center">
  <img src="apps/web/public/logo.svg" width="128" alt="知乎答主助手 Logo" />
</p>

<h1 align="center">知乎答主助手</h1>

<p align="center">
  <strong>让每一次回答，都像批阅奏章一样从容。</strong><br />
  面向垂直领域答主的 Windows 桌面工作台
</p>

<p align="center">
  <a href="https://github.com/LYOfficial/Zhihu-QA-Helper/releases"><img src="https://img.shields.io/github/v/release/LYOfficial/Zhihu-QA-Helper?display_name=tag&label=Release&color=1772F6" alt="Latest release" /></a>
  <a href="https://github.com/LYOfficial/Zhihu-QA-Helper/blob/main/LICENSE"><img src="https://img.shields.io/github/license/LYOfficial/Zhihu-QA-Helper?color=1772F6" alt="License" /></a>
  <a href="https://github.com/LYOfficial/Zhihu-QA-Helper/issues"><img src="https://img.shields.io/github/issues/LYOfficial/Zhihu-QA-Helper?color=1772F6" alt="Open issues" /></a>
  <a href="https://github.com/LYOfficial/Zhihu-QA-Helper/stargazers"><img src="https://img.shields.io/github/stars/LYOfficial/Zhihu-QA-Helper?style=flat&color=1772F6" alt="GitHub stars" /></a>
</p>

<p align="center">
  <a href="#快速开始">快速开始</a> ·
  <a href="#功能特性">功能特性</a> ·
  <a href="#工作流程">工作流程</a> ·
  <a href="#本地开发">本地开发</a> ·
  <a href="#检查与构建">检查与构建</a> ·
  <a href="https://github.com/LYOfficial/Zhihu-QA-Helper/issues">问题反馈</a>
</p>

---

知乎答主助手把知乎通知、领域画像、权威资料、配图和人工编辑集中到一个本地工作台中。智能体负责把问题、资料、存疑点和配图摆到案头，答主亲自批阅、改写和发布，在提升效率的同时保留最终判断权。

<p align="center">
  <img src="shots/app-home.png" width="960" alt="知乎答主助手案头界面" />
</p>

## 功能特性

- **通知同步**：使用知乎账号登录，按设定时间同步邀请回答和推荐回答；首次登录或本地问题为空时，自动补齐最近 3 天的内容。
- **领域画像**：从历史回答与文章中归纳主领域、子领域和附属领域，提供实时进度、阶段说明和可视化归纳节点。
- **智能筛选**：根据个人领域画像计算问题契合度，将内容划分为重点队列和待阅队列。
- **资料核查**：从权威平台检索资料、核对存疑点、获取配图，并生成可编辑初稿。
- **批阅台**：集中查看素材、来源与存疑点，支持 Markdown 编辑、预览、拖拽配图、自动保存和跳转知乎。
- **模型库**：自动同步 `models.dev` 开源目录，覆盖 OpenAI、Claude、DeepSeek、智谱 GLM、Kimi、通义千问和 MiniMax；网络不可用时使用内置清单。目录里每个模型的 `limit.context` / `limit.output` 会一并存入配置，拟稿时按真实上下文容量决定一次性能喂多少资料、输出上限开多大，不会再把长文截断成半截 JSON。
- **本地化体验**：提供知乎现代蓝与宣纸印章黄两套主题；数据保存在本机，Tauri 托盘常驻，关闭窗口后仍可定时值守。
- **归档清理**：已回答或弃用的问题会归档正文，并清理对应的本地图片缓存。

## 工作流程

```text
知乎登录
   ↓
同步邀请与推荐（首次补齐最近 3 天）
   ↓
领域画像与契合度筛选
   ├── 待阅队列：答主随时手动启动拟稿
   └── 重点队列：自动检索资料、核查、取图、生成初稿
                                      ↓
                              批阅台人工定稿
                                      ↓
                         复制内容 → 知乎发布 → 归档清理
```

> 智能体不会自动发布知乎回答。最终正文始终由答主人工编辑、确认并发布。

## 快速开始

1. 从 [GitHub Releases](https://github.com/LYOfficial/Zhihu-QA-Helper/releases) 下载适用于 Windows 的版本，或按照[本地开发](#本地开发)说明从源码运行。
2. 启动应用并完成知乎登录；登录异常时，可尝试应用内浏览器或默认浏览器 Cookie 兜底方式。
3. 在「设置 → 大模型」中选择平台并填写 API Key，模型清单会自动同步。
4. 等待通知同步与领域画像生成，在案头选择问题，核查资料并完成人工定稿。

正式分发面向 Windows 10/11。如果系统提示缺少 WebView2，请先安装 [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。

## 技术架构

| 层级 | 技术 | 职责 |
| --- | --- | --- |
| 桌面壳 | Tauri v2 | Windows 窗口、系统托盘与本地集成 |
| 前端 | Next.js、React、TypeScript | 界面、主题、批阅台与进度展示 |
| 本地内核 | Rust、Axum、SQLite | 本地 API、数据存储、知乎同步与任务调度 |
| 智能体 | Python 标准库 | 模型适配、资料整理、领域画像与初稿生成 |

所有服务默认只监听本机回环地址。知乎 Cookie、草稿、图片和数据库均保存在本机数据目录中。

## 本地开发

### 环境要求

- Windows 10/11
- Node.js 20 或更高版本
- Rust stable 与 MSVC Build Tools
- Python 3.10 或更高版本
- WebView2 Runtime

### 启动项目

```powershell
npm install

# 桌面开发模式：Next 热更新 + Rust 内核 + Tauri 窗口
npm run dev

# 只调前端界面
npm run dev:web

# 分别启动 Rust 内核或 Python 智能体
npm run dev:core
npm run dev:agent
```

首次启动后，在「设置 → 大模型」选择平台并填写 API Key。模型清单会自动从 `models.dev` 同步；网络不可用时可以使用内置清单。选中模型后，设置页会显示它的上下文与单次输出上限（例如 `1M tokens`），拟稿流程据此决定资料预算，避免长文被截断。

## 检查与构建

```powershell
# 前端类型检查（会先生成 Next 构建类型）
npm run typecheck

# 前端生产导出
npm run build:web

# Rust 工作区测试
cargo test --workspace

# 生成 release 内核和桌面壳
npm run build
```

<details>
<summary><strong>生成便携版</strong></summary>

便携版不需要安装器，把桌面壳和内核放在同一目录即可：

```powershell
npm run build
New-Item -ItemType Directory -Force dist\portable | Out-Null
Copy-Item target\release\zhihu-qa-helper.exe, target\release\zhihu-qa-core.exe dist\portable\ -Force
Copy-Item agents dist\portable\agents -Recurse -Force
New-Item -ItemType Directory -Force dist\portable\apps\web | Out-Null
Copy-Item apps\web\.build dist\portable\apps\web\.build -Recurse -Force
Compress-Archive -Path dist\portable\* -DestinationPath dist\zhihu-qa-helper-portable.zip -Force
```

解压后运行 `zhihu-qa-helper.exe`。运行数据会写入用户数据目录，不建议把数据库放进安装包目录。

</details>

<details>
<summary><strong>生成 NSIS 安装包</strong></summary>

先安装 Tauri CLI：

```powershell
cargo install tauri-cli --version "^2"
```

然后构建安装包：

```powershell
npm run build:web
cargo tauri build --bundles nsis
```

安装包位于 `target\release\bundle\nsis\`。

</details>

## 数据目录

```text
data/
  zhihu-qa.db       SQLite 数据库
  images/           问题配图缓存
  domains/          已回答正文归档
  logs/             Rust 内核日志
```

## 贡献与反馈

欢迎通过 [Issues](https://github.com/LYOfficial/Zhihu-QA-Helper/issues) 提交问题或功能建议，也欢迎发起 Pull Request。反馈问题时，请附上系统版本、复现步骤和必要日志，并先移除 Cookie、API Key 等敏感信息。

## 注意事项

- `models.dev` 是模型名称目录，不代表当前账号一定拥有对应模型权限；实际调用结果由所选平台 API 决定。
- 知乎接口和风控策略可能变化，登录失败时优先尝试应用内浏览器或 Cookie 兜底方式。
- 本项目是第三方工具，不代表知乎官方。

## License

本项目基于 [GNU General Public License v3.0](LICENSE) 开源。
