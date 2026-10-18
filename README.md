# hproxy

hproxy 是面向 Linux 服务器的代理节点与端口转发控制台。Docker 主控提供 Web 面板和 SQLite 存储；各子机安装原生二进制 agent，主动连接主控并运行项目管理的 sing-box 与 [Realm](vendor/realm/LICENSE) 补丁版。LXC 和 NAT VPS 也按此方式接入，子机不需要 Docker。

## 功能

- 管理 VLESS Reality、Shadowsocks 2022 节点及 TCP/UDP 转发。
- 按节点或规则统计流量，设置额度与重置周期。
- 同步服务器 IPv4/IPv6 变化，运行中重新解析落地域名；手动填写的地址保持不变。
- 按服务器管理 sing-box、Realm 核心，提供 agent 更新、日志和备份操作。

## 部署要求

| 组件 | 运行方式 | 要求 |
| --- | --- | --- |
| 主控 | Docker Compose | Linux、Docker Engine 与 Compose 插件；可供子机访问的 HTTPS 域名 |
| 子机 | 原生二进制 agent | Linux amd64、arm64 或 armv7；`curl`/`wget` 和 `sha256sum` |

主控镜像包含三个架构的 agent 安装文件，无需单独下载发布包。agent 经 HTTPS/WebSocket 主动连接主控，不需要开放管理通道的子机入站端口；代理节点和端口转发使用的业务端口仍需按实际网络放行。系统已安装的 sing-box/Realm 不会被覆盖，但相同监听端口会冲突。

## 快速开始

在主控 Linux 主机上执行：

```bash
git clone https://github.com/haoch1/hproxy.git
cd hproxy
cp .env.example .env
# 将 .env 中的 HPROXY_PUBLIC_URL 改为实际的 HTTPS 面板地址
docker compose pull
read -rsp '设置管理员密码: ' HPROXY_PASSWORD; echo
printf '%s' "$HPROXY_PASSWORD" | docker compose run --rm -T --no-deps \
  --entrypoint /usr/local/bin/hproxy-server-init hproxy-server init
unset HPROXY_PASSWORD
docker compose up -d
```

管理员密码至少 12 字符。默认端口为 `19080`，仅绑定宿主机 `127.0.0.1`；请通过支持 WebSocket 和 SSE 的 HTTPS 反向代理对外提供面板，并转发 `/downloads/`。若端口被占用，修改 `.env` 中的 `HPROXY_PORT`。可用 `docker compose ps` 和 `curl -fsS http://127.0.0.1:19080/healthz` 检查主控。

## 接入子机

进入面板的“服务器”页面添加服务器，在目标 Linux 主机执行面板生成的一次性安装命令。安装器按 CPU 架构从主控下载并校验 agent 二进制，然后使用 systemd、OpenRC 或内置监督进程运行它。安装完成后可在子机检查：

```sh
sudo sh /opt/hproxy/agent/agentctl status
```

普通用户安装、LXC/NAT 环境以及本地恢复命令见[部署说明](docs/deployment.md)。

## 数据与更新

主控数据存放在 Docker 卷 `hproxy-server-data`（容器内 `/var/lib/hproxy/server`）；子机程序位于 `/opt/hproxy/agent`，身份、配置、流量统计及日志位于 `/var/lib/hproxy/agent`。更新和卸载不会默认删除这些数据。

推送 `main` 后 GitHub Actions 自动构建主控的 `latest` 镜像，并为该次构建保留 `sha-<完整提交 SHA>` 标签。升级前先备份主控数据并记录当前镜像 digest，再运行 `docker compose pull && docker compose up -d`；agent 可通过面板或 `agentctl update` 更新。回滚、备份与卸载步骤见[运维说明](docs/operations.md)。本项目不需要 GitHub Release 或子机容器镜像。

## 文档

- [部署主控和 agent](docs/deployment.md)：HTTPS 入口、Docker 主控与 Linux 二进制 agent。
- [运维与数据](docs/operations.md)：检查、备份、更新、回滚和卸载。

源码位于 `crates/`、`web/`，项目补丁版 Realm 位于 `vendor/realm/`。构建流程见 [GitHub Actions](.github/workflows/ci.yml)。项目采用 [GPL-3.0-or-later](LICENSE)；第三方素材的许可声明保存在各自目录。
