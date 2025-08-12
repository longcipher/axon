#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/static_files.toml

cleanup_ports 8080

$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid 2>/dev/null || true' EXIT
wait_port_listen 8080
wait_http_ok http://127.0.0.1:8080/static/ 50 0.2 200 || { echo "[static_files] FAIL: route not ready"; exit 1; }

curl -sf http://127.0.0.1:8080/static/ | grep -q "Axon Static OK" && echo "[static_files] OK" || { echo "[static_files] FAIL"; exit 1; }
