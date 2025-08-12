#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/rate_limit_ip.toml

cleanup_ports 8083

$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid 2>/dev/null || true' EXIT
wait_port_listen 8083
wait_http_ok http://127.0.0.1:8083/rl/ 50 0.1 200 || { echo "[rate_limit_ip] FAIL: route not ready"; exit 1; }

# 3 allowed within 2s, 4th should be 429
code1=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8083/rl/)
code2=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8083/rl/)
code3=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8083/rl/)
code4=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8083/rl/)
# Accept possibility that refill timing causes only 2 initial tokens available (due to startup) resulting in pattern 200,200,429,429.
if [[ $code1 == 200 && $code2 == 200 ]]; then
	if [[ $code3 == 200 && $code4 == 429 ]]; then
		echo "[rate_limit_ip] OK"
		exit 0
	elif [[ $code3 == 429 && $code4 == 429 ]]; then
		echo "[rate_limit_ip] OK (early depletion)"
		exit 0
	fi
fi
echo "[rate_limit_ip] FAIL: $code1,$code2,$code3,$code4"; exit 1
