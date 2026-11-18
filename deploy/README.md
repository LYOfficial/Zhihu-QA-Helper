# Linux：部署知乎 OAuth 回调占位服务

需要 Linux、Docker Engine 28 或更高版本、支持 `docker compose up --wait` 的 Docker Compose v2 插件，以及可以运行 `docker` 的账号。无需在宿主机安装 Python，也不需要任何知乎凭证。将本目录复制到服务器，或在服务器拉取仓库后运行：

```sh
sh deploy/deploy.sh
curl --fail http://127.0.0.1:33111/healthz
```

命令从仓库根目录运行；只复制了 `deploy` 目录时，从该目录执行 `sh deploy.sh`。健康检查应返回 `{"status":"ok","oauthEnabled":false}`。更新代码后重跑同一脚本即可，容器会随 Docker 重启自动恢复。停止服务使用 `docker compose -f deploy/compose.yaml down`。

## DNS 和反向代理

1. 在 `ijune.cn` 的 DNS 控制台添加 `A` 记录：主机记录 `zh`，值为 **HTTPS 反代服务器的公网 IPv4**。只有反代已支持公网 IPv6 时才添加 `AAAA`。不要修改现有 `zhihu.ijune.cn` 记录。
2. 反代绑定 `zh.ijune.cn` 并启用 HTTPS。如果反代运行在 Docker 宿主机上，上游填 `http://127.0.0.1:33111`，原样保留请求路径。若反代自身也在容器里，容器中的 `127.0.0.1` 指向反代容器自己；可将两个容器接到同一专用网络，反代上游改为 `http://oauth:33111`。不需要将 `33111` 开放到公网。
3. 用浏览器验证 `https://zh.ijune.cn/healthz` 和 `https://zh.ijune.cn/oauth/zhihu/callback`，确认 HTTPS 有效且回调说明显示登录未启用。提交给知乎的地址是 `https://zh.ijune.cn/oauth/zhihu/callback`（不加尾斜杠）。本服务的回调不读取查询参数，也不记录请求 URL；正式接入前还要关闭或脱敏反代对回调查询参数的日志。此时不要用真实授权码测试。

GitHub 的 `Check Linux OAuth service` 工作流仅在测试机器上执行部署脚本、验证健康检查和回调页，不接触正式服务器，不需要部署凭证，也不创建 Release 或标签。
