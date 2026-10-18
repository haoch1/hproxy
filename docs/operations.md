# 运维与数据

以下主控命令在 `compose.yaml` 与 `.env` 所在目录执行；agent 命令在目标服务器执行。若修改了 `HPROXY_PORT`，健康检查也应改用相同端口。

| 操作 | 主控 | 原生 agent |
| --- | --- | --- |
| 查看状态 | `docker compose ps` | `sudo sh /opt/hproxy/agent/agentctl status` |
| 查看日志 | `docker compose logs --tail 100 hproxy-server` | `sudo sh /opt/hproxy/agent/agentctl logs` |
| 检查数据 | `docker compose exec -T hproxy-server hproxy-server check --data /var/lib/hproxy/server` | `sudo sh /opt/hproxy/agent/agentctl check` |
| 重启 | `docker compose restart hproxy-server` | `sudo sh /opt/hproxy/agent/agentctl restart` |

## 备份与恢复

主控 SQLite 数据和备份位于 `hproxy-server-data` 卷的 `/var/lib/hproxy/server`。运行中使用内置备份命令，不直接复制活跃的数据库文件：

```sh
mkdir -p ./backups
backup_name="manual-$(date -u +%Y%m%dT%H%M%SZ).db"
docker compose exec -T hproxy-server hproxy-server backup \
  --data /var/lib/hproxy/server \
  --output "/var/lib/hproxy/server/backups/$backup_name"
docker cp "hproxy-server:/var/lib/hproxy/server/backups/$backup_name" "./backups/$backup_name"
chmod 600 "./backups/$backup_name"
```

保留异机备份。恢复前停止主控，使用与备份兼容的镜像版本；先保存当前数据库，再把备份恢复到卷内，运行 `hproxy-server check` 后启动。备份含身份和密钥，妥善保管。原生 agent 的 `/var/lib/hproxy/agent` 是独立数据目录，需单独备份；只恢复主控旧库可能与 agent 的最新计数不一致。

## 更新与回滚

主控先备份，再将 `.env` 的 `HPROXY_IMAGE` 设为目标版本：

```sh
docker compose pull hproxy-server
docker compose up -d --no-deps hproxy-server
docker compose ps
```

失败时将 `HPROXY_IMAGE` 改回原版本，重新执行 `docker compose up -d --no-deps hproxy-server`。如果数据库迁移不兼容，停机后使用升级前的备份恢复。原生 agent 用 `agentctl update` 检查兼容发布包，失败时保留或回退上一个健康版本；离线 agent 重连后再检查。

容器 agent 由宿主更新。先保留原镜像标签和数据卷，再拉取新镜像并重建容器；需要回滚时用原标签重建。不要删除 `hproxy-agent-data` 卷。Podman 使用对应的 `podman pull/stop/rm/run` 命令。

## 卸载与清理

```sh
docker compose down
sudo sh /opt/hproxy/agent/agentctl uninstall
```

`docker compose down` 保留主控卷；原生 agent 卸载默认保留数据。确认已备份且不再使用后，才分别执行 `docker compose down --volumes` 或 `agentctl uninstall --purge-data`。容器 agent 先在宿主停止并删除容器，再决定是否删除 `hproxy-agent-data` 卷。

程序版本仅保留当前和上一健康版本；自动备份与日志按限额轮转。手动备份不参与自动清理。卸载或升级不会清理其他程序安装的 sing-box/Realm；相同监听端口由管理员自行协调。
