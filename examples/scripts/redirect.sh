#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/redirect.toml

cleanup_ports 8087

$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid 2>/dev/null || true' EXIT
wait_port_listen 8087
wait_http_ok http://127.0.0.1:8087/new/ 50 0.2 200 || { echo "[redirect] FAIL: /new/ not ready"; exit 1; }

out=$(curl -sfL http://127.0.0.1:8087/old/)
[[ "$out" =~ "Axon Static OK" ]] && echo "[redirect] OK" || { echo "[redirect] FAIL"; exit 1; }
