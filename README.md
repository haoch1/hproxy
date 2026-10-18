# hproxy

hproxy 是面向 Linux 服务器的代理节点与端口转发控制台。主控提供 Web 面板和 SQLite 存储，子机 agent 主动连接主控，并运行项目管理的 sing-box 与 [Realm](vendor/realm/LICENSE) 补丁版。主控通过 Docker Compose 部署；agent 可在宿主机或 LXC 内原生安装，也可作为容器运行。

## 功能

- 管理 VLESS Reality、Shadowsocks 2022 节点及 TCP/UDP 转发。
- 按节点或规则统计流量，设置额度与重置周期。
- 同步服务器 IPv4/IPv6 变化，运行中重新解析落地域名；手动填写的地址保持不变。

镜像支持 Linux amd64、arm64、armv7，并内置对应的 agent 安装文件。agent 主动连接主控，无须开放管理通道的子机入站端口；节点和转发的业务端口仍需放行。

## 快速开始

在装有 Docker Engine 和 Compose 插件的 Linux 主机上：

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

管理员密码至少 12 字符。默认在本机 `127.0.0.1:19080` 监听，通过支持 WebSocket 的 HTTPS 反向代理对外提供面板。若端口被占用，修改 `.env` 中的 `HPROXY_PORT`。进入面板后在“服务器”页面添加子机，按面板给出的命令安装 agent。

`main` 更新后 GitHub Actions 自动构建 `latest` 镜像；升级前请备份数据，回滚时可使用对应提交的镜像标签。

## 文档

- [部署主控和 agent](docs/deployment.md)：HTTPS、Docker、Linux、LXC 与 Podman。
- [运维与数据](docs/operations.md)：状态、备份、更新、回滚和卸载。

源码位于 `crates/`、`web/`，项目补丁版 Realm 位于 `vendor/realm/`。构建流程见 [GitHub Actions](.github/workflows/ci.yml)。项目采用 [GPL-3.0-or-later](LICENSE)；第三方素材的许可声明保存在各自目录。
