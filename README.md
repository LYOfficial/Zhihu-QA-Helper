# 知乎答主助手

知乎答主助手是一个面向垂直领域答主的 Windows 桌面工具。它把知乎通知、领域画像、权威资料、配图和人工编辑集中到一个本地工作台中。

它的目标是实现**古代皇帝批阅奏章一般回答问题的体验**：智能体负责把奏章、资料、存疑点和配图摆到案头，答主亲自批阅、改写和发布。中间要求人工亲自回答的步骤，也有效防止了 AI 乱答现象的产生。

## 功能

- 使用知乎账号登录，支持应用内浏览器登录和默认浏览器 Cookie 兜底登录。
- 每天按设定时间同步邀请回答和推荐回答。
- 首次登录或本地问题为空时，自动补齐最近 3 天的邀请与推荐问题。
- 根据个人领域画像计算契合度，将问题分为重点队列和待阅队列。
- 从回答与文章中生成主领域、子领域和附属领域画像，支持实时进度、阶段说明和可视化归纳节点。
- 从权威平台检索资料、核对存疑点、获取配图并生成可编辑初稿。
- 批阅台支持素材、来源、存疑点、Markdown 编辑、预览、拖拽配图、自动保存和跳转知乎。
- 支持知乎现代蓝与宣纸印章黄两套主题，主题选择保存在本机。
- 模型库自动同步 `models.dev` 开源目录，覆盖 OpenAI、Claude、DeepSeek、智谱 GLM、Kimi、通义千问和 MiniMax；网络不可用时使用内置兜底清单。
- 已回答或弃用的问题会归档正文并清理本地图片缓存。
- Tauri 托盘常驻，关闭窗口后仍可继续执行定时值守。

## 工作流

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

## 技术架构

```text
Tauri Windows 桌面壳
  ├─ Next.js + React：界面、主题、批阅台和进度展示
  ├─ Rust + Axum：本地 API、SQLite、知乎同步、任务调度
  └─ Python 标准库智能体：模型适配、资料整理、画像和初稿
```

所有服务默认只监听本机回环地址。知乎 Cookie、草稿、图片和数据库保存在本机数据目录中。

## 环境要求

- Windows 10/11
- Node.js 20 或更高版本
- Rust stable、MSVC Build Tools
- Python 3.10 或更高版本
- WebView2 Runtime

## 本地开发

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

首次启动进入登录页。登录后，在「设置 → 大模型」选择平台并填写 API Key；模型清单会自动从 models.dev 同步，也可以在网络不可用时使用内置清单。

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

### 便携版

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

### NSIS Setup 安装包

先安装 Tauri CLI：

```powershell
cargo install tauri-cli --version "^2"
```

然后构建 NSIS 安装包：

```powershell
npm run build:web
cargo tauri build --bundles nsis
```

安装包位于：

```text
target\release\bundle\nsis\
```

## 数据目录

```text
data/
  zhihu-qa.db       SQLite 数据库
  images/            问题配图缓存
  domains/          已回答正文归档
  logs/             Rust 内核日志
```

## 注意事项

- 智能体不会自动发布知乎回答，最终正文必须由答主人工编辑和发布。
- `models.dev` 是模型名称目录，不代表当前账号一定拥有对应模型权限；实际调用仍由所选平台 API 返回结果决定。
- 知乎接口和风控策略可能变化，登录失败时优先尝试应用内浏览器或 Cookie 兜底方式。
- 本项目是第三方工具，不代表知乎官方。

## License

见 [LICENSE](LICENSE)。
