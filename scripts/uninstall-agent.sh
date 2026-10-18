#!/bin/sh
# Local fallback removal. Data is preserved unless --purge-data is explicit.
set -eu
root=$(CDPATH= cd "$(dirname "$0")" && pwd -P)
case "$root" in
  /opt/hproxy/agent) data=/var/lib/hproxy/agent; system_install=1 ;;
  "$HOME/.local/share/hproxy/agent") data="$HOME/.local/state/hproxy/agent"; system_install=0 ;;
  *) echo "hproxy: unsupported agent installation path: $root" >&2; exit 1 ;;
esac
die() { printf 'hproxy: %s\n' "$*" >&2; exit 1; }
case "${1:-}" in
  "") purge=0 ;;
  --purge-data) purge=1 ;;
  *) die "Usage: $root/uninstall.sh [--purge-data]" ;;
esac
[ "$#" -le 1 ] || die "too many arguments"
[ -f "$root/supervise.sh" ] && [ -f "$root/current/hproxy-agent" ] || die "cannot verify agent installation"
[ -f "$data/agent.json" ] && [ ! -e "$data/hproxy.db" ] || die "cannot verify agent data"
[ ! -L "$root" ] && [ ! -L "$data" ] || die "installation paths cannot be symlinks"
[ "$(cd "$data" && pwd -P)" = "$data" ] || die "data path is redirected"
[ ! -e "$root/current/hproxy-server" ] || die "refusing to remove a controller"
if [ "$system_install" = 1 ]; then
  [ "$(id -u)" = 0 ] || die "run as root"
fi
if [ "$system_install" = 1 ] && [ -f /etc/systemd/system/hproxy-agent.service ]; then
  grep -Fx "ExecStart=$root/supervise.sh" /etc/systemd/system/hproxy-agent.service >/dev/null ||
    die "systemd service belongs to another installation"
fi
if [ "$system_install" = 1 ] && [ -f /etc/init.d/hproxy-agent ]; then
  grep -Fx "command=\"$root/supervise.sh\"" /etc/init.d/hproxy-agent >/dev/null ||
    die "OpenRC service belongs to another installation"
fi
sh "$root/agentctl" stop
if [ "$system_install" = 1 ] && [ -f /etc/systemd/system/hproxy-agent.service ]; then
  systemctl disable hproxy-agent.service
  rm -f /etc/systemd/system/hproxy-agent.service
  systemctl daemon-reload
fi
if [ "$system_install" = 1 ] && [ -f /etc/init.d/hproxy-agent ]; then
  rc-update del hproxy-agent default
  rm -f /etc/init.d/hproxy-agent
fi
rm -rf -- "$root"
if [ "$purge" = 1 ]; then
  rm -rf -- "$data"
  printf '%s\n' "hproxy agent program and data removed"
else
  printf 'hproxy agent program removed; identity, counters and backups remain in %s\n' "$data"
fi
