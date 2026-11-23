# 知乎 OAuth 迁移

桌面端只通过官方 OAuth 登录。登录按钮打开系统浏览器里的知乎授权页，授权后桌面端自动确认本次会话。旧版应用内浏览器、接口扫码和手动粘贴 Cookie 登录已移除。

## 部署与凭证

保持已登记的回调地址 `https://zh.ijune.cn/oauth/zhihu/callback`。APP ID 和 APP Key 填在回调服务器的 `deploy/.env`，变量为 `ZHIHU_OAUTH_APP_ID`、`ZHIHU_OAUTH_APP_KEY`。服务器配置和启动命令见[部署说明](../deploy/README.md)。

## 授权与数据

- 服务器按官方文档使用表单提交 `app_id`、`app_key`、`grant_type=authorization_code`、`redirect_uri` 和 `code`。回调须返回与本次请求匹配的 `state` 且来自发起授权的同一浏览器。桌面端使用独立的一次性凭据领取会话，服务端保存知乎 OAuth 令牌。
- 公开文档没有列出用户信息接口的地址与字段，也没有说明会否回传 `state`。上线前需从知乎确认这些契约，完成真实 HTTPS 授权。登录状态按令牌有效期到期，用户可随时退出并重新授权。
- 使用[用户内容 API](https://developer.zhihu.com/docs?key=user_contents)时，服务同时发送开放平台 Access Secret、秒级时间戳与当前授权用户的 `X-OAuth-Token`。画像最多读取 24 条回答、16 篇文章的公开摘要，翻页跟随 `Paging.NextOffset`。桌面端设置中的个人 Access Secret 仍可用于搜索及明确绑定账号的本人内容。
- 旧版账号首次升级也需授权。账号 ID 相同时沿用本机已存的答题历史和旧版网页会话。知乎 OAuth 当前只覆盖官方开放的能力。通知同步与创作中心同步需要同一账号已有的网页会话，使用 OAuth 新登录的账号可以导入问题、主动找题、拟稿和阅读其本机已存的历史作答。
