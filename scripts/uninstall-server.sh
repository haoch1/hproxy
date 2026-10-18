#!/bin/sh
# Program removal preserves controller data unless --purge-data is explicit.
set -eu
root=/opt/hproxy/server data=/var/lib/hproxy/server purge=0
die() { printf 'hproxy: %s\n' "$*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --root|--data)
      [ "$#" -ge 2 ] || die "缺少 $1 的值"
      case "$1" in --root) root=$2;; --data) data=$2;; esac; shift 2;;
    --purge-data) purge=1; shift;;
    -h|--help) printf '%s\n' '用法: sh /opt/hproxy/server/uninstall.sh [--purge-data] [--root PATH --data PATH]'; exit 0;;
    *) die "未知参数: $1";;
  esac
done
[ "$(uname -s)" = Linux ] && [ "$(id -u)" = 0 ] || die '请在 Linux 上以 root 运行'
for path in "$root" "$data"; do
  case "$path" in /*) ;; *) die '目录必须是绝对路径';; esac
  case "$path" in *[!A-Za-z0-9_./-]*|/|/opt|/var|/var/lib|/opt/hproxy|/var/lib/hproxy|*/../*|*/..|*/./*|*/.|*/agent|*/) die '拒绝清理此目录';; esac
  [ ! -L "$path" ] || die '目录不能是符号链接'
  if [ -d "$path" ]; then [ "$(cd "$path" && pwd -P)" = "$path" ] || die '目录存在重定向'; fi
done
case "$root/" in "$data/"*) die '程序目录不能位于数据目录内';; esac
case "$data/" in "$root/"*) die '数据目录不能位于程序目录内';; esac
[ ! -e "$data/agent.json" ] || die '不能删除 agent 数据'
if [ -d "$root" ]; then
  [ -f "$root/supervise.sh" ] && [ -f "$root/current/hproxy-server" ] || die '无法确认主控安装归属'
  grep -F "'$root/current/hproxy-server' run --data '$data'" "$root/supervise.sh" >/dev/null || die '主控路径不匹配'
  [ ! -e "$root/current/hproxy-agent" ] || die '不能卸载 agent 目录'
  unit=/etc/systemd/system/hproxy-server.service
  if [ -e "$unit" ]; then
    grep -Fx "ExecStart=$root/supervise.sh" "$unit" >/dev/null || die '服务不属于此安装'
    systemctl disable --now hproxy-server.service
    rm -f "$unit"; systemctl daemon-reload
  elif [ -f "$data/supervisor.pid" ]; then
    pid=$(cat "$data/supervisor.pid")
    case "$pid" in ''|*[!0-9]*) die '监督进程 PID 无效';; esac
    if [ -d "/proc/$pid" ]; then
      tr '\000' '\n' < "/proc/$pid/cmdline" | grep -Fx "$root/supervise.sh" >/dev/null || die '监督进程不属于此安装'
      kill "$pid"
      attempt=0
      while [ -f "$data/supervisor.pid" ] && [ "$attempt" -lt 25 ]; do sleep 1; attempt=$((attempt+1)); done
      [ ! -f "$data/supervisor.pid" ] || die '主控尚未停止'
    fi
  fi
  rm -rf -- "$root"
elif [ "$purge" = 1 ]; then
  die '程序目录不存在，无法核验数据归属；未删除数据'
fi
if [ "$purge" = 1 ]; then
  rm -rf -- "$data"
  printf '%s\n' '主控程序与数据已卸载'
else printf '主控程序已卸载，数据库、配置和备份保留在 %s\n' "$data"; fi
