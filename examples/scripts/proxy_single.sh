#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/proxy_single.yaml

# Start a tiny backend
python3 -m http.server 9001 --bind 127.0.0.1 >/dev/null 2>&1 &
B1=$!
trap 'kill $B1 2>/dev/null || true' EXIT

# Start gateway
$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

# Verify proxy (python http.server serves current dir; 200 expected)
code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8081/api/)
[[ "$code" == "200" ]] && echo "[proxy_single] OK" || { echo "[proxy_single] FAIL"; exit 1; }
