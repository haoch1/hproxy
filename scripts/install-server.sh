#!/bin/sh
# Install a verified, locally unpacked compatibility release on the controller.
set -eu
umask 077
release='' public_url='' source='' password_file='' update_ca='' root=/opt/hproxy/server data=/var/lib/hproxy/server listen=127.0.0.1:19080
die() { printf 'hproxy: %s\n' "$*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --release-dir|--public-url|--update-source|--update-ca|--password-file|--root|--data|--listen)
      [ "$#" -ge 2 ] || die "缺少 $1 的值"
      case "$1" in
        --release-dir) release=$2;; --public-url) public_url=${2%/};; --update-source) source=${2%/};;
        --password-file) password_file=$2;; --root) root=$2;; --data) data=$2;; --listen) listen=$2;;
        --update-ca) update_ca=$2;;
      esac; shift 2;;
    -h|--help) printf '%s\n' '用法: sh install-server.sh --release-dir /path/to/release --public-url https://panel.example.com --update-source https://releases.example.com/hproxy/stable --password-file /root/hproxy-password'; exit 0;;
    *) die "未知参数: $1";;
  esac
done
[ "$(uname -s)" = Linux ] && [ "$(id -u)" = 0 ] || die '主控安装脚本请在 Linux 上以 root 运行'
[ -d "$release" ] && [ -n "$public_url" ] && [ -n "$source" ] || die '缺少发布包、面板公网地址或更新源'
[ -z "$update_ca" ] || { [ -f "$update_ca" ] && [ -r "$update_ca" ]; } || die '无法读取更新源 CA 证书文件'
[ -f "$release/uninstall-server.sh" ] || die '发布包缺少主控卸载脚本'
case "$public_url" in https://*) ;; *) die '公网地址必须使用 HTTPS，请先配置反向代理';; esac
case "$source" in https://*) ;; *) die '更新源必须使用 HTTPS';; esac
case "$source$public_url" in *[\'\"\`\ \;\|\&\$\?\#]*) die '地址格式无效';; esac
for path in "$root" "$data"; do
  case "$path" in /*) ;; *) die '安装与数据目录必须为绝对路径';; esac
  case "$path" in *[!A-Za-z0-9_./-]*|/|/opt|/var|/var/lib|/opt/hproxy|/var/lib/hproxy|*/../*|*/..|*/./*|*/.|*/) die '安装与数据目录格式无效';; esac
  [ ! -L "$path" ] || die '安装与数据目录不能是符号链接'
done
case "$root/" in "$data/"*) die '程序目录不能位于数据目录内';; esac
case "$data/" in "$root/"*) die '数据目录不能位于程序目录内';; esac
case "$listen" in *[!A-Za-z0-9.:\[\]-]*) die '监听地址格式无效';; esac
[ ! -e "$root/current" ] || die '主控已安装，请在面板设置中更新'
case "$(uname -m)" in x86_64|amd64) arch=linux-amd64;; aarch64|arm64) arch=linux-arm64;; armv7l|armv7) arch=linux-armv7;; *) die '不支持当前架构';; esac
command -v sha256sum >/dev/null 2>&1 || die '需要 sha256sum'
verify() {
  platform=$1 count=0
  while read -r hash file extra; do
    [ -z "${extra:-}" ] && [ "${#hash}" = 64 ] || die '校验清单无效'
    case "$hash" in *[!a-fA-F0-9]*) die '校验值无效';; esac
    case "$file" in "hproxy-agent-$platform"|"sing-box-$platform"|"realm-$platform") ;; *) die '文件名无效';; esac
    count=$((count+1))
  done < "$release/SHA256SUMS-$platform"
  [ "$count" = 3 ] || die '缺少子 agent 安装文件'
  (cd "$release" && sha256sum -c "SHA256SUMS-$platform") || die '文件校验失败'
  [ -f "$release/manifest-$platform.json" ] || die '缺少发布清单'
}
for platform in linux-amd64 linux-arm64 linux-armv7; do verify "$platform"; done
[ "$(wc -l < "$release/SHA256SUMS-server-$arch" | tr -d ' ')" = 1 ] || die '主控校验清单无效'
read -r server_hash server_file extra < "$release/SHA256SUMS-server-$arch"
[ "$server_file" = "hproxy-server-$arch" ] && [ -z "${extra:-}" ] || die '主控文件名无效'
(cd "$release" && sha256sum -c "SHA256SUMS-server-$arch") || die '主控校验失败'
build=$(sha256sum "$release/SHA256SUMS-$arch" | cut -d ' ' -f 1)
next="$root/releases/$build" downloads="$root/downloads"
mkdir -p "$next" "$data/logs" "$data/backups" "$downloads/releases/$build"
for path in "$root" "$data"; do [ "$(cd "$path" && pwd -P)" = "$path" ] || die '安装目录存在重定向'; done
chmod 700 "$root" "$root/releases" "$next" "$data" "$downloads"
chmod 700 "$data/logs" "$data/backups"
update_ca_arg=''
if [ -n "$update_ca" ]; then
  # Keep the service independent of the original certificate file's location.
  cp "$update_ca" "$data/update-ca.crt"; chmod 600 "$data/update-ca.crt"
  update_ca_arg=" --update-ca '$data/update-ca.crt'"
