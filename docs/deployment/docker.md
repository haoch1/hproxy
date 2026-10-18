# Docker 主控部署

主控镜像由 GitHub Actions 从已验证的三架构 Linux 发布包构建，支持 `linux/amd64`、`linux/arm64`、`linux/arm/v7`。容器只运行主控；镜像内的 `/opt/hproxy/release` 为只读 agent/核心分发目录，SQLite 数据放在 `/var/lib/hproxy/server` 命名卷。仓库的 [`compose.yaml`](../../compose.yaml) 默认使用非 root UID 10001、只读根文件系统、最小 Linux capabilities、本机回环端口和轮转的 Docker 日志。

| 对象 | 固定名称 |
| --- | --- |
| GitHub 仓库、Compose 项目 | `hproxy` |
| 主控服务、容器 | `hproxy-server` |
| 主控镜像 | `ghcr.io/haoch1/hproxy` |
| 主控持久卷 | `hproxy-server-data` |
| 可选容器 agent、持久卷 | `hproxy-agent`、`hproxy-agent-data` |
| agent 镜像 | `ghcr.io/haoch1/hproxy-agent` |

镜像名是 GitHub 包名，主控容器名明确带 `-server`；命令、日志与卷名统一使用上表。修改 Compose 项目名或卷名时，Docker 不会自动搬迁旧数据，需要先手动备份和迁移。

## 准备

1. Linux 主机安装 Docker Engine 与 Compose V2 插件，确认 `docker compose version` 可执行。没有 Docker 的主机可用兼容 OCI 的 Podman 运行主控镜像，但本文的 Compose 命令按 Docker 验证路径编写；Podman 兼容性见[兼容性](../compatibility.md)。
2. 域名解析到主控公网地址。反向代理处理 TLS，证书须被 agent 所在系统信任。主控设置的 `HPROXY_PUBLIC_URL` 必须是公网可达的 `https://` **源地址**，无路径、查询或尾部斜杠。
3. 反向代理至少转发普通 HTTP、`/agent/v1/connect` WebSocket、`/api/v1/logs/events` SSE 和 `/downloads/` 大文件。代理不得阻断 agent 的长连接。容器内与宿主默认统一使用 `19080`，宿主只绑定 `127.0.0.1:19080`；若与其他服务冲突，只修改 `.env` 的 `HPROXY_PORT`，Compose 会同步修改两侧端口与健康检查。若代理在其他宿主机，按网络拓扑设置 `HPROXY_BIND_ADDRESS` 并用防火墙限制来源。

```sh
git clone https://github.com/haoch1/hproxy.git
cd hproxy
cp .env.example .env
# 编辑 .env，设置真实 HPROXY_PUBLIC_URL；生产环境建议将 HPROXY_IMAGE 固定到版本标签
# 若本机 19080 已占用，修改 HPROXY_PORT；先用 ss -ltn 检查监听端口
docker compose config
docker compose pull
```

首次初始化数据库和管理员密码：

```sh
read -rsp '设置管理员密码: ' HPROXY_PASSWORD; echo
printf '%s' "$HPROXY_PASSWORD" | docker compose run --rm -T --no-deps \
  --entrypoint /usr/local/bin/hproxy-server-init hproxy-server init
unset HPROXY_PASSWORD
docker compose up -d
docker compose ps
curl -fsS http://127.0.0.1:19080/healthz
```

密码至少 12 字符。`set-password` 对已有数据库会更换密码，因此**只在首次初始化或有意改密时**执行。不要将密码写入 `.env`、镜像层、shell 命令参数或聊天记录。若 `read -s` 不受当前 shell 支持，可用其他安全方式通过标准输入传入密码。

同机 Nginx 反向代理示意（证书和站点由运营者配置）：

```nginx
server {
    listen 443 ssl http2;
    server_name panel.example.com;
    ssl_certificate /etc/letsencrypt/live/panel.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/panel.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:19080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $http_connection;
        proxy_read_timeout 3600s;
        proxy_buffering off;
    }
}
```

