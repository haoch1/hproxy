#!/bin/sh
# Read a one-time token from stdin so it never appears in docker/podman inspect.
set -eu
umask 077
[ "$#" -eq 1 ] || { echo "Usage: hproxy-agent-enroll https://panel.example.com < token-file" >&2; exit 2; }
token_file=$(mktemp)
trap 'rm -f "$token_file"' EXIT HUP INT TERM
cat > "$token_file"
[ -s "$token_file" ] || { echo "Enrollment token is empty" >&2; exit 2; }
set -- enroll --controller "$1" --token-file "$token_file" --data /var/lib/hproxy/agent
if [ -n "${HPROXY_CA_FILE:-}" ]; then
  set -- "$@" --ca "$HPROXY_CA_FILE"
fi
hproxy-agent "$@"
