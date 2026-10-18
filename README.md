# hproxy

hproxy 是面向 Linux 服务器的代理节点与端口转发控制台。主控通过 Docker Compose 运行，提供 Web 面板和 SQLite 数据存储；每台子机安装原生二进制 agent，主动连接主控，并按需运行项目管理的 sing-box 与 [Realm](vendor/realm/LICENSE) 补丁版。LXC、NAT VPS 等子机也使用同一种 agent 安装方式，无需 Docker。

## 能做什么

- 创建 VLESS Reality、Shadowsocks 2022 节点，以及 TCP、UDP 端口转发规则。
- 查看服务器状态、连接日志和流量统计；为节点或转发规则设置流量额度与重置周期，支持月、季、半年、年、一次性额度和自定义天数。
- 服务器公网 IPv4/IPv6 变化时同步使用该服务器地址的节点与转发配置；落地使用域名时在运行中重新解析。手动指定的地址不会被自动覆盖。
- 从面板管理子机的 sing-box、Realm 核心与 agent 更新；主控失联时仍可在子机执行本地检查、重启和卸载命令。

Web 面板使用 React、TypeScript 和 Vite；主控与 agent 使用 Rust，主控以 SQLite 保存配置及统计数据。项目安装的核心与系统中已有的同名程序分别存放，不会覆盖系统安装；如果双方监听同一端口，仍需调整端口避免冲突。

## 部署前准备

| 组件 | 运行方式 | 需要准备 |
| --- | --- | --- |
| 主控 | amd64 Linux 上的 Docker Compose | Docker Engine、Compose 插件、可供子机访问的 HTTPS 域名及反向代理 |
| 子机 | x86_64 Linux 原生二进制（Intel/AMD） | `wget` 和 `sha256sum`（只有 `curl` 时可替换面板命令中的下载步骤） |

主控默认使用 `19080`，仅绑定宿主机 `127.0.0.1`。反向代理需要转发 WebSocket、SSE 和 `/downloads/`；子机经 HTTPS/WebSocket 主动连接主控，无须为管理通道开放子机入站端口。节点和转发规则使用的业务端口仍须在服务器防火墙、云防火墙及 NAT 映射中放行。主控及子机都只支持 Linux 部署。

## 安装主控

在主控 Linux 主机执行：

```bash
git clone https://github.com/haoch1/hproxy.git
cd hproxy
cp .env.example .env
# 编辑 .env：将 HPROXY_PUBLIC_URL 设置为实际的 HTTPS 面板地址
docker compose pull
read -rsp '设置管理员密码: ' HPROXY_PASSWORD; echo
printf '%s' "$HPROXY_PASSWORD" | docker compose run --rm -T --no-deps \
  --entrypoint /usr/local/bin/hproxy-server-init hproxy-server init
unset HPROXY_PASSWORD
docker compose up -d
```

管理员密码至少 12 字符。完成 HTTPS 反向代理配置后，通过 `.env` 中的 `HPROXY_PUBLIC_URL` 访问面板。检查主控状态：

```bash
docker compose ps
curl -fsS http://127.0.0.1:19080/healthz
docker compose logs --tail 100 hproxy-server
```

若 `19080` 已占用，在 `.env` 修改 `HPROXY_PORT`；健康检查也改用新端口。容器名为 `hproxy-server`，数据卷名为 `hproxy-server-data`。HTTPS 反向代理和端口设置见[部署说明](docs/deployment.md)。

## 接入子机

在面板的“服务器”页面添加服务器，然后在该 x86_64 Linux 子机执行面板生成的一次性安装命令。安装器会检查 CPU 架构，从主控下载并校验 agent 二进制；安装凭据有效期为 10 分钟，不要将命令或凭据公开。主控镜像已包含子机所需的安装文件，不需要 GitHub Release 或单独的 agent 镜像。

系统安装会使用 systemd 或 OpenRC；没有这两者时启动自身监督进程，需按所在环境配置开机自启。接入后可在子机执行：

```sh
sudo sh /opt/hproxy/agent/agentctl status
sudo sh /opt/hproxy/agent/agentctl check
sudo sh /opt/hproxy/agent/agentctl logs
```

普通用户安装、受限 LXC/NAT 环境及主控失联时的操作见[部署说明](docs/deployment.md)。

## 数据与日常维护

| 位置 | 内容 |
| --- | --- |
| Docker 卷 `hproxy-server-data`（容器内 `/var/lib/hproxy/server`） | 主控数据库与主控备份 |
| 子机 `/opt/hproxy/agent` | agent 程序及项目管理的核心 |
| 子机 `/var/lib/hproxy/agent` | 身份、配置、流量统计、日志与备份 |

子机使用 `--user` 安装时，程序和数据分别位于 `~/.local/share/hproxy/agent` 与 `~/.local/state/hproxy/agent`。主控容器更新和 agent 卸载默认保留持久数据；删除数据前先备份。主控可用 `docker compose logs` 查看日志，子机可用 `agentctl logs` 查看日志。

推送 `main` 后，[GitHub Actions](.github/workflows/ci.yml) 自动验证 Linux 构建并更新主控 `latest` 镜像，同时保留该次构建的 `sha-<完整提交 SHA>` 镜像标签。升级前先备份主控数据并记录当前镜像 digest，然后执行 `docker compose pull && docker compose up -d`。子机可通过面板更新，或使用 `sudo sh /opt/hproxy/agent/agentctl update` 检查主控提供的兼容包。备份、固定版本、回滚与卸载命令见[运维说明](docs/operations.md)。

## 更多资料

- [部署说明](docs/deployment.md)：HTTPS 入口、Docker 主控、原生 agent 与 LXC/NAT 注意事项。
- [运维说明](docs/operations.md)：检查、备份、更新、回滚和卸载。

主控、agent 与共享代码位于 `crates/`，前端位于 `web/`，项目补丁版 Realm 位于 `vendor/realm/`。项目采用 [GPL-3.0-or-later](LICENSE)；第三方素材的许可声明保存在各自目录。
