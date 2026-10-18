#!/bin/sh
# Local recovery controls for installer-managed native Linux agents.
set -eu
root=$(CDPATH= cd "$(dirname "$0")" && pwd -P)
case "$root" in
  /opt/hproxy/agent) data=/var/lib/hproxy/agent; system_install=1 ;;
  "$HOME/.local/share/hproxy/agent") data="$HOME/.local/state/hproxy/agent"; system_install=0 ;;
  *) echo "hproxy: unsupported agent installation path: $root" >&2; exit 1 ;;
esac
die() { printf 'hproxy: %s\n' "$*" >&2; exit 1; }
[ -f "$root/supervise.sh" ] && [ -f "$root/current/hproxy-agent" ] || die "agent installation is incomplete"

service_kind() {
  if [ "$system_install" = 1 ] && [ -f /etc/systemd/system/hproxy-agent.service ]; then
    grep -Fx "ExecStart=$root/supervise.sh" /etc/systemd/system/hproxy-agent.service >/dev/null || die "systemd service belongs to another installation"
    command -v systemctl >/dev/null 2>&1 || die "systemd service exists but systemctl is unavailable"
    printf systemd
  elif [ "$system_install" = 1 ] && [ -f /etc/init.d/hproxy-agent ]; then
    grep -Fx "command=\"$root/supervise.sh\"" /etc/init.d/hproxy-agent >/dev/null || die "OpenRC service belongs to another installation"
    command -v rc-service >/dev/null 2>&1 || die "OpenRC service exists but rc-service is unavailable"
    printf openrc
  else
    printf supervisor
  fi
}
owned_pid() {
  [ -f "$data/supervisor.pid" ] || return 1
  pid=$(cat "$data/supervisor.pid")
  case "$pid" in ''|*[!0-9]*) return 1;; esac
  [ -d "/proc/$pid" ] || return 1
  tr '\000' '\n' < "/proc/$pid/cmdline" | grep -Fx "$root/supervise.sh" >/dev/null
}
supervisor_start() {
  if owned_pid; then die "agent is already running (PID $pid)"; fi
  rm -f "$data/supervisor.pid"
  nohup "$root/supervise.sh" </dev/null >/dev/null 2>&1 &
  printf 'hproxy agent supervisor started (PID %s)\n' "$!"
}
supervisor_stop() {
  if ! owned_pid; then
    [ ! -f "$data/supervisor.pid" ] || die "supervisor PID does not belong to this agent"
    printf '%s\n' "hproxy agent is not running"
    return
  fi
  kill "$pid"
  attempt=0
  while [ -f "$data/supervisor.pid" ] && [ "$attempt" -lt 25 ]; do
    sleep 1; attempt=$((attempt+1))
  done
  [ ! -f "$data/supervisor.pid" ] || die "agent did not stop"
  printf '%s\n' "hproxy agent stopped"
}
kind=$(service_kind)
case "${1:-}" in
  status)
    case "$kind" in
      systemd) systemctl status hproxy-agent.service --no-pager ;;
      openrc) rc-service hproxy-agent status ;;
      supervisor) if owned_pid; then printf 'hproxy agent running (PID %s)\n' "$pid"; else die "agent is not running"; fi ;;
    esac ;;
  start)
    case "$kind" in
      systemd) systemctl start hproxy-agent.service ;;
      openrc) rc-service hproxy-agent start ;;
      supervisor) supervisor_start ;;
    esac ;;
  stop)
    case "$kind" in
      systemd) systemctl stop hproxy-agent.service ;;
      openrc) rc-service hproxy-agent stop ;;
      supervisor) supervisor_stop ;;
    esac ;;
  restart|update)
    case "$kind" in
      systemd) systemctl restart hproxy-agent.service ;;
      openrc) rc-service hproxy-agent restart ;;
      supervisor) supervisor_stop; supervisor_start ;;
    esac
    if [ "$1" = update ]; then
      printf '%s\n' "agent restarted; it checks the controller's HTTPS release manifest after about 60 seconds."
      printf '%s\n' "The current version remains in place if the controller is unavailable or the package fails validation."
    fi ;;
  check)
    "$root/current/hproxy-agent" check --data "$data" --core-dir "$root/current" ;;
  logs)
    if [ -f "$data/logs/hproxy-agent.log" ]; then tail -n 100 -f "$data/logs/hproxy-agent.log"
    elif [ "$kind" = systemd ]; then journalctl -u hproxy-agent.service -n 100 -f
    else die "no agent log found"; fi ;;
  uninstall)
    shift
    exec sh "$root/uninstall.sh" "$@" ;;
  *)
    printf '%s\n' "Usage: $root/agentctl {status|start|stop|restart|update|check|logs|uninstall [--purge-data]}" >&2
    exit 2 ;;
esac
