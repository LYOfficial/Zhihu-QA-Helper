# Linux 部署知乎 OAuth 授权服务

在运行 `zh.ijune.cn` HTTPS 回调服务的 Linux 服务器上，进入服务器上的仓库，将 `deploy/.env.example` 复制为 `deploy/.env`，并限制为部署账号可读。若只上传了 `deploy` 目录，就放在 `compose.yaml` 同目录。

```sh
cd deploy
umask 077
cp -n .env.example .env
chmod 600 .env
```

编辑该文件，填入审批获得的凭证。

```dotenv
ZHIHU_OAUTH_APP_ID='你的 APP ID'
ZHIHU_OAUTH_APP_KEY='你的 APP Key'
ZHIHU_OAUTH_USERINFO_URL=
```

| 配置项 | 用途 |
| --- | --- |
| `ZHIHU_OAUTH_APP_ID` | 知乎审批发放的 APP ID |
| `ZHIHU_OAUTH_APP_KEY` | 知乎审批发放的 APP Key。只保存在服务器 |
| `ZHIHU_OAUTH_USERINFO_URL` | 知乎确认的用户信息 HTTPS 地址，支持 `openapi.zhihu.com` 或 `developer.zhihu.com`。服务以 GET 请求并附带 `Authorization: Bearer <access_token>` |
| `ZHIHU_OAUTH_USERINFO_PATH` | 用户对象在 JSON 中的位置，默认为根对象。若包在 `data.user` 下，填 `/data/user` |
| `ZHIHU_OAUTH_USERINFO_ID_FIELD`、`ZHIHU_OAUTH_USERINFO_NAME_FIELD` | 默认为 `id`、`name`。ID 必须是知乎返回的稳定用户标识。要关联旧版历史记录，须使用与旧版账号相同的知乎用户 ID |
| `ZHIHU_OAUTH_ACCESS_SECRET` | 可选。服务端读取已授权用户公开创作时使用的开放平台 Access Secret，配合该用户的 `X-OAuth-Token` 使用 |

[知乎 OAuth 公共文档](https://developer.zhihu.com/docs?key=zhihu_oauth_integrated)列出了换取令牌的接口，用户信息的地址、返回格式与 `state` 回传行为需要向知乎确认。回调会验证知乎原样回传的 `state` 和发起授权的浏览器。仅填写 APP ID 和 APP Key 时，健康检查会显示登录尚未启用。可向 `openplatform@zhihu.com` 索取用户信息接口说明和 `state` 支持说明。

需要 Linux、Docker Engine 28 或更高版本，以及支持 `up --wait` 的 Docker Compose v2。配置完成后，从 `deploy` 目录启动：

```sh
sh deploy.sh
curl --fail https://zh.ijune.cn/healthz
```

配置齐全时健康检查返回 `{"status":"ok","oauthEnabled":true}`。它只检查服务配置，随后需在桌面端完成一次真实知乎授权。更新后重跑同一脚本，停止时运行 `docker compose down`。当前只运行单个副本，授权事务和知乎令牌保存在服务内存。服务重启后，打开桌面端「设置 → 知乎账号 → 刷新」可检查会话，失效时重新授权。

## DNS 和反向代理

1. 在 `ijune.cn` 的 DNS 控制台将主机记录 `zh` 配置为 `A`，值为 **HTTPS 反代服务器的公网 IPv4**。如果 `zh` 已有指向 `lyofficial.github.io` 的 `CNAME`，先删掉或改掉那条记录，不能让同名 `CNAME` 和 `A` 共存。只有反代已支持公网 IPv6 时才添加 `AAAA`。不要修改现有 `zhihu.ijune.cn` 记录。
2. 反代绑定 `zh.ijune.cn` 并启用 HTTPS。如果反代运行在 Docker 宿主机上，上游填 `http://127.0.0.1:33111`，原样保留请求路径。若反代自身也在容器里，容器中的 `127.0.0.1` 指向反代容器自己；可将两个容器接到同一专用网络，反代上游改为 `http://oauth:33111`。不需要将 `33111` 开放到公网。
3. 知乎登记的回调必须是 `https://zh.ijune.cn/oauth/zhihu/callback`（无尾斜杠）。反代应保留所有查询参数，并关闭 `/oauth/zhihu/` 下的访问日志或去掉查询参数、请求头和请求体，避免记录授权码或会话凭证。回调页由服务提供，不应经缓存或 CDN 改写。
