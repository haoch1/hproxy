#!/bin/sh
# Initialize the persistent database without exposing the password as an
# environment variable or command-line argument.
set -eu
umask 077
[ "${1:-}" = init ] || { echo "Usage: pipe password into hproxy-server-init init" >&2; exit 2; }
password_file=$(mktemp)
trap 'rm -f "$password_file"' EXIT HUP INT TERM
cat > "$password_file"
[ -s "$password_file" ] || { echo "Administrator password is empty" >&2; exit 2; }
hproxy-server set-password --data /var/lib/hproxy/server --password-file "$password_file"
