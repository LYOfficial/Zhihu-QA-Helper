# 知乎接口备忘

登录使用[知乎官方 OAuth](https://developer.zhihu.com/docs?key=zhihu_oauth_integrated)，部署和凭证配置见[授权服务](../deploy/README.md)。

以下记录旧版网页通知接口和公开内容查询。账号经 OAuth 确认后，通知接口可继续使用同一账号此前保存在本机的网页会话。旧版网页端 v4 接口可能要求 `x-zse-96` 签名，见文末。

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

## 历史作答与选题数据

| 请求 | 用途 |
| --- | --- |
| `GET https://www.zhihu.com/api/v4/creators/creations/v2/answer?start=0&end=0&limit=20&offset=0&need_co_creation=1&sort_type=created` | 从当前登录账号的创作中心读取历史回答。用 `paging.next` 中的偏移量翻页，`paging.is_end` 为真时结束。正文摘要在条目的 `data` 中，阅读、赞同、评论、收藏和喜欢在 `reaction` 中。 |
| `GET https://www.zhihu.com/api/v4/answers/{id}?include=content,content_need_truncated` | 打开历史回答时读取正文，根据 `content_need_truncated` 标注正文是否截断。 |
| `GET https://www.zhihu.com/api/v4/questions/{id}` | 导入、主动找题或刷新收件箱机会数据时读取浏览数、回答数、关注数与提问时间。 |

质效分与答题分参考 [知乎创作效率工具箱](https://github.com/kestory/zhihu-creator-userscripts)。
缺失的互动数据保留为空，返回零时按零参与计算。
打开问题收件箱或同步通知时，从已保存的原题链接自动补齐待答问题的数据。查询失败时保留上次已取得的数据，手动刷新可以立即重试。

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
- 只同步本人的历史作答。收件箱的机会数据刷新限于本机收件箱中已收录的待答问题。
