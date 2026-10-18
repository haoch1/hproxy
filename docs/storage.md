# 文件与维护

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

## 自动清理

- 程序和核心发布目录只保留当前版本及上一健康版本。主控分发给 agent 的完整发布包也保留当前、上一套。第一版安装只有当前版本，不会生成虚假的旧版本。
- 主控下载或准备切换时、更新等待健康确认时不清理版本。agent 暂停收集至上线健康检查完成；更新失败先回退，随后才清理失败版本。实际仍在运行的程序版本也受保护。
- 发布目录名须为项目的 64 位哈希。收集器核对真实路径及所属目录，不跟随符号链接，不扫描兄弟目录。缺失保护路径、损坏状态或清理错误会记日志并停止本轮收集，业务进程继续运行。
- 未发布的下载暂存目录在更新任务结束后清理；过期版本内的半包随版本一起删除。保留目录里的 `.tmp`、`.part` 文件超过 24 小时才清理，避免干扰正在写入的文件。数据库检查副本在检查结束后删除。
- 主控自动数据库备份保留最近 3 份，agent 重新绑定备份保留最近 3 份，均按修改时间排序。手动备份不使用自动备份命名，不会被自动回收。数据库备份在完成写入后才原子发布，避免下次更新误用半份备份。
- 清理在启动健康确认后、更新任务结束后以及后台每小时执行。清理失败只告警并重试，不将成功更新变成失败，也不清空身份、配置、累计流量、周期计量点或当前待上报队列。

应用日志每份最多 10 MiB，保留当前文件和 3 份轮转文件，每个组件约 40 MiB 上限，单次大写入也受限制；重启继续追加现有日志。托管安装显式传入 `--log-dir`。不指定时仍输出到标准输出，便于开发或交给宿主日志系统；systemd 自身的服务消息仍由宿主 journal 管理。

更新下载以流式方式写盘，复用已有安装包和核心时也使用 64 KiB 分块校验，不将整个核心文件读入内存。该校验路径核对文件大小及完整 SHA-256，损坏文件仍须重新下载。

连接日志仍按面板的 7/30/90 天设置清理，累计流量保留。SQLite 删除记录后会复用空闲页，数据库文件不承诺立即缩小；自动维护不会为缩小文件而频繁执行全库重写。

## 回滚与卸载

Docker 主控的回滚是将 `.env` 中镜像标签改回上一已验证版本并重新创建容器；先核对数据库迁移兼容性，必要时停机恢复备份。Docker 卸载使用 `docker compose down`，默认保留卷；完整步骤见 [Docker 部署](deployment/docker.md)。以下主控卸载脚本仅用于原生安装。

更新回滚保留原有身份、资源和累计计数。主控只切回程序和下载包，不覆盖实时数据库；agent 使用原有持久化计数与配置恢复机制，保留回退所需配置快照。停止服务前仍先完成核心关闭和计数落盘。

使用 `--replace` 重新接入时，先停止旧 agent 并保存完整备份。同一主控、同一服务器 ID 仅更换接入凭据时，会继续使用原配置、累计计数、周期计量点、地址缓存及待确认报告，避免从零计数造成少计流量；绑定到另一服务器身份或主控时，从独立状态开始，旧数据仍留在备份中。运行锁和队列文件检查失败时，不移动或恢复外部文件。

主控卸载：

```sh
sudo sh /opt/hproxy/server/uninstall.sh
```

默认只停止服务、删除服务注册及主控程序目录，数据库、配置和备份保留。重新安装到相同数据目录会继续使用原数据库。确实需要清空全部主控数据时，在卸载时显式追加 `--purge-data`。自定义目录安装须同时传入原 `--root` 和 `--data`。卸载脚本核验安装、服务归属及真实路径，不删除 agent 子目录。

agent 通过面板删除服务器触发卸载，清理自己的服务、程序、核心、数据及内部备份；主控和其他应用目录保留。正常清理不删除宿主机共用的 journal。

OCI 容器 agent 由 Docker/Podman 管理，不具备上述原生安装器的远程自删除能力；先在宿主机停止、删除容器，再从面板移除离线记录。容器持久卷默认保留，只有确认身份与流量数据不再需要时才由宿主删除。
