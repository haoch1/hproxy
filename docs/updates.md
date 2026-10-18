# 版本更新

> **当前推荐方式**：Docker 主控通过 GitHub Actions 发布到 GHCR，运营者备份后使用 `docker compose pull` / `up -d` 更新；容器主控不提供进程内自替换或自动回滚。下面的面板内更新、监督脚本回滚与 `--update-source` 仅适用于单独安装的**原生托管主控**。详细容器步骤见 [Docker 部署](deployment/docker.md)，原生与容器 agent 的差异见 [agent 部署](deployment/agents.md)。

GitHub `main` 发布测试用 `edge` 镜像；`vX.Y.Z` 标签发布 `X.Y.Z`、`X.Y`、`latest` 镜像与 GitHub Release 原生安装包。原生 agent 从主控 `/downloads/` 读取与镜像匹配的兼容包并自行检查更新；容器 agent 应在宿主 Docker/Podman 中单独拉取和重建。主控镜像内的安装包为只读内容，不在运行时修改。

登录成功或刷新进入面板后在后台静默检查一次更新，切换页面不会重复检查；已有检查或更新任务时沿用当前任务。示例模式使用相同逻辑。设置页保留已有版本结果，后台检查不显示进度提示，结果变化后更新显示，也可点击“检查更新”手动检查。主控启动后和每 6 小时仍会检查一次。

每个有更新的组件均有独立的“更新”按钮，也可以使用“更新全部”。单独更新 hproxy 时同步主控和子 agent，保留现有核心；单独更新 sing-box 或 Realm 时只替换选中的核心，主控无需重启。核心仍然来自部署者的兼容发布清单，不直接跟随上游下载，Realm 使用带流量统计补丁的版本。

首次从旧版升级时，先更新 hproxy，等待所有托管子 agent 报告支持单组件更新，再单独更新核心；不满足条件的按钮会禁用并说明原因。原有“更新全部”仍可直接升级完整兼容包。发布者仍应提升 hproxy 版本并修改 `crates/common/src/lib.rs` 的默认核心版本，让新安装使用完整匹配版本。

`POST /api/v1/updates/apply` 接收发布清单的 `build` 和可选 `component`：`hproxy`、`sing_box`、`realm`、`all`，省略时为 `all`。服务端保留未选组件的文件、版本和 SHA-256，生成新的完整发布清单并原子发布；已安装的新版本不会降级。托管 agent 按经过校验的本地 `release.json` 读取实际核心版本，未托管安装仍使用编译时默认版本。替换前由候选 agent 校验版本和现有资源配置，替换后保留连接与核心健康检查及失败回退。核心更新会短暂重启该 agent 的代理服务。

## 启用在线更新

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

## 发布文件

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

## 更新和失败处理

主控先下载所有架构的 agent 与核心及当前架构的主控，检查文件名、大小、SHA-256、版本匹配，并在数据库副本上运行新主控检查。下载或校验失败会保留当前进程和当前安装包。

切换前将数据库完整备份到数据目录的 `backups/before-update-<build>.db`，写完后才发布备份文件，随后原子切换主控版本目录并重启。新主控监听成功并稳定运行 30 秒后发布子 agent 安装包，通知已连接的 agent 检查更新。离线 agent 重连后执行；子 agent 平时也每 6 小时检查一次。

每个子 agent 下载并校验新 agent 和该服务器已选择的核心，再用这些程序检查现有配置，保存计数器后切换。已卸载的核心不会因更新而重新安装。上线检查失败会回退上一版，失败构建不会反复自动安装。设置页展示 agent 上报的更新失败原因。

新主控启动或早期运行失败由监督进程恢复上一版程序与下载目录。数据库迁移必须向后兼容；自动回退不覆盖数据库中的新写入，必要时可在停止服务后手动恢复更新前备份。主控重启和 agent 核心重启期间会短暂断开连接。

健康检查完成后才回收不再需要的发布目录，保留当前版本和上一健康版本；主控的子 agent 下载包也保留当前和上一套。更新、待确认及回退过程中暂停收集，状态文件损坏时停止清理并记日志。自动备份各保留最近 3 份，手动备份不参与自动删除。清理失败不会使已确认的更新回退，后台每小时重试，详见[文件与维护](storage.md)。
