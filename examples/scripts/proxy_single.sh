#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/proxy_single.toml

cleanup_ports 8081 9001

# Create temp dir for backend
TMP_DIR=$(mktemp -d)
mkdir -p "$TMP_DIR/api"
echo "hello from backend" > "$TMP_DIR/api/hello"

# Start backend
(cd "$TMP_DIR" && python3 -m http.server 9001 --bind 127.0.0.1) >/dev/null 2>&1 &
backend_pid=$!
trap 'kill $backend_pid 2>/dev/null || true; rm -rf "$TMP_DIR"' EXIT
wait_port_listen 9001

# Start gateway
$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid $backend_pid 2>/dev/null || true; rm -rf "$TMP_DIR"' EXIT
wait_port_listen 8081

# Test
resp=$(curl -s http://127.0.0.1:8081/api/hello)
if [[ "$resp" == "hello from backend"* ]]; then
	echo "[proxy_single] OK"
else
	echo "[proxy_single] FAIL: expected 'hello from backend', got '$resp'"
	exit 1
fi
