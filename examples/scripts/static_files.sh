#!/usr/bin/env bash
set -euo pipefail

# Serve static files config and curl it
BIN=${BIN:-"cargo run --"}
CFG=examples/configs/static_files.yaml

$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

curl -sf http://127.0.0.1:8080/static/ | grep -q "Axon Static OK"
echo "[static_files] OK"
