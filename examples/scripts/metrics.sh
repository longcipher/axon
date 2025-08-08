#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/metrics.yaml

$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

# Touch an endpoint then check /metrics contains expected exposition names
curl -sf http://127.0.0.1:8088/static/ >/dev/null
curl -sf http://127.0.0.1:8088/metrics | grep -q 'axon_request_duration_seconds' && echo "[metrics] OK" || { echo "[metrics] FAIL"; exit 1; }
