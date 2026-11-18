#!/usr/bin/env sh
set -eu

cd "$(dirname "$0")"
if ! docker compose version >/dev/null 2>&1; then
    printf '%s\n' '请先安装 Docker Engine 与 Docker Compose v2 插件。' >&2
    exit 1
fi

docker compose up --detach --build --wait --wait-timeout 60
printf '%s\n' '服务已就绪，反向代理上游：http://127.0.0.1:33111'
