#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/rate_limit_header.yaml

$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

# Missing key should be denied
code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8085/api/)
[[ "$code" == "429" ]] || { echo "[rate_limit_header] FAIL: missing-key $code"; exit 1; }

# With key, allow 2, 3rd should be 429
for i in 1 2; do curl -sf -H 'X-Api-Key: k' http://127.0.0.1:8085/api/ >/dev/null; done
code=$(curl -s -o /dev/null -w "%{http_code}" -H 'X-Api-Key: k' http://127.0.0.1:8085/api/)
[[ "$code" == "429" ]] && echo "[rate_limit_header] OK" || { echo "[rate_limit_header] FAIL: $code"; exit 1; }
