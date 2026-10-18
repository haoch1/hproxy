#!/bin/sh
# hproxy child installer. Served by the controller at /install.sh.
set -eu
umask 077
endpoint='' token='' token_file='' ca='' user_mode=0 replace=0 allow_local=0
usage() { printf '%s\n' '用法: sh install-agent.sh -e https://panel.example.com {-t TOKEN|--token-file FILE} [--ca FILE] [--user] [--replace]'; }
die() { printf 'hproxy: %s\n' "$*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    -e|--endpoint) [ "$#" -ge 2 ] || die '缺少主控地址'; endpoint=${2%/}; shift 2 ;;
    -t|--token) [ "$#" -ge 2 ] || die '缺少安装凭据'; token=$2; shift 2 ;;
    --token-file) [ "$#" -ge 2 ] || die '缺少安装凭据文件'; token_file=$2; shift 2 ;;
    --ca) [ "$#" -ge 2 ] || die '缺少 CA 证书文件'; ca=$2; shift 2 ;;
    --user) user_mode=1; shift ;;
    --replace) replace=1; shift ;;
    --allow-local-http) allow_local=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "未知参数: $1" ;;
  esac
done
if [ -n "$token_file" ]; then
  [ -z "$token" ] || die '安装凭据与凭据文件不能同时指定'
  [ -f "$token_file" ] && [ -r "$token_file" ] || die '无法读取安装凭据文件'
  IFS= read -r token < "$token_file" || [ -n "$token" ] || die '安装凭据文件为空'
fi
[ -z "$ca" ] || { [ -f "$ca" ] && [ -r "$ca" ]; } || die '无法读取 CA 证书文件'
[ -n "$endpoint" ] && [ -n "$token" ] || { usage; exit 1; }
case "$endpoint" in
  https://*) ;;
  http://127.0.0.1:*|http://localhost:*) [ "$allow_local" = 1 ] || die 'HTTP 只供显式启用的本机测试' ;;
  *) die '主控地址必须为 HTTPS' ;;
esac
case "$endpoint" in *[\'\"\`\ \;\|\&\$]*) die '主控地址无效';; esac
case "$token" in *[!A-Za-z0-9_-]*) die '安装凭据格式无效';; esac
[ "$(uname -s)" = Linux ] || die '子 agent 安装器仅支持 Linux'
case "$(uname -m)" in x86_64|amd64) arch=linux-amd64;; aarch64|arm64) arch=linux-arm64;; armv7l|armv7) arch=linux-armv7;; *) die "暂不支持的架构: $(uname -m)";; esac
fetch() {
  if command -v curl >/dev/null 2>&1; then
    if [ -n "$ca" ]; then curl -fSL --retry 2 --connect-timeout 15 --max-time 300 --cacert "$ca" "$1" -o "$2"
    else curl -fSL --retry 2 --connect-timeout 15 --max-time 300 "$1" -o "$2"; fi
  elif command -v wget >/dev/null 2>&1; then
    if [ -n "$ca" ]; then wget --help 2>&1 | grep -q -- '--ca-certificate' || die '此 wget 不支持指定 CA，请安装 curl'; wget -q --ca-certificate="$ca" -O "$2" "$1"
    else wget -q -O "$2" "$1"; fi
  else die '需要 curl 或 wget'; fi
}
command -v sha256sum >/dev/null 2>&1 || die '需要 sha256sum（Alpine BusyBox 已内置）'
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
trap 'exit 130' HUP INT TERM
if [ "$(id -u)" != 0 ] && [ "$user_mode" != 1 ]; then
  command -v sudo >/dev/null 2>&1 || die '请使用 root 执行，或在命令末尾添加 --user'
  fetch "$endpoint/install.sh" "$work/install.sh"
  printf '%s' "$token" > "$work/token"
  set -- -e "$endpoint" --token-file "$work/token"
  if [ "$replace" = 1 ]; then set -- "$@" --replace; fi
  if [ "$allow_local" = 1 ]; then set -- "$@" --allow-local-http; fi
  if [ -n "$ca" ]; then set -- "$@" --ca "$ca"; fi
  sudo sh "$work/install.sh" "$@"
  exit $?
fi
if [ "$user_mode" = 1 ]; then
  root="$HOME/.local/share/hproxy/agent"; data="$HOME/.local/state/hproxy/agent"
else root=/opt/hproxy/agent; data=/var/lib/hproxy/agent; fi
if [ -f "$data/agent.json" ] && [ "$replace" != 1 ]; then die '此服务器已经安装，agent 和核心会自动更新。重新绑定请在命令末尾添加 --replace'; fi
printf '%s\n' "正在下载 $arch 的子 agent…"
fetch "$endpoint/downloads/SHA256SUMS-$arch" "$work/sums"
fetch "$endpoint/agentctl.sh" "$work/agentctl.sh"
fetch "$endpoint/uninstall-agent.sh" "$work/uninstall-agent.sh"
count=0 seen='|' agent_hash=''
while read -r hash file extra; do
  [ -z "${extra:-}" ] && [ "${#hash}" = 64 ] || die '安装包校验清单无效'
  case "$hash" in *[!a-fA-F0-9]*) die '安装包校验清单无效';; esac
  case "$file" in "hproxy-agent-$arch"|"sing-box-$arch"|"realm-$arch") ;; *) die '安装包文件名无效';; esac
  case "$seen" in *"|$file|"*) die '安装包清单有重复文件';; esac
  seen="$seen$file|"
  if [ "$file" = "hproxy-agent-$arch" ]; then agent_hash=$hash; fetch "$endpoint/downloads/$file" "$work/$file"; fi
  count=$((count+1))
