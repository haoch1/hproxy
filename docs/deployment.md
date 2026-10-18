# 部署

## 主控：Docker Compose

需要 Linux、Docker Engine 与 Compose 插件，以及指向主控的 HTTPS 域名。按 [README](../README.md#快速开始) 初始化密码并启动。反向代理需支持 WebSocket、SSE 和 `/downloads/` 文件下载。默认只将主控的 `19080` 端口绑定到宿主机回环地址；更改端口只需设置 `.env` 中的 `HPROXY_PORT`。镜像由 `main` 自动构建，无须另行下载发布包。

```sh
docker compose ps
curl -fsS http://127.0.0.1:19080/healthz
docker compose logs --tail 100 hproxy-server
```

主控容器和数据卷分别为 `hproxy-server`、`hproxy-server-data`。主控持久数据在容器内的 `/var/lib/hproxy/server`，更新容器不会删除数据卷。安装令牌、密码和备份文件均为敏感数据，不应公开。

## 子机：原生 Linux agent

在面板“服务器”页面创建服务器，在目标 Linux 机器执行面板提供的一次性安装命令。agent 使用原生二进制，不需要在子机安装 Docker。它主动经 HTTPS/WebSocket 连接主控，管理通道无须开放子机入站端口；节点和转发的业务端口仍需放行。安装支持 amd64、arm64、armv7；脚本按可用环境使用 systemd、OpenRC 或自身监督进程。

原生安装的程序与数据分开：`/opt/hproxy/agent` 存放程序及核心，`/var/lib/hproxy/agent` 存放身份、配置、流量和日志。使用 `--user` 安装时，分别位于 `~/.local/share/hproxy/agent` 和 `~/.local/state/hproxy/agent`。项目核心与系统中已有的 sing-box、Realm 文件独立，但相同监听端口仍会冲突。

主控失联时可在子机本地操作：

```sh
sudo sh /opt/hproxy/agent/agentctl status
sudo sh /opt/hproxy/agent/agentctl check
sudo sh /opt/hproxy/agent/agentctl logs
sudo sh /opt/hproxy/agent/agentctl restart
sudo sh /opt/hproxy/agent/agentctl update
sudo sh /opt/hproxy/agent/agentctl uninstall
```

`update` 会触发检查主控提供的兼容包；主控不可达时继续使用当前版本。卸载默认保留身份和统计数据；确认不再需要时可显式加 `--purge-data`。普通用户安装需改用其实际 `agentctl` 路径、以该用户执行。

LXC 与 NAT VPS 在各自的 Linux 系统内直接安装二进制 agent，并按提供商规则映射业务端口。无 systemd/OpenRC 时，安装脚本会启动自身监督进程；重启后的自启方式需按宿主环境配置。

## 支持范围

CI 构建并验证三种 Linux 架构的主控镜像和原生 agent 二进制，在 Linux 上测试主控、agent、Realm 及主控容器启动。Debian/Ubuntu、RHEL 系和 Alpine 使用对应服务管理方式；受限 LXC、旧内核、NAT 映射和 IPv6 公网连通性还需在目标环境验证。此项目不提供 Windows/macOS 服务端。落地使用域名时 Realm 会在运行中重新解析；公网 IP 变化不会自动修改运营商 DNS 记录。
