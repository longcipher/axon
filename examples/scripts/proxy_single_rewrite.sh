#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/proxy_single_rewrite.toml

cleanup_ports 8081 9001

# Start backend
python3 -m http.server 9001 --bind 127.0.0.1 >/dev/null 2>&1 &
backend_pid=$!
trap 'kill $backend_pid 2>/dev/null || true' EXIT
wait_port_listen 9001

# Start gateway
$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid $backend_pid 2>/dev/null || true' EXIT
wait_port_listen 8081
wait_http_ok http://127.0.0.1:8081/api 50 0.1 200 || { echo "[proxy_single] FAIL: gateway not ready"; exit 1; }

code1=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8081/api)
code2=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8081/api/)
if [[ "$code1" == "200" && "$code2" == "200" ]]; then
	echo "[proxy_single_rewrite] OK"
else
	echo "[proxy_single_rewrite] FAIL: code1=$code1 code2=$code2"; exit 1
fi
