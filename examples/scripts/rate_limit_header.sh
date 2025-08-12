#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/rate_limit_header.toml

cleanup_ports 8085

$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid 2>/dev/null || true' EXIT
wait_port_listen 8085
wait_http_ok http://127.0.0.1:8085/api/ 50 0.1 429 || true  # may be 200 or 429 depending on state

# Missing key should be denied
code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8085/api/)
[[ "$code" == "429" ]] || { echo "[rate_limit_header] FAIL: missing-key $code"; exit 1; }

# With key, allow 2, 3rd should be 429
for i in 1 2; do curl -sf -H 'X-Api-Key: k' http://127.0.0.1:8085/api/ >/dev/null; done
code=$(curl -s -o /dev/null -w "%{http_code}" -H 'X-Api-Key: k' http://127.0.0.1:8085/api/)
[[ "$code" == "429" ]] && echo "[rate_limit_header] OK" || { echo "[rate_limit_header] FAIL: $code"; exit 1; }
