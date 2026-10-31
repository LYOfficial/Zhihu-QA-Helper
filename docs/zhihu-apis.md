# 知乎接口备忘

本文件只记录**本项目实际调用**的接口，以及调用时的注意事项。
协议来源：`zly2006/zhurl` 的接口索引与知乎++ 的调用点整理（均为非官方、随时可能变化）。

> 所有请求都带 `Cookie`、`x-requested-with: fetch`、`Referer: https://www.zhihu.com/`。
> 网页端 v4 接口通常还要求 `x-zse-96` 签名，见文末。

## 登录（二维码）

三步握手，顺序不能变。**少了第 1 步，建码接口会直接返回
`400 {"error":{"code":1000,"name":"AccountRequestError"}}`** —— 这是实测踩过的坑。

| 步骤 | 请求 | 说明 |
| --- | --- | --- |
| ① 建匿名会话 | `GET https://www.zhihu.com/signin` | 响应 `Set-Cookie` 里带 `_xsrf`、`d_c0` 等，必须收进本地 jar |
| ② 预热设备 | `POST https://www.zhihu.com/udid`，body `{}` | 带上 ① 的 Cookie，并带 `x-xsrftoken: <_xsrf>` |
| ③ 建码 | `POST https://www.zhihu.com/api/v3/account/api/login/qrcode`，body `{}` | 同样带 Cookie 与 `x-xsrftoken`，返回 `token` / `link` / `expires_at` |
| ④ 轮询 | `GET .../login/qrcode/{token}/scan_info` | 必须沿用 ① 的匿名会话，否则认不出这张码 |

返回 `login_status` 为 `SCANNED`（已扫码）/ `CONFIRMED`（已确认）/ `EXPIRED`（过期）。
`CONFIRMED` 时响应里带 `cookie: "z_c0=...; Path=/; Domain=.zhihu.com"`，
解析出 `z_c0` 与已有 Cookie 合并，再调一次 `/api/v4/me` 验证登录态。

`d_c0` 是后续签名（`x-zse-96`）的必要参数，**由 `captcha/v2` 下发**（`/signin` 只给 `_xsrf` 和 `BEC`）。
没有 `d_c0` 的二维码是「无设备绑定」的：手机扫了也确认不下来，表现就是一直停在等待扫码。
不要自己伪造 `d_c0`，伪造值会让后续带签名的接口全部失败。

## 已知拦截：40352 人机验证

`GET .../scan_info` 有时会直接返回：

```json
{"error":{"need_login":true,"code":40352,
  "message":"系统监测到您的网络环境存在异常…请点击下方验证按钮进行验证。",
  "redirect":"https://www.zhihu.com/account/unhuman?..."}}
```

这是会话级风控（换 IP 无效，实测直连与代理同一 `session`）。触发后扫码流程在本机不可用。
本项目的处理：轮询状态返回 `blocked` 并把原文案交给界面，界面提示改用
「应用内浏览器登录」或「默认浏览器 + 粘贴 Cookie」，而不是继续伪装成"等待扫码"。

实现见 `crates/core/src/zhihu.rs`：
`qr_create()` 负责 ①②③，`absorb()` 维护 Cookie jar，`qr_poll()` 负责 ④。
匿名会话存在 `AppState.qr_cookie` 里，跨「建码」与「轮询」两次请求复用。

## 账号

```
GET https://www.zhihu.com/api/v4/me?include=answer_count,articles_count,follower_count
```

用于：验证登录态、取 `url_token`（后续拉历史内容要用）。

## 通知 → 邀请回答

```
GET https://api.zhihu.com/notifications/v3/timeline/entry/invite?invite_with_time_slice=1&limit=20
GET https://www.zhihu.com/api/v4/notifications/v2/default?limit=20
GET https://www.zhihu.com/api/v4/notifications/v2/follow?limit=20
GET https://www.zhihu.com/api/v4/notifications/v2/vote_thank?limit=20
```

独立的 `invite` 是移动端邀请回答时间线，使用与 `zhurl --android` 一致的移动端请求头，不做 Web 签名。
另外三个 Web 源也要拉，用于覆盖推荐回答及仍出现在旧通知中心的邀请。所有来源都跟随响应里的
`paging.next` 翻页，以 `paging.is_end` 判断结束，不再自行递增 `offset`；近三天模式遇到整页
带时间戳的条目均早于截止时间后停止。

条目结构在 Web v2 与 Android v3 间有差异，`crates/core/src/zhihu.rs::parse_notification` 做了容错：

1. 独立 `invite` 来源直接按邀请处理，其他来源看 `content.verb` / 文案是否包含邀请或推荐语义；
2. 问题对象兼容 `content.subject`、`content.target`、`target`、`targetSource`，回答对象会下钻到 `question`；
3. 同时读取字符串或数字 ID，并可从 `url` / `link` / `targetLink` 里的 `/question/<id>` 提取；
4. 时间字段兼容 `create_time`、`created_time`、`created` 等，秒和毫秒统一转成秒；
5. 邀请人兼容 `actor`、`actors[]` 与移动端 `head.author`。

宁可漏，不可错 —— 解析不出问题 id 的条目直接丢弃，不猜。请求失败、非 JSON 风控页和缺少
`data` 数组都会记录明确错误；所有来源失败时任务直接失败，不再伪装成“同步成功，读取 0 条”。

## 历史内容（画像用）

```
GET https://www.zhihu.com/api/v4/members/{url_token}/answers?include=data[*].content,excerpt,voteup_count,comment_count,question.title&limit=20&offset=0&sort_by=voteups
GET https://www.zhihu.com/api/v4/members/{url_token}/articles?include=data[*].content,excerpt,voteup_count,comment_count&limit=20&offset=0&sort_by=created
```

翻页用 `offset`，`data` 长度不足 20 即视为到底。默认各取 3 页 / 2 页，够画像用了，
再多会把 token 烧在无关的早期内容上。

## 签名 x-zse-96

网页端 v4 接口普遍要求：

```
x-zse-93: 101_3_3.0
x-zse-96: 2.0_<加密后的签名>
```

签名原文是 `101_3_3.0+{path?query}+{d_c0}`，`path?query` 为去掉域名的完整路径，含 query，不含 hash。
不能只做 `base64(md5(...))`：现行算法还会执行自定义块变换与字符表编码。本项目直接使用
`zhihu_sign = "=0.1.0"`（与 `zly2006/zhurl` 相同），实现在
`crates/core/src/zhihu.rs::send_web`。

**注意**：签名算法可能随知乎迭代失效。因此设置里给了三档：

| 模式 | 行为 |
| --- | --- |
| `auto`（默认） | 先带签名；遇 401/403/404 自动用不带签名的请求重试一次 |
| `always` | 始终带签名 |
| `off` | 从不签名 |

如果同步一直 403，先切 `off` 试试；能拉到数据说明是签名算法过期，而不是账号问题。

## 不要做的事

- 本项目**不调用**任何写接口（点赞、关注、发回答、发评论）。所有写操作都由你本人在知乎完成。
- 不做批量爬取、不遍历他人主页、不加并发。三次值守分别拉一轮通知，足够了。
