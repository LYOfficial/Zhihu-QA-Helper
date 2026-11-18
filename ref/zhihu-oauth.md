# 知乎搜索与 OAuth 接入

## 现在可以做：搜索凭证

1. 在[知乎数据开放平台](https://developer.zhihu.com)个人中心创建 Access Secret，并确认账号开通了[知乎搜索](https://developer.zhihu.com/docs?key=zhihu_search)和[全网搜索](https://developer.zhihu.com/docs?key=global_search)。两个接口不需要 OAuth。
2. 在项目 `data/zhihu-search.env`（开发版）或 `%APPDATA%\ZhihuQAHelper\zhihu-search.env`（安装版）写入 `ZHIHU_ACCESS_SECRET=你的原始凭证`。没有文件就复制根目录的 `zhihu-search.env.example`；不加引号或 `Bearer`，写好后从托盘退出并重启。显式的数据目录 `ZHIHU_QA_DATA` 和进程级同名环境变量优先于默认目录及文件中的值。
3. 在 **`LYOfficial/Zhihu-QA-Helper`** 的 `Settings → Environments → New environment` 建 `release`，在该环境的 **Environment secrets** 新建 `ZHIHU_ACCESS_SECRET`，值同上。不是 **Repository secrets**，也不要放入 GitHub Pages 仓库。没有凭证时 Release 跳过联网检查；配置后两个接口各发一次固定查询，出错只提示，不阻断安装包构建。凭证不写进安装包或 Release 产物。

每批桌面取材/核查最多各调用一次知乎搜索和全网搜索，核查补搜可能另起批次；服务不对单次调用自动重试。公开问题的检索词会发往知乎开放平台，不发送 Cookie、草稿全文或模型密钥。通过搜索得到的链接仍走原始页面提取、去重和现有证据分级，不将搜索排名或摘要当成事实。原目录、新闻、社区和普通搜索仍并行工作。临时停用可清空本机配置并重启（若设置了同名环境变量也需清除）。

## 现在可以做：Linux 回调占位服务

在 Linux 服务器按[部署说明](../deploy/README.md)启动 Docker 容器，将 `zh.ijune.cn` 的 `A` 记录指向 HTTPS 反代服务器的公网 IPv4；反代和 Docker 同机时，上游为 `http://127.0.0.1:33111`。现有 GitHub Pages 官网 `zhihu.ijune.cn` 不变，不需为新域名创建 Pages 仓库或更改原 DNS。服务仅显示占位页，不处理授权码、不请求知乎令牌。GitHub Actions 只检查 Docker 部署，不发布到你的服务器，也不需要部署 Secret。

## 发给知乎的申请信息

按[知乎 OAuth 申请文档](https://developer.zhihu.com/docs?key=zhihu_oauth_integrated)，**授权回调地址 URL 和 `redirect_uri` 指的是同一个值**，不是两个不同的 URL：

```text
https://zh.ijune.cn/oauth/zhihu/callback
```

申请时不加尾部 `/`、`authorization_code` 或其他参数。发给 `openplatform@zhihu.com`，主题可用「知乎答主助手申请接入知乎 OAuth 服务」。包含应用名称和简介、至少 256×256 的图标附件、上述回调地址、申请人姓名、手机、知乎个人中心链接。用户画像只申请 **C. 公开内容**，不申请邮箱或手机号读取权限；申请人自行提供的联系手机号不是向用户请求的权限。

## 审批后实施（本次不启用）

1. **先确认协议**：向知乎确认是否支持并回传 `state`、是否支持 PKCE、令牌续期或撤销方式，以及身份接口与稳定标识。审批后在同一个 Linux 服务中实现动态回调，保持域名和路径不变；如果必须改 `redirect_uri`，先让知乎更新允许列表。
2. **并行接入**：把官方 OAuth 做成可关闭的登录选项。后端创建短时、一次性的授权事务并绑定发起登录的客户端；知乎回传 `authorization_code` 后校验事务及会话，后端使用 `app_id`、`app_key` 和完全一致的 `redirect_uri` 换取访问令牌。确认能可靠防止 CSRF、重放及账号串换后再开启；不能确认则保持禁用。不把密钥、授权码或访问令牌写入网页、公开包、URL 日志或桌面回跳参数。桌面端只兑换绑定本次会话的一次性凭据，并有明确的断开、过期和重新授权行为。
3. **新增画像来源**：[用户内容 API](https://developer.zhihu.com/docs?key=user_contents)用 Access Secret 与秒级时间戳鉴权。拉取已授权用户时同时传 `X-OAuth-Token`；不带该头仅代表凭证所有者，不能用作其他账号的回退。以 `answer`、`article` 对接当前样本格式，按 `Paging.NextOffset` 翻页，限制采样数量，区分摘要与正文。只处理用户明确授权的公开样本，不抓取私密内容、不上传令牌给模型。
4. **继续保留原路径**：配额耗尽、用户撤回授权、授权过期或服务不可用时，保留二维码、应用内浏览器及手动 Cookie 登录，以及与当前账号匹配的现有历史内容来源。迁移不覆盖已存画像；没有同账号凭证时提示用户选择备用方式。邀请通知等未被官方接口替代的功能不改。搜索也始终保留现有来源；不要把 OAuth 成功当作自动获取全部 API 权限。
5. **上线前验收**：验证允许/取消授权、CSRF/重放拒绝、过期/断开、两账号隔离、配额耗尽回退、URL/日志/产物无凭证，并实际通过 HTTPS 完成一次端到端登录。当前完成的是静态回调页和部署配置，不是 OAuth 登录或用户内容接入。
