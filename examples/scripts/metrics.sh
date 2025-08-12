#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/metrics.toml

cleanup_ports 8088

$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid 2>/dev/null || true' EXIT
wait_port_listen 8088
wait_http_ok http://127.0.0.1:8088/static/ 50 0.2 200 || { echo "[metrics] FAIL: static route not ready"; exit 1; }

curl -sf http://127.0.0.1:8088/metrics | grep -q 'axon_request_duration_seconds' && echo "[metrics] OK" || { echo "[metrics] FAIL"; exit 1; }
