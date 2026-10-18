# hproxy

面向自有 Linux 服务器的代理节点与端口转发控制台。Rust 主控提供 Web 面板、SQLite 存储和配置下发；轻量 agent 主动通过 HTTPS/WebSocket 连接主控，在子机运行项目管理的 sing-box 与补丁版 Realm。主控推荐使用 Docker Compose 部署，agent 推荐在宿主 Linux 或 LXC 内原生安装。

> 适用范围：管理自己有权使用的服务器和网络。发布版本支持 Linux `amd64`、`arm64`、`armv7`；浏览器访问面板需要 HTTPS 公网入口。尚未验证的系统或网络环境见[兼容性与限制](#兼容性与部署边界)。

## 功能

- VLESS Reality + Vision、Shadowsocks 2022 节点；TCP、UDP、TCP+UDP 端口转发。Realm 使用本仓库的计数与动态 DNS 补丁，不调用宿主机已有的 Realm。
- 节点和转发独立计量入站、出站流量；额度支持 K/M/G/T 与 KB/MB/GB/TB 输入，按 1024 换算；流量周期支持月、季、半年、年、自定义天数和一次性额度。
- IPv4/IPv6 双栈、不同地址族间的转发、服务器地址变更同步与动态域名重新解析。手动地址保持原值；详情见[服务器与流量规则](#服务器与流量规则)。
- 一次性接入令牌、离线队列、配置恢复、累计流量保护、健康检查、日志保留和手动备份。
- 主控与 agent 的版本化更新。Docker 主控由镜像更新；原生 agent 根据主控提供的兼容发布包更新，失败时保留当前可用版本。

## 架构

```text
浏览器 ──HTTPS──> 反向代理 ──HTTP(本机)──> hproxy 主控容器
                                               │ SQLite 持久卷
                                               │ HTTPS/WSS + 安装包
                                               ▼
                                            Linux agent
                                               ├─ sing-box
                                               └─ Realm（项目补丁版）
```

agent 主动连接主控，不要求为管理通道开放子机入站端口。节点和转发的业务端口仍需在服务器、云防火墙和 NAT 映射中放行。主控镜像内含三个 Linux 架构的 agent 和核心安装包；子机原有 sing-box/Realm 不会被覆盖或作为本项目运行核心。

## 快速部署

准备 Linux 主机、Docker Engine 与 Compose 插件、一个指向该主机的域名，以及能转发 WebSocket 的 HTTPS 反向代理。主控容器内与宿主机使用同一个 `19080` 端口，宿主默认只绑定 `127.0.0.1:19080`；如被占用，在 `.env` 修改一次 `HPROXY_PORT`，两侧会同步。

```sh
git clone https://github.com/haoch1/hproxy.git
cd hproxy
cp .env.example .env
# 编辑 .env：把 HPROXY_PUBLIC_URL 改为真实的 https:// 面板域名
docker compose pull
read -rsp '设置管理员密码: ' HPROXY_PASSWORD; echo
printf '%s' "$HPROXY_PASSWORD" | docker compose run --rm -T --no-deps \
  --entrypoint /usr/local/bin/hproxy-server-init hproxy-server init
unset HPROXY_PASSWORD
docker compose up -d
docker compose ps
```

密码至少 12 字符。上面的初始化命令把密码从标准输入交给容器，不放进命令参数或 `.env`。在反向代理上配置 HTTPS 后，访问 `.env` 中的地址，打开“服务器 → 添加服务器”，在目标 Linux 主机执行面板给出的一次性安装命令。**不要把令牌、管理员密码或完整安装命令提交到仓库、工单或公开日志。**

`latest` 对应正式版本标签；`edge` 跟随 `main`，供测试使用。建议生产环境在 `.env` 的 `HPROXY_IMAGE` 指定明确版本，例如 `ghcr.io/haoch1/hproxy:0.1.0`，并在升级前备份。镜像由 [GitHub Actions](.github/workflows/ci-release.yml) 自动构建和发布；首次下载若被仓库包权限阻止，先确认对应 GHCR 包已设为公开。

以下各节包含完整部署、运维、兼容性、更新回滚、数据位置和验证说明。

## 本地开发与验证

源码需要 Rust 1.90+、Node.js 22 和 npm。Realm 位于独立的 `vendor/realm` Cargo 工作区。

```sh
npm ci --prefix web
npm run build --prefix web
cargo test --workspace --locked
python3 -m unittest discover -s tests
cargo test --manifest-path vendor/realm/Cargo.toml \
  -p realm_core --no-default-features --features batched-udp --lib --locked
```

本机预览通过 `hproxy-server run --data ./data` 监听 `127.0.0.1:19080`；`/demo/` 使用浏览器本地示例数据，不连接真实 agent。Linux 真机验证记录与未覆盖项目见下文“发版前验证”。

## 内容导航

- [Docker 主控部署](#docker-主控部署)
- [子 agent 部署](#子-agent-部署)
- [运维与故障恢复](#运维与故障恢复)
- [服务器与流量规则](#服务器与流量规则)
- [兼容性与部署边界](#兼容性与部署边界)
- [版本更新](#版本更新)
- [文件与维护](#文件与维护)
- [发版前验证](#发版前验证)
- [前端视觉与交互验收](#前端视觉与交互验收)
- [hproxy 界面基准](#hproxy-界面基准)
- [许可与第三方来源](#许可与第三方来源)

## Docker 主控部署

主控镜像由 GitHub Actions 从已验证的三架构 Linux 发布包构建，支持 `linux/amd64`、`linux/arm64`、`linux/arm/v7`。容器只运行主控；镜像内的 `/opt/hproxy/release` 为只读 agent/核心分发目录，SQLite 数据放在 `/var/lib/hproxy/server` 命名卷。仓库的 [`compose.yaml`](compose.yaml) 默认使用非 root UID 10001、只读根文件系统、最小 Linux capabilities、本机回环端口和轮转的 Docker 日志。

| 对象 | 固定名称 |
| --- | --- |
| GitHub 仓库、Compose 项目 | `hproxy` |
| 主控服务、容器 | `hproxy-server` |
| 主控镜像 | `ghcr.io/haoch1/hproxy` |
| 主控持久卷 | `hproxy-server-data` |
| 可选容器 agent、持久卷 | `hproxy-agent`、`hproxy-agent-data` |
| agent 镜像 | `ghcr.io/haoch1/hproxy-agent` |

镜像名是 GitHub 包名，主控容器名明确带 `-server`；命令、日志与卷名统一使用上表。修改 Compose 项目名或卷名时，Docker 不会自动搬迁旧数据，需要先手动备份和迁移。

### 准备

1. Linux 主机安装 Docker Engine 与 Compose V2 插件，确认 `docker compose version` 可执行。没有 Docker 的主机可用兼容 OCI 的 Podman 运行主控镜像，但本文的 Compose 命令按 Docker 验证路径编写；Podman 兼容性见[兼容性](#兼容性与部署边界)。
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

### 镜像、发布和更新

`main` 推送、每周定时任务和手动触发运行校验与构建；`main` 发布 `:edge`。`vX.Y.Z` 标签发布 GHCR `:X.Y.Z`、`:X.Y`、`:latest`，同时生成包含原生安装包的 GitHub Release。流水线使用静态 musl 构建并检查 ELF/哈希；sing-box 下载锁定上游版本和 SHA-256，Realm 从仓库的补丁源码构建。

更新前先备份数据库，再把 `.env` 的 `HPROXY_IMAGE` 改成目标版本并执行：

```sh
docker compose pull hproxy-server
docker compose up -d --no-deps hproxy-server
docker compose ps
curl -fsS http://127.0.0.1:19080/healthz
```

此操作只替换容器；命名卷中的数据库和配置不删除。agent 取得新镜像携带的发布清单后独立更新，在线 agent 可收到检查通知，离线 agent 重连后检查，正常运行时还会周期检查。镜像内没有主控进程的原地替换能力；面板“更新”按钮不用于升级 Docker 主控。若新镜像启动失败，把 `.env` 改回上一个已验证的镜像标签，再 `docker compose up -d --no-deps hproxy-server`；先检查数据库是否发生不兼容迁移，必要时按下述备份恢复。**不要使用自动拉取 latest 的无验证定时重建器**，升级应先备份、核对镜像再实施。

### 备份与恢复

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

### 卸载

```sh
docker compose down          # 删除容器和项目网络，保留数据卷
# 可选：确认备份后再执行 docker compose down --volumes
```

清理镜像可在确认没有其他项目使用相同标签后用 `docker image rm <具体镜像标签>`。删除容器不会触发远端 agent 卸载；应先在面板或 agent 宿主机按[agent 部署](#子-agent-部署)逐台移除。Docker 数据卷名称随 Compose 项目名变化，使用 `docker volume ls` 核对，不凭猜测删除其他卷。

## 子 agent 部署

agent 应安装在承载节点或转发端口的 **Linux 网络命名空间** 内。Debian、Ubuntu、CentOS/RHEL 衍生版、Alpine、LXC 和 NAT VPS 优先使用原生安装脚本；脚本自动识别 `amd64`、`arm64`、`armv7`，下载项目发布清单中的二进制，校验 SHA-256 后运行。systemd、OpenRC 或无 init 环境使用各自的启动方式。宿主机现有 sing-box/Realm 与项目核心独立，项目不会改写系统包。

### 原生安装

在面板创建服务器，取得一次性安装令牌（10 分钟有效）。在**目标服务器**执行；把示例域名改成主控实际 HTTPS 地址。令牌从终端读取并写入权限 0600 的临时文件，不进入进程参数：

```sh
umask 077
installer=$(mktemp)
token_file=$(mktemp)
trap 'rm -f "$installer" "$token_file"' EXIT
curl -fsS https://panel.example.com/install.sh -o "$installer"
read -rsp '输入面板中的一次性接入令牌: ' HPROXY_TOKEN; echo
printf '%s' "$HPROXY_TOKEN" > "$token_file"
unset HPROXY_TOKEN
sudo sh "$installer" -e https://panel.example.com --token-file "$token_file"
```

没有 `curl` 时可用 `wget -qO "$installer" https://.../install.sh`；安装器自身要求 `curl` 或 `wget` 以及 `sha256sum`。在 root shell 里去掉 `sudo`。使用私有 CA 时同时添加 `--ca /path/to/ca.crt`，先在系统内安全分发证书；不要用跳过证书校验的选项。普通用户安装追加 `--user` 并以该用户运行安装器，但低于 1024 的业务端口与部分容器网络权限可能不可用。

安装完成后在面板确认 agent 在线，检查目标机器的核心状态与测试节点/转发。脚本只下载 agent 核心可执行文件；用到的 sing-box/Realm 核心由 agent 按资源需求安装。业务端口需在防火墙、VPS 控制台和 NAT 映射里分别放行 TCP/UDP。IPv6 能否公网访问取决于运营商路由与宿主网络，不能仅凭面板显示 IPv6 判断。

安装目录与持久化目录：

| 安装方式 | 程序与核心 | 身份、配置、流量、日志 |
| --- | --- | --- |
| 系统安装 | `/opt/hproxy/agent` | `/var/lib/hproxy/agent` |
| 普通用户 `--user` | `~/.local/share/hproxy/agent` | `~/.local/state/hproxy/agent` |

同一服务器重新绑定时，不要直接覆盖现有身份。先确认旧 agent 已停止，再用新一次性令牌追加 `--replace`；同一主控和服务器 ID 的更换会保留累计计数与周期计量点，其他绑定保留独立备份。详情见[文件与维护](#文件与维护)。

### 主控失联时的本地指令

安装器会创建 `/opt/hproxy/agent/agentctl`。不依赖面板即可执行：

```sh
sudo sh /opt/hproxy/agent/agentctl status
sudo sh /opt/hproxy/agent/agentctl check
sudo sh /opt/hproxy/agent/agentctl logs
sudo sh /opt/hproxy/agent/agentctl restart
sudo sh /opt/hproxy/agent/agentctl update
sudo sh /opt/hproxy/agent/agentctl stop
sudo sh /opt/hproxy/agent/agentctl start
sudo sh /opt/hproxy/agent/agentctl uninstall
```

`update` 重启 agent 并触发它在约 60 秒后检查主控提供的兼容发布包；主控不可达时继续使用本地版本。程序或网络故障时可用 `status`、`logs`、`check` 定位。普通用户安装把路径换成 `~/.local/share/hproxy/agent/agentctl`，**以安装时的同一用户**执行，不加 `sudo`。卸载默认保留身份与统计数据，可在核对备份后显式 `uninstall --purge-data`。面板删除原生服务器会在 agent 在线时远程卸载程序、核心和数据；离线任务等待重连。系统级日志与本机其他程序不参与删除。

旧安装版本若没有 `agentctl`，可先从主控 HTTPS 站点获取 `https://panel.example.com/agentctl.sh` 与 `uninstall-agent.sh`，确认脚本内容后放入现有安装目录，或按[版本更新](#版本更新)升级。**不要**在无法验证目录归属时手动执行递归删除。

### Docker/Podman 容器 agent（可选）

当宿主没有适合的 init、但可运行 OCI 容器时，可使用 `ghcr.io/haoch1/hproxy-agent:<版本>`。业务端口由 sing-box/Realm 在容器里监听，因此需要 `--network host`；桥接网络必须自行明确发布每个 TCP/UDP 端口，且可能无法处理动态端口/双栈。持久卷保存 agent 身份、核心、配置、离线队列和流量计数。**容器生命周期由 Docker/Podman 管理，面板不能远程卸载容器 agent，面板也不会替它拉取新镜像。**

以下示例使用 Docker；Podman 可在目标环境验证 host network 和卷所有权后用 `podman` 替换命令。首次下载应使用与主控兼容的相同版本：

```sh
AGENT_IMAGE=ghcr.io/haoch1/hproxy-agent:0.1.0
docker volume create hproxy-agent-data
read -rsp '输入一次性接入令牌: ' HPROXY_TOKEN; echo
printf '%s' "$HPROXY_TOKEN" | docker run --rm -i --network host \
  -v hproxy-agent-data:/var/lib/hproxy/agent \
  --entrypoint /usr/local/bin/hproxy-agent-enroll "$AGENT_IMAGE" \
  https://panel.example.com
unset HPROXY_TOKEN
docker run -d --name hproxy-agent --network host --restart unless-stopped \
  -v hproxy-agent-data:/var/lib/hproxy/agent "$AGENT_IMAGE"
docker logs --tail 100 hproxy-agent
```

容器镜像内的进程以 UID 10001 运行；挂载现有宿主目录时需授予该 UID 写权限，不要为解决权限问题开放整个宿主文件系统。rootless Podman 的低端口绑定、host network 和用户命名空间受宿主策略限制，不能保证与 rootful Docker 等价。LXC 内使用 Podman/Docker 还依赖宿主启用 nesting、cgroup、overlay 等能力；如果不满足，直接在 LXC 内使用原生安装。

Podman 可按相同的卷和镜像名称单独部署，先用 `podman volume create hproxy-agent-data`，把上面首次接入命令中的 `docker run` 换成 `podman run`。长期运行建议交给宿主的 systemd/Quadlet 管理，而非仅依赖一次交互式启动。下面是**接入完成后**可放在 `~/.config/containers/systemd/hproxy-agent.container` 的 rootless 示例；实际 `host` 网络和卷权限应先在目标机验证：

```ini
[Unit]
Description=hproxy agent
After=network-online.target
Wants=network-online.target

[Container]
Image=ghcr.io/haoch1/hproxy-agent:0.1.0
ContainerName=hproxy-agent
Network=host
Volume=hproxy-agent-data:/var/lib/hproxy/agent

[Service]
Restart=always

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
systemctl --user start hproxy-agent.service
systemctl --user status hproxy-agent.service
journalctl --user -u hproxy-agent.service -n 100 --no-pager
```

要让 rootless 用户服务在注销后继续运行，需由宿主管理员启用该用户的 linger；没有 user systemd 的 Podman 环境用宿主提供的服务管理器启动容器。Quadlet 更新是修改 `Image=` 到已验证版本，`podman pull` 后重新加载并重启服务；卸载先停止/禁用服务，再移除 `.container` 文件和容器，最后按是否保留身份决定是否删除卷。

容器 agent 更新与回退由宿主执行。更新前保留原镜像标签与卷备份；只更换进程，不能删除持久卷：

```sh
docker pull ghcr.io/haoch1/hproxy-agent:<新版本>
docker stop hproxy-agent && docker rm hproxy-agent
docker run -d --name hproxy-agent --network host --restart unless-stopped \
  -v hproxy-agent-data:/var/lib/hproxy/agent ghcr.io/haoch1/hproxy-agent:<新版本>
docker logs --tail 100 hproxy-agent
```

如需回退，用旧镜像标签重复最后三步；确保新版本没有引入不兼容的数据格式。卸载时先在目标宿主机停止并删除容器；默认保留卷，明确决定清除身份和统计后才删除卷：

```sh
docker stop hproxy-agent && docker rm hproxy-agent
# 仅在确认不再使用身份与流量数据后：docker volume rm hproxy-agent-data
```

容器移除后，面板中的服务器记录不会自动清理。至少等待 60 秒让主控确认离线，再在面板使用“本地已卸载，移除记录”；此操作只删除面板记录和下发配置，保留历史流量，不会远程删除文件。若主控暂时不可达，先保留卷和记录，恢复后再处理。

## 运维与故障恢复

主控与 agent 有各自的数据和生命周期。主控容器失联不会立即清空 agent 已下发的配置；agent 继续使用本地配置和持久化流量计数，连接日志写入有上限的离线队列。恢复连接后补报。管理操作需要主控可用；应首先检查健康状态、证书、DNS 和反向代理，再考虑重装。

### 常用命令

| 操作 | Docker 主控 | 原生系统 agent |
| --- | --- | --- |
| 状态 | `docker compose ps`、`curl -fsS http://127.0.0.1:19080/healthz` | `sudo sh /opt/hproxy/agent/agentctl status` |
| 最近日志 | `docker compose logs --tail 100 hproxy-server` | `sudo sh /opt/hproxy/agent/agentctl logs` |
| 配置/数据库检查 | `docker compose exec -T hproxy-server hproxy-server check --data /var/lib/hproxy/server` | `sudo sh /opt/hproxy/agent/agentctl check` |
| 重启 | `docker compose restart hproxy-server` | `sudo sh /opt/hproxy/agent/agentctl restart` |
| 停止/启动 | `docker compose stop hproxy-server` / `docker compose up -d hproxy-server` | `agentctl stop` / `agentctl start` |
| 更新 | 备份后 `docker compose pull hproxy-server && docker compose up -d --no-deps hproxy-server` | `sudo sh /opt/hproxy/agent/agentctl update` |
| 卸载 | `docker compose down` 保留卷 | `sudo sh /opt/hproxy/agent/agentctl uninstall` 保留数据 |

命令在保存 `compose.yaml` 和 `.env` 的目录执行。容器 agent 使用 `docker logs`、`docker stop/start`、`docker pull`；完整步骤见[agent 部署](#子-agent-部署)。OpenRC 与无 init 安装仍使用同一个 `agentctl`，脚本选择相应服务管理方式。

### 故障排查顺序

1. **面板打不开**：检查 `docker compose ps` 和 `/healthz`；如果健康，检查域名、TLS 证书、反向代理到 `127.0.0.1:19080`（或 `.env` 中自定端口）的路径，以及宿主防火墙。不要直接暴露明文主控端口到公网。
2. **agent 离线**：在子机确认 `agentctl status`、`agentctl logs`；测试 `curl -fsS https://panel.example.com/healthz`。检查证书有效期、系统时间、DNS 和代理的 WebSocket 升级；私有 CA 应使用安装时指定的证书。不要反复生成新身份覆盖持久目录。
3. **核心未启动**：先用 `agentctl check` 查看本地状态，再确认业务端口没有被系统已安装的 sing-box、Realm、其他服务或另一个 hproxy agent 占用。项目不会接管系统级核心；端口冲突需调整业务配置。
4. **NAT/IPv6 不通**：分别检查主机监听、LXC/Podman 网络命名空间、运营商 NAT 映射、云安全组、TCP/UDP 协议和公网路由；面板显示双栈不代表服务商放通 IPv6 入站。可用独立客户端按地址族测试，避免把本机回环成功当成公网成功。
5. **更新失败**：查看主控日志、agent 日志和更新状态。不要直接覆盖 `current` 链接或删除 `releases`。Docker 主控切回旧镜像标签；原生 agent 会保留/回退上一健康构建。下载校验失败时检查主控 `/downloads/`、磁盘空间和 HTTPS 代理。

### 数据与清理

Docker 主控的 SQLite、备份和运行状态在命名卷 `/var/lib/hproxy/server`；原生 agent 身份、配置、周期计量点、累计流量、离线队列与日志在 `/var/lib/hproxy/agent`。程序与数据分开，升级程序不清空数据。容器 stdout/stderr 由 Compose 配置轮转；原生 agent 应用日志每份最多 10 MiB、保留 3 份轮转文件。连接日志按面板配置的 7/30/90 天清理，累计流量保留。更多目录与自动回收规则见[文件与维护](#文件与维护)。

定期运行在线数据库备份并验证离机副本可恢复。备份包含密钥及会话相关数据，按敏感数据保护。**不要手动删除正在使用的 SQLite `-wal`/`-shm` 文件**。恢复必须先停止主控，按[Docker 恢复步骤](#docker-主控部署)操作。主控数据库、agent 身份和 agent 流量计数并非同一份文件；只恢复主控旧库可能造成状态和 agent 报告不同步，应先在隔离环境验证恢复路径。

### 删除服务器与失联恢复

原生 agent 的面板“删除服务器”会先撤销资源，再向 agent 发出本地清理任务；离线时任务排队，重连后执行。因而只删除面板记录不等于已经移除业务进程。若主控永久不可用，在子机执行 `agentctl uninstall` 停止服务并移除本项目程序；默认保留数据便于恢复。确认不再需要身份与计数时才使用 `--purge-data`。

OCI 容器 agent 必须先由宿主的 Docker/Podman 停止并移除。容器 agent 不具备原生安装器的远程自删除能力；随后在面板移除离线记录。不要通过删除 Docker/Podman 卷来“修复”连接失败，这会丢失身份和本地累计计数。

## 服务器与流量规则

在面板的“服务器”页面点击“添加服务器”，填写名称并选择核心，再把生成的安装命令复制到目标服务器执行。sing-box 和 Realm 默认都勾选，只选一个就只安装该核心，都不选则只安装子 agent。agent 接入后自动安装选定核心，并上报 IPv4、IPv6 和核心状态。

服务器首次成功连接后才会显示在列表、数量和服务器选项中。未执行安装命令的记录不会显示。已接入的服务器断开后仍保留在列表中。

使用私有 CA 的面板可在安装命令中追加 `--ca /path/to/ca.crt`，下载、接入和后续 WSS/更新均校验证书，无须修改系统证书库。自动化部署可用 `--token-file /path/to/token` 代替 `-t TOKEN`，凭据不会出现在安装器的命令行参数中；两者不能同时指定。

### 核心管理

在服务器管理窗口中安装或卸载 sing-box（代理节点）和 Realm（端口转发）。离线时安装、卸载按钮禁用，接口也会拒绝操作；连接后才能管理核心。创建节点或转发仍会自动选择该功能需要的核心，配置可在重新连接后应用。

服务器、节点和转发统一使用“运行中、已停止、离线、处理中、失败”五种状态，具体原因单独显示。配置保存后先显示“处理中”，收到应用成功确认和核心运行状态后才显示“运行中”；服务器离线时显示“离线”，并说明上线后自动应用。到期、额度用尽归为“已停止”，删除清理过程也使用同一套状态。示例模式共用这套显示逻辑。

失败原因保留在列表、服务器设置和编辑窗口中；监听端口被占用时会明确提示修改端口，修改后保存即可重试。agent 会先检查新增监听端口，启动失败时尝试回退旧配置。

卸载前须删除使用该核心的全部配置，包括已停用和已到期的配置。卸载只移除对应核心程序，不删除 agent 身份、配置和流量记录。

服务器管理窗口右上方的开关控制整台服务器的节点和转发。关闭后停止所有节点与转发，原配置及各配置的启用状态保留；重新开启后按原状态恢复。离线服务器会在重连后应用开关状态。

### 地址与自动同步

服务器列表和详情显示上报的 IPv4、IPv6，不重复显示额外域名。创建节点时，地址优先使用 IPv4；只有 IPv6 时使用 IPv6。需要域名时可直接修改节点的“地址”。

agent 每 30 秒检测公网 IPv4、IPv6，主控连接恢复时也立即检测；请求超时为 4 秒，检测与心跳、流量上报分开执行。某个地址族检测失败时保留最后一次成功地址，避免临时网络故障把配置改为空。地址只在变化后上报和保存。

新建配置通过服务器 ID 保存地址关联：节点地址默认“跟随服务器”，转发中转的对外地址跟随所属服务器，落地“选择服务器”也自动跟随该服务器的 IP。修改节点地址会切换为手动；落地“指定地址”中的 IP 或域名始终保持原值。需要恢复节点自动同步时，点击地址旁的“跟随服务器”。没有关联标记的旧配置或旧导入按手动处理，不根据 IP 相同猜测归属；可在编辑窗口重新启用跟随或选择落地服务器。

主控会在同一事务中更新地址与关联配置，每台受影响 agent 只增加一次配置版本；离线 agent 重连后收到最新地址。保存已打开的编辑窗口时也重新核对关联，避免旧窗口写回旧 IP。地址变化保留配置 ID、端口、密钥、额度、流量周期、周期计量点和累计统计。只改变对外地址时无需重启核心；落地或监听参数实际改变时自动应用对应核心，其他核心保持运行。

指定域名由 Realm 在处理新 TCP 连接和 UDP 数据时重新解析，本地正向 DNS 缓存最多 30 秒（更短的记录 TTL 仍生效），失败结果最多缓存 5 秒，无需人工重启。UDP 在解析结果切换 IPv4/IPv6 时使用各自的关联套接字，旧关联到期不会清掉新关联或其流量计数。解析结果仍受所选上游 DNS 缓存影响；已有 TCP 连接继续使用原连接，不强制迁移到新 IP。

面板会更新节点的分享地址，但已导入外部客户端的固定 IP 链接不会被远程改写；客户端需刷新链接，或在节点地址中使用稳定域名。服务器公网 IP 或实际落地参数变化也可能中断已有连接，自动同步不会清零统计。

创建 Vless Reality 节点默认端口为 8443，SS 2022 为 2053；转发的中转、落地默认端口均为 8443。端口已占用时会提示冲突，需自行修改；编辑现有配置不会重置端口。

服务器页的汇总和列表使用同一口径：北京时间本月的代理节点入站、出站合计。转发流量在端口转发页面单独显示。

转发默认使用双栈监听。选择具有 IPv4 和 IPv6 的落地服务器时，使用两条 Realm 原生端点，分别处理 IPv4 到 IPv4、IPv6 到 IPv6；单栈落地只使用对应地址。指定地址时按填写的 IP 或域名转发，域名解析由 Realm 处理。

节点和转发表单统一通过流量额度与流量周期控制用量，不再单独设置到期日期。旧配置已有的到期限制在重新保存时取消；未重新保存的历史配置仍保持原行为。

### 流量周期

新建节点和转发默认使用月周期。流量周期输入框支持下拉选择月、季、半年、年，选中后分别显示 30、92、183、365；直接输入正整数按自定义天数计算。选择一次性或输入 -1 表示累计额度用完即停，不会自动重置；清空输入也保持不重置的兼容行为。原有配置保留原周期，不会因升级自动改变。

预设周期按北京时间的日历月计算，例如 3 月 15 日开始的每月周期在 4 月 15 日重置。月末会取当月最后一天，后续仍以最初日期计算；手动输入 45 严格按 45 天计算，手动输入 30 也严格按 30 天计算。输入框旁的日历可修改周期开始日期，编辑窗口显示下次重置日期，说明按钮解释两种计算方式。

每次重置只清零本周期额度的已用量，累计流量和历史统计保留。额度耗尽时自动停止，下一周期恢复额度后仅恢复原本启用的配置。修改周期或开始日期本身不会清零已用量。一次性额度按累计用量计算，不会因为重启或经过较长时间恢复额度；需要手动增加额度后再使用。

子 agent 保存周期计量点，重启和断开主控后仍按周期执行。主控通过上报的计量点展示本周期流量。主控、子 agent 均需更新到支持流量周期的版本；旧 agent 的周期设置会被禁用，接口也会拒绝保存。

连接日志默认每页 20 条，可切换为 50 或 100 条。页数根据当前搜索、服务器和日期筛选结果计算，修改筛选或每页条数后回到第一页。

### 安装命令的有效期

安装命令包含一次性接入令牌，有效期为生成后的 **10 分钟**，成功接入后即失效。此期限只用于首次接入，不影响已安装 agent 的持续运行和自动重连。

如果命令过期，重新点击“添加服务器”生成新的安装命令。

示例模式中的命令仅用于展示界面，不能用于真实安装。

### 文件与卸载

本节路径和面板远程卸载针对**原生安装**。主控通过 Docker 部署时，子机仍可照常原生安装。OCI 容器 agent 的本地安装、更新、停用和卷管理见 [agent 部署](#子-agent-部署)。

系统安装的程序和核心位于 `/opt/hproxy/agent`，配置、身份、统计、离线队列和轮转日志位于 `/var/lib/hproxy/agent`。普通用户模式分别使用 `~/.local/share/hproxy/agent` 和 `~/.local/state/hproxy/agent`。agent 只操作自己的子目录，同机主控使用独立的 `server` 子目录。

`--replace` 在新身份成功接入并停止旧服务后，将旧身份、配置和计数保存到数据目录内的 `backups/rebind-<时间>-<PID>/`；保留最近 3 份，不能将新身份直接混用旧身份的资源 ID。默认安装不会覆盖已有身份；普通运行、更新和自动清理不会重置业务配置或计数。删除服务器时仍会卸载该 agent、自己的核心及数据目录，主控历史统计按原规则保留。

## 兼容性与部署边界

### 支持目标

| 环境 | 建议部署方式 | 要点 |
| --- | --- | --- |
| Debian/Ubuntu | 主控 Docker Compose；agent 原生 | systemd 自动注册；需 HTTPS、`curl`/`wget`、`sha256sum` |
| RHEL/CentOS/AlmaLinux/Rocky 等 | 主控 Docker Compose；agent 原生 | 发行版版本、内核及容器运行时由运营者验证；检查 SELinux 与防火墙规则 |
| Alpine | agent 原生 | OpenRC；发布包采用静态 musl，`sha256sum` 可由 BusyBox 提供 |
| LXC、NAT VPS | 容器内原生 agent | 无 systemd/OpenRC 时使用项目监督进程；业务端口必须获得宿主映射 |
| Docker/Podman 宿主 | 可选容器 agent | host network、持久卷、容器生命周期由宿主管理；见[agent 部署](#子-agent-部署) |

三种发布架构为 Linux `x86_64/amd64`、`aarch64/arm64`、`armv7l/armv7`。主控镜像的构建清单与 agent/核心包采用静态 musl 二进制；这降低 glibc/musl 差异，但**不保证**任意旧内核、受限系统调用、32 位容器、SELinux 策略或所有云厂商 NAT 组合可用。安装前核对 `uname -m`、容器架构和业务端口权限。此仓库不发布 Windows/macOS 服务端。

### 网络与容器限制

- agent 管理通道由子机向主控发起 HTTPS/WSS。反向代理或 CDN 需允许 WebSocket、SSE 和较大的 `/downloads/` 文件，不得缓存含身份或管理 API 的响应。主控公网 URL 必须与证书域名一致。
- 容器 agent 的 `--network host` 使用宿主网络命名空间；rootless Podman、LXC nesting 和用户命名空间的权限由宿主决定。低于 1024 的端口、IPv6 绑定、原始网络能力和 `host` 模式在这些环境中可能受限。不能使用 host network 时需为每个业务端口及协议显式映射，且动态配置要同步更新映射。
- NAT VPS 的公网端口映射不由 hproxy 自动申请。面板上的 IPv4/IPv6 可能是容器自身地址；应按提供商映射填写可被客户端访问的地址和端口。IPv4 到 IPv6 等跨地址族转发需两端的网络栈和路由均可用。
- 指定落地域名由项目补丁版 Realm 在运行中重新解析，正向缓存最长约 30 秒、失败后约 5 秒重试；已有 TCP 会话仍使用已建立连接。主控不能替运营者修改 DNS 记录。
- 如果宿主已有 sing-box/Realm，项目仍使用自己的程序和目录；但**相同监听地址与端口会冲突**。安装时不会删除、更新、复用系统程序。

### 容量与版本

内存与磁盘占用受服务器数量、同时连接数、连接日志保留期、离线队列及核心进程影响；仓库不把未经测量的固定值写成最低配置。曾在 128 MiB 的 Alpine NAT 环境分开验证两个核心，但这不能证明所有节点/转发并发场景都适合 128 MiB。部署前在自己的最小目标机器上观察 `docker stats hproxy-server`、`ps`/`free -m`、数据目录和 Docker 卷使用量，给 SQLite 备份与镜像升级预留额外空间。Linux 真机已验证范围和限制见[测试说明](#发版前验证)。

GitHub Actions 发布 `edge`（主分支）与语义版本标签；生产环境固定版本、先备份后升级。Docker 主控与 agent 原生更新属于不同机制，不能把镜像自动拉取视作子机已经更新。发布清单哈希保护下载完整性，传输依赖 HTTPS；管理自己的发行版签名、镜像签名或私有镜像仓库时需另行制定供应链策略。

## 版本更新

> **当前推荐方式**：Docker 主控通过 GitHub Actions 发布到 GHCR，运营者备份后使用 `docker compose pull` / `up -d` 更新；容器主控不提供进程内自替换或自动回滚。下面的面板内更新、监督脚本回滚与 `--update-source` 仅适用于单独安装的**原生托管主控**。详细容器步骤见 [Docker 部署](#docker-主控部署)，原生与容器 agent 的差异见 [agent 部署](#子-agent-部署)。

GitHub `main` 发布测试用 `edge` 镜像；`vX.Y.Z` 标签发布 `X.Y.Z`、`X.Y`、`latest` 镜像与 GitHub Release 原生安装包。原生 agent 从主控 `/downloads/` 读取与镜像匹配的兼容包并自行检查更新；容器 agent 应在宿主 Docker/Podman 中单独拉取和重建。主控镜像内的安装包为只读内容，不在运行时修改。

登录成功或刷新进入面板后在后台静默检查一次更新，切换页面不会重复检查；已有检查或更新任务时沿用当前任务。示例模式使用相同逻辑。设置页保留已有版本结果，后台检查不显示进度提示，结果变化后更新显示，也可点击“检查更新”手动检查。主控启动后和每 6 小时仍会检查一次。

每个有更新的组件均有独立的“更新”按钮，也可以使用“更新全部”。单独更新 hproxy 时同步主控和子 agent，保留现有核心；单独更新 sing-box 或 Realm 时只替换选中的核心，主控无需重启。核心仍然来自部署者的兼容发布清单，不直接跟随上游下载，Realm 使用带流量统计补丁的版本。

首次从旧版升级时，先更新 hproxy，等待所有托管子 agent 报告支持单组件更新，再单独更新核心；不满足条件的按钮会禁用并说明原因。原有“更新全部”仍可直接升级完整兼容包。发布者仍应提升 hproxy 版本并修改 `crates/common/src/lib.rs` 的默认核心版本，让新安装使用完整匹配版本。

`POST /api/v1/updates/apply` 接收发布清单的 `build` 和可选 `component`：`hproxy`、`sing_box`、`realm`、`all`，省略时为 `all`。服务端保留未选组件的文件、版本和 SHA-256，生成新的完整发布清单并原子发布；已安装的新版本不会降级。托管 agent 按经过校验的本地 `release.json` 读取实际核心版本，未托管安装仍使用编译时默认版本。替换前由候选 agent 校验版本和现有资源配置，替换后保留连接与核心健康检查及失败回退。核心更新会短暂重启该 agent 的代理服务。

### 启用在线更新

在线替换支持 Linux amd64、arm64 和 armv7 的托管安装。使用发布包中的 `install-server.sh` 安装主控，配置 HTTPS 发布源：

```sh
sudo sh ./install-server.sh \
  --release-dir /root/hproxy-release \
  --public-url https://panel.example.com \
  --update-source https://releases.example.com/hproxy/stable \
  --password-file /root/hproxy-password
```

密码文件保存至少 12 字符的管理员密码。原生主控默认监听 `127.0.0.1:19080`，由 HTTPS 反向代理提供公网访问；代理须支持 `/agent/v1/connect` 的 WebSocket 升级。默认程序目录 `/opt/hproxy/server`，数据目录 `/var/lib/hproxy/server`。有 systemd 时注册服务，否则使用后台监督进程。托管安装的应用日志统一写入数据目录的 `logs/` 并按大小轮转。

直接运行二进制也能配置 `--update-source` 或 `HPROXY_UPDATE_SOURCE` 来检查版本，但不会在线替换进程。设置页会显示实际运行方式。Windows 的本地预览属于这种情况。

原生托管主控的 `--update-source` 必须由部署者设置为自己信任的 HTTPS 发布目录。GitHub Release 提供归档下载，但不是自动更新端点；不要把 Release 页面 URL 当成此参数。示例域名不是可下载的更新源，示例面板的版本变化也是本地模拟。

自建发布源使用私有 CA 时，安装脚本或直接运行主控均可追加 `--update-ca /path/to/ca.crt`，只为更新连接信任该 CA，无须修改系统证书库。安装脚本将证书保存为数据目录内的 `update-ca.crt`，后续运行不依赖原证书文件的位置。证书主机名和 HTTPS 校验仍然生效。

### 发布文件

构建三个架构的静态 musl 主控和子 agent，准备相同架构的 sing-box 官方 `-musl` 构建及本项目静态补丁版 Realm。统一使用静态二进制，兼容 Debian 等 glibc 系统及 Alpine 等 musl 容器；不要混入依赖 glibc 动态加载器的核心。将 12 个二进制放入同一目录，名称为：

```text
hproxy-server-linux-amd64   hproxy-agent-linux-amd64
sing-box-linux-amd64       realm-linux-amd64
```

arm64、armv7 使用对应后缀。运行仓库中的打包脚本：

```sh
python3 scripts/package-release.py --input ./release-binaries --output ./hproxy-release
```

脚本校验 Linux ELF 架构、大小和程序头，拒绝依赖动态加载器的文件，并生成每个平台的清单及 SHA-256 校验文件。将目录内容完整发布到更新源，先上传二进制，最后发布三份 `manifest-*.json`；更新时应使用原子目录替换或不可变版本目录，避免混合两次发布的文件。

### 更新和失败处理

主控先下载所有架构的 agent 与核心及当前架构的主控，检查文件名、大小、SHA-256、版本匹配，并在数据库副本上运行新主控检查。下载或校验失败会保留当前进程和当前安装包。

切换前将数据库完整备份到数据目录的 `backups/before-update-<build>.db`，写完后才发布备份文件，随后原子切换主控版本目录并重启。新主控监听成功并稳定运行 30 秒后发布子 agent 安装包，通知已连接的 agent 检查更新。离线 agent 重连后执行；子 agent 平时也每 6 小时检查一次。

每个子 agent 下载并校验新 agent 和该服务器已选择的核心，再用这些程序检查现有配置，保存计数器后切换。已卸载的核心不会因更新而重新安装。上线检查失败会回退上一版，失败构建不会反复自动安装。设置页展示 agent 上报的更新失败原因。

新主控启动或早期运行失败由监督进程恢复上一版程序与下载目录。数据库迁移必须向后兼容；自动回退不覆盖数据库中的新写入，必要时可在停止服务后手动恢复更新前备份。主控重启和 agent 核心重启期间会短暂断开连接。

健康检查完成后才回收不再需要的发布目录，保留当前版本和上一健康版本；主控的子 agent 下载包也保留当前和上一套。更新、待确认及回退过程中暂停收集，状态文件损坏时停止清理并记日志。自动备份各保留最近 3 份，手动备份不参与自动删除。清理失败不会使已确认的更新回退，后台每小时重试，详见[文件与维护](#文件与维护)。

## 文件与维护

项目尚未正式部署，新安装直接使用下列路径，不增加旧目录自动迁移逻辑。已有本地开发数据通过显式 `--data` 参数继续使用，安装器和自动清理不会扫描源码或测试目录。

推荐的 Docker 主控使用镜像内 `/usr/local/bin/hproxy-server` 和只读 `/opt/hproxy/release`（agent/核心分发包），数据库、SQLite WAL/SHM、手动备份和主控状态统一保存在命名卷的 `/var/lib/hproxy/server`。镜像层随镜像替换，命名卷随 `docker compose down` 保留，只有显式 `--volumes` 才删除。容器日志由 Docker/Podman 管理，仓库 Compose 配置为每份 10 MiB、保留 3 份。下面的主控 `/opt/hproxy/server` 路径和原地更新清理规则**只适用于原生安装**。

| 内容 | 系统安装位置 |
| --- | --- |
| 主控程序、bootstrap、监督脚本、卸载脚本 | `/opt/hproxy/server/` |
| 主控版本、当前版本链接 | `/opt/hproxy/server/releases/<build>/`、`current` |
| 向 agent 分发的程序和核心 | `/opt/hproxy/server/downloads/releases/<build>/`、`downloads/current` |
| 主控数据库及 SQLite WAL/SHM | `/var/lib/hproxy/server/hproxy.db*` |
| 主控自动数据库备份 | `/var/lib/hproxy/server/backups/before-update-<build>.db` |
| agent 程序、核心、bootstrap 和监督脚本 | `/opt/hproxy/agent/` |
| agent 版本和当前版本链接 | `/opt/hproxy/agent/releases/<build>/`、`current` |
| agent 身份、配置、周期及累计计数、地址缓存 | `/var/lib/hproxy/agent/` 内的 JSON 文件 |
| agent 离线上报队列 | `/var/lib/hproxy/agent/spool/`，约 16 MiB 上限 |
| agent 重新绑定前的数据备份 | `/var/lib/hproxy/agent/backups/rebind-<时间>-<PID>/` |
| 应用运行日志 | 对应数据目录的 `logs/hproxy-server.log` 或 `logs/hproxy-agent.log` |
| 服务注册 | `/etc/systemd/system/hproxy-server.service`、`hproxy-agent.service`；OpenRC agent 使用 `/etc/init.d/hproxy-agent` |

程序目录内少量更新切换元数据随程序管理。普通用户安装 agent 的程序、数据根目录分别为 `~/.local/share/hproxy/agent`、`~/.local/state/hproxy/agent`，内部结构相同。主控安装器支持显式 `--root`、`--data`，两者必须是独立的绝对目录。主控与 agent 不共用数据目录，不共用系统已安装的核心。

Linux 命令行的数据目录默认值与系统安装路径一致；开发和测试仍应明确指定自己的 `--data`、`--core-dir`、`--downloads`。安装器的监督脚本和 systemd 工作目录均固定为对应数据目录，核心进程也从该目录启动。

### 自动清理

- 程序和核心发布目录只保留当前版本及上一健康版本。主控分发给 agent 的完整发布包也保留当前、上一套。第一版安装只有当前版本，不会生成虚假的旧版本。
- 主控下载或准备切换时、更新等待健康确认时不清理版本。agent 暂停收集至上线健康检查完成；更新失败先回退，随后才清理失败版本。实际仍在运行的程序版本也受保护。
- 发布目录名须为项目的 64 位哈希。收集器核对真实路径及所属目录，不跟随符号链接，不扫描兄弟目录。缺失保护路径、损坏状态或清理错误会记日志并停止本轮收集，业务进程继续运行。
- 未发布的下载暂存目录在更新任务结束后清理；过期版本内的半包随版本一起删除。保留目录里的 `.tmp`、`.part` 文件超过 24 小时才清理，避免干扰正在写入的文件。数据库检查副本在检查结束后删除。
- 主控自动数据库备份保留最近 3 份，agent 重新绑定备份保留最近 3 份，均按修改时间排序。手动备份不使用自动备份命名，不会被自动回收。数据库备份在完成写入后才原子发布，避免下次更新误用半份备份。
- 清理在启动健康确认后、更新任务结束后以及后台每小时执行。清理失败只告警并重试，不将成功更新变成失败，也不清空身份、配置、累计流量、周期计量点或当前待上报队列。

应用日志每份最多 10 MiB，保留当前文件和 3 份轮转文件，每个组件约 40 MiB 上限，单次大写入也受限制；重启继续追加现有日志。托管安装显式传入 `--log-dir`。不指定时仍输出到标准输出，便于开发或交给宿主日志系统；systemd 自身的服务消息仍由宿主 journal 管理。

更新下载以流式方式写盘，复用已有安装包和核心时也使用 64 KiB 分块校验，不将整个核心文件读入内存。该校验路径核对文件大小及完整 SHA-256，损坏文件仍须重新下载。

连接日志仍按面板的 7/30/90 天设置清理，累计流量保留。SQLite 删除记录后会复用空闲页，数据库文件不承诺立即缩小；自动维护不会为缩小文件而频繁执行全库重写。

### 回滚与卸载

Docker 主控的回滚是将 `.env` 中镜像标签改回上一已验证版本并重新创建容器；先核对数据库迁移兼容性，必要时停机恢复备份。Docker 卸载使用 `docker compose down`，默认保留卷；完整步骤见 [Docker 部署](#docker-主控部署)。以下主控卸载脚本仅用于原生安装。

更新回滚保留原有身份、资源和累计计数。主控只切回程序和下载包，不覆盖实时数据库；agent 使用原有持久化计数与配置恢复机制，保留回退所需配置快照。停止服务前仍先完成核心关闭和计数落盘。

使用 `--replace` 重新接入时，先停止旧 agent 并保存完整备份。同一主控、同一服务器 ID 仅更换接入凭据时，会继续使用原配置、累计计数、周期计量点、地址缓存及待确认报告，避免从零计数造成少计流量；绑定到另一服务器身份或主控时，从独立状态开始，旧数据仍留在备份中。运行锁和队列文件检查失败时，不移动或恢复外部文件。

主控卸载：

```sh
sudo sh /opt/hproxy/server/uninstall.sh
```

默认只停止服务、删除服务注册及主控程序目录，数据库、配置和备份保留。重新安装到相同数据目录会继续使用原数据库。确实需要清空全部主控数据时，在卸载时显式追加 `--purge-data`。自定义目录安装须同时传入原 `--root` 和 `--data`。卸载脚本核验安装、服务归属及真实路径，不删除 agent 子目录。

agent 通过面板删除服务器触发卸载，清理自己的服务、程序、核心、数据及内部备份；主控和其他应用目录保留。正常清理不删除宿主机共用的 journal。

OCI 容器 agent 由 Docker/Podman 管理，不具备上述原生安装器的远程自删除能力；先在宿主机停止、删除容器，再从面板移除离线记录。容器持久卷默认保留，只有确认身份与流量数据不再需要时才由宿主删除。

## 发版前验证

先运行工作区单元测试和前端构建。Realm UDP 数据路径有独立回归测试，须在 Linux 执行，并启用正式发布使用的批量 UDP 接收：

```sh
cargo test --workspace
cargo test --manifest-path vendor/realm/Cargo.toml -p realm_core --no-default-features --features batched-udp --lib
cd web
npm ci
npm run build
```

UDP 回归用例验证空数据报、1501 字节、8 KiB、60 KB、65507 字节的完整收发和流量计数。本机网络过滤软件也可能阻断回环 UDP，遇到超时须用直接回环收发作对照，再在 Linux 真机确认，不能直接跳过断言。

### 真机矩阵

测试应使用独立目录和服务；测试期间保留服务器原有 IPv6、路由、防火墙、DNS 和证书信任设置。原有服务作为清理前后的对照，不参与测试配置。

| 范围 | 必测情况 |
| --- | --- |
| 接入 | HTTPS 与指定 CA、一次性令牌、无效/过期令牌、重新接入、旧身份失效、未安装服务器隐藏 |
| 身份与权限 | 登录、错误密码、会话退出/改密失效、CSRF、Origin、无身份 WebSocket、跨服务器日志与计数隔离 |
| 节点 | Vless Reality、SS 2022 实际代理请求；IPv4、IPv6；分享链接；新增、编辑、启用、关闭、删除 |
| 转发 | TCP、UDP、TCP+UDP；双栈、IPv4 与 IPv6 交叉转发；IPv6 被禁用时默认监听回落到 IPv4 |
| 异常配置 | 端口冲突、显式指定不可用 IPv6、错误密钥/地址/端口、重复 ID、无效所属服务器；失败后原配置可恢复 |
| 生命周期 | 服务器总开关、额度耗尽、到期自动关闭、核心崩溃恢复、重复进程排他锁 |
| 核心 | 自动安装、未使用核心卸载/重装、有配置引用时禁止卸载、错误哈希/版本不能替换已安装文件 |
| 统计 | 日志分页与筛选、累计计数、重放去重、迟到上报、日志保留、仅节点进入日志和流量图表 |
| 持久化 | 主控离线仍转发、离线落盘与重连补报、双方重启后计数不倒退、在线备份及恢复检查 |
| 导入 | Vless/SS/Realm 配置预览、同端口的互补双栈 Realm 监听合并、重复或冲突导入原子拒绝 |
| 更新 | 正常兼容包、证书不受信任、下载损坏、主控健康检查后发布安装包、子端自动跟随、启动失败回退 |
| 清理 | 停用并删除测试服务、测试进程、程序、数据、证书和安装包；核对原有进程、配置与网络状态 |

### 2026-10-16 真机结果与边界

US - FXT 作为主控，US - BestVM 与 SG - AWS 作为子端。BestVM 的内核不支持 IPv6；SG 提供双栈环境。完成 182 个检查项，其中 179 项通过，3 项因 SG 公网 IPv4 UDP 入站不可达而保留为未通过；直接 UDP 回显对照也超时。未修改服务器网络配置来消除该限制。

工作区 27 项单元测试与 Linux 原生批量 UDP 回归测试通过。amd64、arm64、armv7 发布包均完成交叉编译和 ELF/哈希清单检查，实际运行验证为 amd64。完整在线更新使用临时版本 `0.1.1-lab.1`，源码版本保持 `0.1.0`。

本轮真实接入使用独立目录与测试 systemd 服务，未覆盖正式安装脚本在默认系统路径中的完整安装/卸载。IPv6 入站在 SG 的真实公网地址上自测，跨两台独立公网 IPv6 主机的路径仍需补测。长时间运行、实际掉电和 ARM 真机也不在本轮验证范围。上述边界不得写成已经通过。

详细逐项记录和三台机器的清理验证保存在本轮测试报告目录。正式发布前，应补齐目标部署环境的公网 UDP、独立 IPv6 路径、目标架构与安装脚本验证。

### 2026-10-17 目录与维护改动

程序目录改为 `/opt/hproxy/server`、`/opt/hproxy/agent`，数据目录改为 `/var/lib/hproxy/server`、`/var/lib/hproxy/agent`。项目尚未正式部署，因此未增加旧目录迁移，也未对本地开发数据执行搬迁或清理。

本机 Windows 工作区 66 项测试通过，覆盖版本保护、待确认更新期间暂停收集、损坏状态时拒绝清理、自动备份保留数量、手动备份与业务文件保护、过期临时文件与备份日志侧文件清理、轮转日志大小上限、重新绑定时的运行锁与数据备份，以及卸载 agent 后保留主控兄弟目录。

amd64、arm64、armv7 Linux musl 主控和 agent 构建通过并检查 ELF 架构。Linux 单元测试程序完成交叉编译，其中包括真实符号链接切换、监督进程回滚及链接恢复后的清理用例；当前 Windows 环境没有运行这些 Linux 专属用例。

三个安装/卸载脚本完成 POSIX shell 语法检查。主控卸载脚本在 BusyBox 隔离文件夹具中验证默认保留数据库、显式清空数据、保留 agent 兄弟目录、拒绝错误归属/嵌套目录/异常 PID；该测试模拟了 Linux 系统识别，不等同于 Linux 服务管理器实测。发布打包脚本用 12 份小型 ELF 头夹具验证了三架构清单、哈希及全部脚本的打包；这些夹具不是可运行发布包。

新路径下的 Linux 真机安装、systemd/OpenRC 启停、故障更新和完整卸载仍须在隔离 Linux 环境验证后再正式部署，不沿用前一天的旧路径测试结果。


### 2026-10-17 地址同步与动态 DNS

本机主项目 70 项测试通过（agent 25、common 12、主控 33），前端构建和 12 项地址关联交互检查、26 项弹窗关闭检查、49 项额度/下拉/复制回归通过；Realm 的 3 项配置/兼容检查与 2 项动态 DNS/地址族隔离检查通过。

新增回归覆盖显式服务器关联、节点与中转/落地地址同步、手填同值 IP 与域名保护、旧配置不猜测归属、重复地址上报不增加版本、旧编辑窗口保存时采用最新地址，以及更新前后累计计数、历史流量、周期计量点与回执保持一致。agent 回归验证只改分享地址无需调用核心程序，实际落地变更只影响 Realm。

Realm 使用本地 UDP DNS 夹具验证同一解析器在运行中跨越缓存到期，从旧 IPv4 切换到新 IPv4，再切到 IPv6；夹具上游声明 86400 秒 TTL，测试用 1 秒缓存上限加速。配置测试核对生产上限为正向 30 秒、失败 5 秒。UDP 映射回归验证旧地址族关联超时不会删除新地址族关联。离线 React 交互夹具验证自动地址显示、手填覆盖、稳定服务器 ID、复制清空落地，以及与真实配置相同的 DNS 策略。

amd64、arm64、armv7 Linux musl 的主控、agent 与 Realm 均构建通过，9 个组件产物完成 ELF 架构检查。

本轮尚未进行 Linux 真机公网 IP 变更与动态域名的端到端转发验证。Windows 下既有的零字节 UDP 数据报回归在接收零字节时报超时；该项目部署目标为 Linux，该项仍需在 Linux 运行验证，不计为本轮通过。

### 2026-10-18 Linux 完整验证

本节记录当前源码的 Linux 验证结果，补齐前述各轮尚未执行的正式目录安装、维护和卸载。部署目标仅为 Linux，开发机上的测试结果不作为 Linux 验收依据。

US - FXT 同时运行主控和 agent，US - BestVM、SG - AWS、HK - HuaNa 运行 agent。前三台使用 systemd；HK 使用 Alpine/OpenRC、128 MB 内存及 NAT 网络，两个项目核心分开安装测试。原有 sing-box、Realm、配置、网络和证书信任库不参与测试修改。正式安装目录在测试前均不存在。

主控、agent、Realm 和官方静态 musl sing-box 已打成 amd64、arm64、armv7 发布包并完成架构、静态 ELF 和哈希检查；实际运行环境为 amd64，四台完整流程所用构建均核对运行进程映像哈希。源码主控/agent 版本保持 `0.1.0`，测试更新使用的临时版本未留在最终包中。

最终源码在 Linux 原生运行的 82 项测试全部通过：common 14、agent 28、server 34、Realm 配置 3、Realm 数据路径 3。

#### 实际流程

- 面板、API、数据及 agent：服务器、Vless Reality、SS 2022、TCP/UDP/TCP+UDP 转发的增删查改、启停、导入、配置查看、端口冲突恢复、无效参数与原子拒绝；登录、改密、退出、CSRF、Origin、令牌一次性使用和跨服务器上报隔离。
- 数据路径：真实代理 HTTP、IPv4/IPv6 及地址族交叉转发，独立 Windows 客户端通过 SG 公网 IPv6 连接两个节点协议；Realm 的空 UDP 与 65507 字节边界、240 个混合并发客户端。SS 大 UDP 修复后，最终稳定版本的公网 1 KiB/8 KiB 收发连续五轮通过。
- 地址与 DNS：真实 WSS 地址变更上报同步节点、中转和落地，保留手填地址及累计计数，旧编辑窗口不能恢复旧地址。域名夹具使用上游长 TTL，验证生产的 30 秒缓存上限、解析地址运行中切换、已有 TCP 连接保持，以及解析失败后的 5 秒重试；宿主机公网地址和系统 DNS 本身未改动。
- 额度与统计：额度耗尽关闭、周期重置恢复、停用配置不被自动启用、到期关闭、离线落盘与补报、双方重启、重复和迟到上报、日志筛选分页及保留期；转发不进入节点日志和图表。真实同身份重新接入后，累计流量从 2048 字节增至 4096 字节，周期计量点保持不变。
- 安装与维护：默认系统路径安装，HTTPS/指定 CA，普通用户路径模式的后台监督进程，重复安装拒绝、重新接入备份、在线完整备份恢复、主控与 agent 更新、核心独立更新、损坏包拒绝、真实监督脚本启动失败回退、旧版本/临时文件/自动备份清理及手动文件保护。普通用户路径模式由 root UID 实测，不等同于非 root 权限测试。
- 卸载：离线服务器删除任务保留，重连后执行；四台 agent 的程序、数据及服务注册均真实移除。同机 agent 卸载保留主控；主控默认卸载后可用原数据库重装，最后显式清空本轮测试数据库。
- 浏览器：实际 Linux 面板上的创建、保存、复制、删除、额度单位与周期，日历固定尺寸与年月选择，弹窗外部点击和 Escape 保留输入，提示浮层边界、Vless 折叠/SS 表单高度、桌面和窄屏布局。浏览器运行于测试客户端，面板后端运行于 Linux。

#### 本轮修复

安装器补齐私有 CA 和凭据文件传递；发布打包拒绝带动态加载器的 ELF，避免 Alpine 上核心文件存在却无法执行；已进入删除流程的旧 agent 不再阻塞其他机器的兼容更新；SS 2022 入站开启 UDP 分片；同身份更换接入凭据时保留累计计数、周期计量点和待确认报告，且恢复失败会还原旧数据。面板还修复初始数据加载前可误开创建窗口，以及窄屏资源表格挤压地址的问题。

完整四台安装/业务/更新/卸载流程完成后，另将更新重试时的缓存文件校验改为 64 KiB 分块。此改动单独重跑 Linux 原生测试，并在 HK 上限制校验子进程地址空间为 64 MiB，校验 128 MiB 文件及其损坏情况；未再次重复四台完整安装流程。该专项不修改系统内存上限或原有服务。

HK 专项通过正确文件、大小不符、内容损坏和文件缺失检查，子进程峰值 RSS 为 7336 KiB（约 7.2 MiB），cgroup 没有新增 OOM 事件。

#### 保留的环境限制

SG 公网 UDP 入站仍有超时，独立客户端的 IPv4/IPv6 直接 UDP 对照同样失败；本机 UDP 和地址族交叉转发通过。HK 测试公网端口 `55539` 被拒绝，本机及 SSH 隧道内转发、节点通过，尚需确认服务商 NAT 映射或外部过滤。没有将这些公网路径计为通过，也没有改动原有防火墙、路由或服务商配置来掩盖限制。

HK 在北京时间 23:53:55 至 23:56:56 之间经历正常 OpenRC 关机/开机流程，原有 Realm 和 SSH 随系统重新启动，配置哈希与监听恢复一致。日志未确定发起方；当前 cgroup 无 OOM 事件，不能将这次重启认定为内存不足。项目未执行整机重启命令。

更新健康确认期间出现的一次 SS UDP 超时保留在原始记录中；最终包哈希和进程稳定后另行完成五轮验证。早期测试脚本的空数据报代理断言、一次大 UDP 丢包及修复前失败记录也保留，不使用简单的“累计通过条目数”代表全部场景通过。零字节 UDP 仅对 Realm 验证通过；两个加密节点协议的零字节 UDP 未计入已通过范围。

ARM 真机、非 root UID、Docker 镜像、长时间运行和实际掉电不在本次验证范围。逐项原始记录、Linux 测试输出、运行哈希及界面截图保存在 `WORK/90_临时/hproxy-full-test-20261017`；该目录中的旧测试包和凭据不属于正式发布包。

最终四台测试安装、独立测试目录和临时进程均已移除，原有配置哈希及安装路径与基线一致，原有 TCP 服务监听保留。临时 Netcatty 转发规则移除后，关闭本轮创建的 SSH 会话。

## 前端视觉与交互验收

面板覆盖服务器、代理节点、端口转发、连接日志、流量统计、设置与登录入口，沿用黑白灰视觉体系。创建和编辑窗口按内容定高；默认节点与转发表单在桌面视口无空白占位和纵向滚动，展开 Vless 连接参数时可在窗口内滚动。下拉菜单、年份列表与日期选择使用统一的边框、焦点和间距；切换月份或年份时，日历尺寸保持稳定。

2026-10-18 的浏览器检查覆盖桌面 1440×900、平板 768×900、手机 390×844 的列表、表单、弹窗、日期选择与浅色/暗色交互。窄屏下节点与转发改用卡片展示关键字段；日志表格保留横向滚动提示。桌面转发列表和手机页面未发现额外水平溢出，前端 TypeScript 与 Vite 构建通过。这是浏览器界面的验收范围，Linux 后端与容器部署结果以上面的真机和 CI 记录为准。

## hproxy 界面基准

视觉参考见 [界面基准图](docs/design/accepted.png)。面板仅保留服务器、节点、转发、日志、统计和设置等直接服务于管理任务的页面。

- 背景 #fafafa，表面 #fff，文本 #202020，次要文本 #737373，边框 #e5e5e5。
- 深色主按钮与导航选中态，绿色只表示运行正常；错误使用克制的红色。
- 侧栏 224px，内容内边距 28px，标题 22px/550，正文 14px，辅助文字 12–13px。
- 输入与按钮 38px 高、6px 圆角；面板 8px 圆角；表格行约 76px。
- 图标统一 lucide-react，18px、strokeWidth=1.75；不混用 emoji、实心图标或其他图标集。
- 服务器为默认首页：紧凑四项摘要、筛选栏与服务器列表；流量趋势放在流量统计页。
- 实际运行展示真实数据；无服务器时给出接入操作，无流量时展示零值与空趋势。
- 页面：服务器、代理节点、端口转发、连接日志、流量统计、设置。
- 窄屏保留清晰的任务顺序，导航折叠，表格可横向滚动，表单单列。

## 许可与第三方来源

hproxy 主控、agent、前端代码及项目脚本按仓库根目录的 [GPL-3.0-or-later](LICENSE) 发布。源文件中未单独标注时，以工作区 `Cargo.toml` 的许可证声明为准。贡献者提交代码应确认有权按该许可证发布。

仓库和发布包同时包含第三方材料，使用时应保留对应版权和许可声明：

| 材料 | 来源与说明 | 许可位置 |
| --- | --- | --- |
| Realm 补丁源码 | 本仓库 `vendor/realm`，上游作者 zhboner；项目修改动态 DNS、流量统计等代码 | [`vendor/realm/LICENSE`](vendor/realm/LICENSE)，MIT |
| sing-box 静态发布二进制 | 构建脚本固定从 [SagerNet/sing-box v1.14.3](https://github.com/SagerNet/sing-box/releases/tag/v1.14.3) 下载并校验 SHA-256；不在源码库中提交其二进制 | [上游仓库的许可与声明](https://github.com/SagerNet/sing-box)；使用发布包时请同时阅读上游条款 |
| 国旗后备图形 | Twemoji 17.0.3 的原始 SVG，仅在系统不支持彩色旗帜时使用 | [`SOURCES.md`](web/public/flag-emoji/SOURCES.md) 和 [`LICENSE-GRAPHICS`](web/public/flag-emoji/LICENSE-GRAPHICS)，CC BY 4.0 |
| Rust/npm 依赖 | `Cargo.lock` 与 `web/package-lock.json` 固定实际构建依赖 | 各依赖的上游许可文件 |

自动发布的 OCI 镜像包含项目主控或 agent 二进制、Alpine 基础层及 CA 证书，并可能包含 sing-box/Realm 发布包。分发镜像或发布包时仍须遵守这些组件各自的许可条件；本页不是对第三方许可的重新授权。
