# 子 agent 部署

agent 应安装在承载节点或转发端口的 **Linux 网络命名空间** 内。Debian、Ubuntu、CentOS/RHEL 衍生版、Alpine、LXC 和 NAT VPS 优先使用原生安装脚本；脚本自动识别 `amd64`、`arm64`、`armv7`，下载项目发布清单中的二进制，校验 SHA-256 后运行。systemd、OpenRC 或无 init 环境使用各自的启动方式。宿主机现有 sing-box/Realm 与项目核心独立，项目不会改写系统包。

## 原生安装

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

同一服务器重新绑定时，不要直接覆盖现有身份。先确认旧 agent 已停止，再用新一次性令牌追加 `--replace`；同一主控和服务器 ID 的更换会保留累计计数与周期计量点，其他绑定保留独立备份。详情见[文件与维护](../storage.md)。

## 主控失联时的本地指令

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

旧安装版本若没有 `agentctl`，可先从主控 HTTPS 站点获取 `https://panel.example.com/agentctl.sh` 与 `uninstall-agent.sh`，确认脚本内容后放入现有安装目录，或按[版本更新](../updates.md)升级。**不要**在无法验证目录归属时手动执行递归删除。

## Docker/Podman 容器 agent（可选）

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

容器移除后，面板中的服务器记录不会自动清理。请在面板使用“移除记录”并确认本地容器已停用；在线容器不能强制移除，以免出现孤立的业务进程。若主控暂时不可达，先保留卷和记录，恢复后再处理。
