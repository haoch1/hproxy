# hproxy

面向自有 Linux 服务器的代理节点与端口转发控制台。Rust 主控提供 Web 面板、SQLite 存储和配置下发；轻量 agent 主动通过 HTTPS/WebSocket 连接主控，在子机运行项目管理的 sing-box 与补丁版 Realm。主控推荐使用 Docker Compose 部署，agent 推荐在宿主 Linux 或 LXC 内原生安装。

> 适用范围：管理自己有权使用的服务器和网络。发布版本支持 Linux `amd64`、`arm64`、`armv7`；浏览器访问面板需要 HTTPS 公网入口。尚未验证的系统或网络环境见[兼容性与限制](docs/compatibility.md)。

## 功能

- VLESS Reality + Vision、Shadowsocks 2022 节点；TCP、UDP、TCP+UDP 端口转发。Realm 使用本仓库的计数与动态 DNS 补丁，不调用宿主机已有的 Realm。
- 节点和转发独立计量入站、出站流量；额度支持 K/M/G/T 与 KB/MB/GB/TB 输入，按 1024 换算；流量周期支持月、季、半年、年、自定义天数和一次性额度。
- IPv4/IPv6 双栈、不同地址族间的转发、服务器地址变更同步与动态域名重新解析。手动地址保持原值；详情见[服务器与流量规则](docs/agents.md)。
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

反向代理示例、首次初始化、数据备份、恢复和升级步骤见[Docker 部署](docs/deployment/docker.md)。agent 的 systemd/OpenRC/无 init、普通用户、Docker/Podman 方式见[agent 部署](docs/deployment/agents.md)；常用命令与故障恢复见[运维手册](docs/operations.md)。

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

本机预览通过 `hproxy-server run --data ./data` 监听 `127.0.0.1:19080`；`/demo/` 使用浏览器本地示例数据，不连接真实 agent。Linux 真机验证记录与未覆盖项目见[测试说明](docs/testing.md)。

## 文档

| 主题 | 内容 |
| --- | --- |
| [Docker 主控部署](docs/deployment/docker.md) | HTTPS 接入、镜像版本、备份、恢复、更新与卸载 |
| [agent 部署](docs/deployment/agents.md) | 原生安装、容器安装、LXC/NAT/Podman 操作 |
| [运维手册](docs/operations.md) | 主控失联时的本地控制、健康检查与故障恢复 |
| [兼容性与限制](docs/compatibility.md) | 发行版、架构、容器权限和网络边界 |
| [服务器与流量规则](docs/agents.md) | 地址同步、流量周期、接入令牌与清理 |
| [版本更新](docs/updates.md) | 发布包、原生更新和回滚机制 |
| [文件与维护](docs/storage.md) | 数据位置、日志、备份和清理策略 |
| [测试说明](docs/testing.md) | Linux 验证范围与保留的限制 |

## 安全与许可

管理员密码使用 Argon2 存储；面板操作检查会话、Origin 和 CSRF。接入令牌一次使用、10 分钟过期。数据库、备份、agent 身份和密钥应限制访问权限，反向代理应使用可信证书。面板“运行配置”会显示真实密钥，仅对管理员开放。

hproxy 自身按 [GPL-3.0-or-later](LICENSE) 授权。`vendor/realm`、sing-box 与前端旗帜图形等第三方组件保留各自许可；来源见[第三方许可](docs/licenses.md)。