fi
cp "$release/uninstall-server.sh" "$root/uninstall.sh"; chmod 700 "$root/uninstall.sh"
cp "$release/hproxy-server-$arch" "$next/hproxy-server"; chmod 700 "$next/hproxy-server"
for platform in linux-amd64 linux-arm64 linux-armv7; do
  for file in "manifest-$platform.json" "SHA256SUMS-$platform" "hproxy-agent-$platform" "sing-box-$platform" "realm-$platform"; do
    cp "$release/$file" "$downloads/releases/$build/$file"
  done
done
if [ ! -f "$data/hproxy.db" ]; then
  [ -f "$password_file" ] || die '首次安装需 --password-file 指定管理员密码文件'
  "$next/hproxy-server" set-password --data "$data" --password-file "$password_file"
fi
"$next/hproxy-server" check --data "$data"
ln -s "$downloads/releases/$build" "$downloads/current"
ln -s "$next" "$root/current"
cp "$next/hproxy-server" "$root/bootstrap"; chmod 700 "$root/bootstrap"
cat > "$root/supervise.sh" <<EOF
#!/bin/sh
umask 077
cd '$data' || exit 1
echo \$\$ > '$data/supervisor.pid'
child=''
trap 'trap - TERM INT; [ -z "\$child" ] || kill "\$child" 2>/dev/null; [ -z "\$child" ] || wait "\$child"; rm -f "$data/supervisor.pid"; exit 0' TERM INT
while :; do
  '$root/current/hproxy-server' run --data '$data' --downloads '$downloads' --update-root '$root' --update-source '$source'$update_ca_arg --public-url '$public_url' --listen '$listen' --log-dir '$data/logs' &
  child=\$!; wait "\$child"; code=\$?; child=''
  [ "\$code" = 75 ] || '$root/bootstrap' recover --update-root '$root' --log-dir '$data/logs'
  sleep 3
done
EOF
chmod 700 "$root/supervise.sh"
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  cat > /etc/systemd/system/hproxy-server.service <<EOF
[Unit]
Description=hproxy controller
After=network-online.target
Wants=network-online.target
[Service]
ExecStart=$root/supervise.sh
WorkingDirectory=$data
Restart=always
RestartSec=3
TimeoutStopSec=20
UMask=0077
LimitNOFILE=65535
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=$root $data
ProtectHome=true
[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload; systemctl enable --now hproxy-server
else
  nohup "$root/supervise.sh" </dev/null >/dev/null 2>&1 &
fi
printf '安装完成，主控监听 %s。请将 HTTPS 反向代理指向此地址。\n' "$listen"