实际部署应让 HTTP 跳转 HTTPS，按自己的反向代理配置 WebSocket `Upgrade`/`Connection`，并检查其针对 SSE 的缓冲选项。若代理前还有 CDN，必须确认它允许长时间 WebSocket 和安装包下载。面板登录与 agent 接入均应以公网 HTTPS 地址验证，不能只用容器内 `/healthz` 代表全链路成功。

## 镜像、发布和更新

`main` 推送、每周定时任务和手动触发运行校验与构建；`main` 发布 `:edge`。`vX.Y.Z` 标签发布 GHCR `:X.Y.Z`、`:X.Y`、`:latest`，同时生成包含原生安装包的 GitHub Release。流水线使用静态 musl 构建并检查 ELF/哈希；sing-box 下载锁定上游版本和 SHA-256，Realm 从仓库的补丁源码构建。

更新前先备份数据库，再把 `.env` 的 `HPROXY_IMAGE` 改成目标版本并执行：

```sh
docker compose pull hproxy-server
docker compose up -d --no-deps hproxy-server
docker compose ps
curl -fsS http://127.0.0.1:19080/healthz
```

此操作只替换容器；命名卷中的数据库和配置不删除。agent 取得新镜像携带的发布清单后独立更新，在线 agent 可收到检查通知，离线 agent 重连后检查，正常运行时还会周期检查。镜像内没有主控进程的原地替换能力；面板“更新”按钮不用于升级 Docker 主控。若新镜像启动失败，把 `.env` 改回上一个已验证的镜像标签，再 `docker compose up -d --no-deps hproxy-server`；先检查数据库是否发生不兼容迁移，必要时按下述备份恢复。**不要使用自动拉取 latest 的无验证定时重建器**，升级应先备份、核对镜像再实施。

## 备份与恢复

在线备份由 SQLite 自身完成，避免直接复制活跃 WAL 文件。将手动备份复制到宿主机，再另行放到加密、异机存储：

```sh
mkdir -p ./backups
backup_name="manual-$(date -u +%Y%m%dT%H%M%SZ).db"
docker compose exec -T hproxy-server hproxy-server backup \
  --data /var/lib/hproxy/server \
  --output "/var/lib/hproxy/server/backups/$backup_name"
docker cp "hproxy-server:/var/lib/hproxy/server/backups/$backup_name" "./backups/$backup_name"
chmod 600 "./backups/$backup_name"
```

恢复前备份当前数据库并停止主控。下面的示例从本地 `./backups/restore.db` 恢复；使用与备份兼容的镜像版本。容器命令只操作 Compose 挂载的数据卷：

```sh
docker compose stop hproxy-server
docker compose run --rm -T --no-deps \
  -v "$PWD/backups/restore.db:/restore.db:ro" --entrypoint sh hproxy-server -ec '
    cp /restore.db /var/lib/hproxy/server/hproxy.db.restore
    rm -f /var/lib/hproxy/server/hproxy.db-wal /var/lib/hproxy/server/hproxy.db-shm
    mv /var/lib/hproxy/server/hproxy.db.restore /var/lib/hproxy/server/hproxy.db
    hproxy-server check --data /var/lib/hproxy/server
  '
docker compose up -d hproxy-server
```

如果 `check` 失败，不要启动服务；用恢复前的备份重新执行恢复。备份中包含登录信息、节点密钥和服务器身份，按密钥处理。`docker compose down` 不删除卷；`docker compose down --volumes` 会删除卷内全部主控数据，除非已经核对离机备份，否则不要执行。

## 卸载

```sh
docker compose down          # 删除容器和项目网络，保留数据卷
# 可选：确认备份后再执行 docker compose down --volumes
```

清理镜像可在确认没有其他项目使用相同标签后用 `docker image rm <具体镜像标签>`。删除容器不会触发远端 agent 卸载；应先在面板或 agent 宿主机按[agent 部署](agents.md)逐台移除。Docker 数据卷名称随 Compose 项目名变化，使用 `docker volume ls` 核对，不凭猜测删除其他卷。
