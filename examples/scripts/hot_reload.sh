#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/hot_reload.yaml

$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8089/r2/)
[[ "$code" == "404" ]] || { echo "[hot_reload] FAIL: unexpected initial $code"; exit 1; }

# Add route r2 and expect it to be served after reload debounce
cat >> "$CFG" <<'YAML'
routes:
  "/r1/":
    type: "static"
    root: "examples/static"
  "/r2/":
    type: "static"
    root: "examples/static"
YAML

sleep 3
code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8089/r2/)
[[ "$code" == "200" ]] && echo "[hot_reload] OK" || { echo "[hot_reload] FAIL: $code"; exit 1; }