done < "$work/sums"
[ "$count" = 3 ] || die '安装包缺少 agent 或核心'
[ -n "$agent_hash" ] || die '安装包缺少 agent'
(cd "$work" && printf '%s  %s\n' "$agent_hash" "hproxy-agent-$arch" | sha256sum -c -) || die '安装包校验失败'
build=$(sha256sum "$work/sums" | cut -d ' ' -f 1)
release="$root/releases/$build"
mkdir -p "$release" "$data"
chmod 700 "$root" "$root/releases" "$release" "$data"
if [ -f "$release/hproxy-agent" ]; then
  installed_hash=$(sha256sum "$release/hproxy-agent" | cut -d ' ' -f 1)
  [ "$installed_hash" = "$agent_hash" ] || die '已有同版本 agent 校验失败，已保留当前安装'
else cp "$work/hproxy-agent-$arch" "$release/hproxy-agent"; fi
chmod 700 "$release/hproxy-agent"
"$release/hproxy-agent" check --data "$work/identity" --core-dir "$release"
printf '%s' "$token" > "$work/token"
set -- enroll --controller "$endpoint" --token-file "$work/token" --data "$work/identity"
if [ "$allow_local" = 1 ]; then set -- "$@" --allow-local-http; fi
if [ -n "$ca" ]; then set -- "$@" --ca "$ca"; fi
"$release/hproxy-agent" "$@"
if [ -f "$data/agent.json" ]; then
  if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ] && [ "$user_mode" = 0 ]; then systemctl stop hproxy-agent
  elif command -v rc-service >/dev/null 2>&1 && [ "$user_mode" = 0 ]; then rc-service hproxy-agent stop
  elif [ -f "$data/supervisor.pid" ]; then
    supervisor_pid=$(cat "$data/supervisor.pid")
    case "$supervisor_pid" in ''|*[!0-9]*) die '监督进程 PID 无效';; esac
    if [ -d "/proc/$supervisor_pid" ]; then
      tr '\000' '\n' < "/proc/$supervisor_pid/cmdline" | grep -Fx "$root/supervise.sh" >/dev/null || die '监督进程不属于当前安装'
      kill "$supervisor_pid"
      attempt=0
      while [ -f "$data/supervisor.pid" ] && [ "$attempt" -lt 25 ]; do sleep 1; attempt=$((attempt+1)); done
      [ ! -f "$data/supervisor.pid" ] || die '旧 agent 尚未停止，已取消重新绑定'
    else rm -f "$data/supervisor.pid"; fi
  fi
  backup=$("$release/hproxy-agent" backup-data --data "$data" --identity "$work/identity/agent.json") || die '旧数据尚未完成备份，已取消重新绑定'
  printf '旧配置已备份: %s\n' "$backup"
fi
cp "$work/identity/agent.json" "$data/agent.json"; chmod 600 "$data/agent.json"
ln -sfn "$release" "$root/current"
[ -f "$root/bootstrap" ] || cp "$release/hproxy-agent" "$root/bootstrap"
chmod 700 "$root/bootstrap"
cp "$work/agentctl.sh" "$root/agentctl"
cp "$work/uninstall-agent.sh" "$root/uninstall.sh"
chmod 700 "$root/agentctl" "$root/uninstall.sh"
cat > "$root/supervise.sh" <<EOF
#!/bin/sh
umask 077
cd '$data' || exit 1
echo \$\$ > '$data/supervisor.pid'
child=''
trap 'trap - TERM INT; [ -z "\$child" ] || kill "\$child" 2>/dev/null; [ -z "\$child" ] || wait "\$child"; rm -f "$data/supervisor.pid"; exit 0' TERM INT
while :; do
  '$root/current/hproxy-agent' run --data '$data' --core-dir '$root/current' --update-root '$root' --log-dir '$data/logs' &
  child=\$!; wait "\$child"; code=\$?; child=''
  '$root/bootstrap' recover --data '$data' --update-root '$root' --code "\$code" --log-dir '$data/logs'
  sleep 3
done
EOF
chmod 700 "$root/supervise.sh"
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ] && [ "$user_mode" = 0 ]; then
  cat > /etc/systemd/system/hproxy-agent.service <<EOF
[Unit]
Description=hproxy child agent
After=network-online.target
Wants=network-online.target
[Service]
ExecStart=$root/supervise.sh
WorkingDirectory=$data
Restart=always
RestartSec=3
TimeoutStopSec=20
LimitNOFILE=65535
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=$root $data
ProtectHome=true
UMask=0077
[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload; systemctl enable --now hproxy-agent
elif command -v rc-service >/dev/null 2>&1 && [ "$user_mode" = 0 ]; then
  cat > /etc/init.d/hproxy-agent <<EOF
#!/sbin/openrc-run
description="hproxy child agent"
supervisor="supervise-daemon"
command="$root/supervise.sh"
respawn_delay=3
depend() { need net; }
EOF
  chmod 755 /etc/init.d/hproxy-agent; rc-update add hproxy-agent default; rc-service hproxy-agent start
else
  nohup "$root/supervise.sh" </dev/null >/dev/null 2>&1 &
  printf '无 init 环境已后台启动；请按所在系统配置开机执行: %s\n' "$root/supervise.sh"
fi
printf '%s\n' '安装完成，正在连接主控，核心将按面板选择自动安装，后续可在服务器设置中安装或卸载'
