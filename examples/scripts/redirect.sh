#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/redirect.yaml

$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

# Follow redirects should land on /new/
out=$(curl -sfL http://127.0.0.1:8087/old/)
[[ "$out" =~ "Axon Static OK" ]] && echo "[redirect] OK" || { echo "[redirect] FAIL"; exit 1; }
