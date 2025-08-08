#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/rate_limit_ip.yaml

$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

# 3 allowed within 2s, 4th should be 429
for i in 1 2 3; do curl -sf http://127.0.0.1:8083/rl/ >/dev/null; done
code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8083/rl/)
[[ "$code" == "429" ]] && echo "[rate_limit_ip] OK" || { echo "[rate_limit_ip] FAIL: $code"; exit 1; }
