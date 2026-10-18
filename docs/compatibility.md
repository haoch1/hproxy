# 兼容性与部署边界

## 支持目标

| 环境 | 建议部署方式 | 要点 |
| --- | --- | --- |
| Debian/Ubuntu | 主控 Docker Compose；agent 原生 | systemd 自动注册；需 HTTPS、`curl`/`wget`、`sha256sum` |
| RHEL/CentOS/AlmaLinux/Rocky 等 | 主控 Docker Compose；agent 原生 | 发行版版本、内核及容器运行时由运营者验证；检查 SELinux 与防火墙规则 |
| Alpine | agent 原生 | OpenRC；发布包采用静态 musl，`sha256sum` 可由 BusyBox 提供 |
| LXC、NAT VPS | 容器内原生 agent | 无 systemd/OpenRC 时使用项目监督进程；业务端口必须获得宿主映射 |
| Docker/Podman 宿主 | 可选容器 agent | host network、持久卷、容器生命周期由宿主管理；见[agent 部署](deployment/agents.md) |

三种发布架构为 Linux `x86_64/amd64`、`aarch64/arm64`、`armv7l/armv7`。主控镜像的构建清单与 agent/核心包采用静态 musl 二进制；这降低 glibc/musl 差异，但**不保证**任意旧内核、受限系统调用、32 位容器、SELinux 策略或所有云厂商 NAT 组合可用。安装前核对 `uname -m`、容器架构和业务端口权限。此仓库不发布 Windows/macOS 服务端。

## 网络与容器限制

- agent 管理通道由子机向主控发起 HTTPS/WSS。反向代理或 CDN 需允许 WebSocket、SSE 和较大的 `/downloads/` 文件，不得缓存含身份或管理 API 的响应。主控公网 URL 必须与证书域名一致。
- 容器 agent 的 `--network host` 使用宿主网络命名空间；rootless Podman、LXC nesting 和用户命名空间的权限由宿主决定。低于 1024 的端口、IPv6 绑定、原始网络能力和 `host` 模式在这些环境中可能受限。不能使用 host network 时需为每个业务端口及协议显式映射，且动态配置要同步更新映射。
- NAT VPS 的公网端口映射不由 hproxy 自动申请。面板上的 IPv4/IPv6 可能是容器自身地址；应按提供商映射填写可被客户端访问的地址和端口。IPv4 到 IPv6 等跨地址族转发需两端的网络栈和路由均可用。
- 指定落地域名由项目补丁版 Realm 在运行中重新解析，正向缓存最长约 30 秒、失败后约 5 秒重试；已有 TCP 会话仍使用已建立连接。主控不能替运营者修改 DNS 记录。
- 如果宿主已有 sing-box/Realm，项目仍使用自己的程序和目录；但**相同监听地址与端口会冲突**。安装时不会删除、更新、复用系统程序。

## 容量与版本

内存与磁盘占用受服务器数量、同时连接数、连接日志保留期、离线队列及核心进程影响；仓库不把未经测量的固定值写成最低配置。曾在 128 MiB 的 Alpine NAT 环境分开验证两个核心，但这不能证明所有节点/转发并发场景都适合 128 MiB。部署前在自己的最小目标机器上观察 `docker stats hproxy`、`ps`/`free -m`、数据目录和 Docker 卷使用量，给 SQLite 备份与镜像升级预留额外空间。Linux 真机已验证范围和限制见[测试说明](testing.md)。

GitHub Actions 发布 `edge`（主分支）与语义版本标签；生产环境固定版本、先备份后升级。Docker 主控与 agent 原生更新属于不同机制，不能把镜像自动拉取视作子机已经更新。发布清单哈希保护下载完整性，传输依赖 HTTPS；管理自己的发行版签名、镜像签名或私有镜像仓库时需另行制定供应链策略。
