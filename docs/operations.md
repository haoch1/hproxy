# 运维与故障恢复

主控与 agent 有各自的数据和生命周期。主控容器失联不会立即清空 agent 已下发的配置；agent 继续使用本地配置和持久化流量计数，连接日志写入有上限的离线队列。恢复连接后补报。管理操作需要主控可用；应首先检查健康状态、证书、DNS 和反向代理，再考虑重装。

## 常用命令

| 操作 | Docker 主控 | 原生系统 agent |
| --- | --- | --- |
| 状态 | `docker compose ps`、`curl -fsS http://127.0.0.1:19080/healthz` | `sudo sh /opt/hproxy/agent/agentctl status` |
| 最近日志 | `docker compose logs --tail 100 hproxy-server` | `sudo sh /opt/hproxy/agent/agentctl logs` |
| 配置/数据库检查 | `docker compose exec -T hproxy-server hproxy-server check --data /var/lib/hproxy/server` | `sudo sh /opt/hproxy/agent/agentctl check` |
| 重启 | `docker compose restart hproxy-server` | `sudo sh /opt/hproxy/agent/agentctl restart` |
| 停止/启动 | `docker compose stop hproxy-server` / `docker compose up -d hproxy-server` | `agentctl stop` / `agentctl start` |
| 更新 | 备份后 `docker compose pull hproxy-server && docker compose up -d --no-deps hproxy-server` | `sudo sh /opt/hproxy/agent/agentctl update` |
| 卸载 | `docker compose down` 保留卷 | `sudo sh /opt/hproxy/agent/agentctl uninstall` 保留数据 |

命令在保存 `compose.yaml` 和 `.env` 的目录执行。容器 agent 使用 `docker logs`、`docker stop/start`、`docker pull`；完整步骤见[agent 部署](deployment/agents.md)。OpenRC 与无 init 安装仍使用同一个 `agentctl`，脚本选择相应服务管理方式。

## 故障排查顺序

1. **面板打不开**：检查 `docker compose ps` 和 `/healthz`；如果健康，检查域名、TLS 证书、反向代理到 `127.0.0.1:19080`（或 `.env` 中自定端口）的路径，以及宿主防火墙。不要直接暴露明文主控端口到公网。
2. **agent 离线**：在子机确认 `agentctl status`、`agentctl logs`；测试 `curl -fsS https://panel.example.com/healthz`。检查证书有效期、系统时间、DNS 和代理的 WebSocket 升级；私有 CA 应使用安装时指定的证书。不要反复生成新身份覆盖持久目录。
3. **核心未启动**：先用 `agentctl check` 查看本地状态，再确认业务端口没有被系统已安装的 sing-box、Realm、其他服务或另一个 hproxy agent 占用。项目不会接管系统级核心；端口冲突需调整业务配置。
4. **NAT/IPv6 不通**：分别检查主机监听、LXC/Podman 网络命名空间、运营商 NAT 映射、云安全组、TCP/UDP 协议和公网路由；面板显示双栈不代表服务商放通 IPv6 入站。可用独立客户端按地址族测试，避免把本机回环成功当成公网成功。
5. **更新失败**：查看主控日志、agent 日志和更新状态。不要直接覆盖 `current` 链接或删除 `releases`。Docker 主控切回旧镜像标签；原生 agent 会保留/回退上一健康构建。下载校验失败时检查主控 `/downloads/`、磁盘空间和 HTTPS 代理。

## 数据与清理

Docker 主控的 SQLite、备份和运行状态在命名卷 `/var/lib/hproxy/server`；原生 agent 身份、配置、周期计量点、累计流量、离线队列与日志在 `/var/lib/hproxy/agent`。程序与数据分开，升级程序不清空数据。容器 stdout/stderr 由 Compose 配置轮转；原生 agent 应用日志每份最多 10 MiB、保留 3 份轮转文件。连接日志按面板配置的 7/30/90 天清理，累计流量保留。更多目录与自动回收规则见[文件与维护](storage.md)。

定期运行在线数据库备份并验证离机副本可恢复。备份包含密钥及会话相关数据，按敏感数据保护。**不要手动删除正在使用的 SQLite `-wal`/`-shm` 文件**。恢复必须先停止主控，按[Docker 恢复步骤](deployment/docker.md)操作。主控数据库、agent 身份和 agent 流量计数并非同一份文件；只恢复主控旧库可能造成状态和 agent 报告不同步，应先在隔离环境验证恢复路径。

## 删除服务器与失联恢复

原生 agent 的面板“删除服务器”会先撤销资源，再向 agent 发出本地清理任务；离线时任务排队，重连后执行。因而只删除面板记录不等于已经移除业务进程。若主控永久不可用，在子机执行 `agentctl uninstall` 停止服务并移除本项目程序；默认保留数据便于恢复。确认不再需要身份与计数时才使用 `--purge-data`。

OCI 容器 agent 必须先由宿主的 Docker/Podman 停止并移除。容器 agent 不具备原生安装器的远程自删除能力；随后在面板移除离线记录。不要通过删除 Docker/Podman 卷来“修复”连接失败，这会丢失身份和本地累计计数。
