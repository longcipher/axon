#!/usr/bin/env bash
# Shared helper functions for example scripts.
# Usage: source "$(dirname "$0")/_lib.sh"
set -euo pipefail

# Kill any processes listening on the given TCP ports (best-effort) and wait until free.
# Args: one or more port numbers
cleanup_ports() {
  local p retries
  for p in "$@"; do
    lsof -ti tcp:"$p" 2>/dev/null | xargs kill -9 2>/dev/null || true
  done
  # Wait (up to 3s) for all to clear
  for p in "$@"; do
    retries=0
    while lsof -ti tcp:"$p" >/dev/null 2>&1; do
      (( retries++ )) || true
      if (( retries > 30 )); then
        echo "[helper] WARN: port $p still in use after wait" >&2
        break
      fi
      sleep 0.1
    done
  done
}

# Wait for a TCP port to enter LISTEN state.
# Args: port [max_tries] [interval_seconds]
wait_port_listen() {
  local port=$1
  local max=${2:-50}
  local interval=${3:-0.1}
  local i
  for (( i=1; i<=max; i++ )); do
    if lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
      return 0
    fi
    sleep "$interval"
  done
  echo "[helper] ERROR: port $port did not start listening in time" >&2
  return 1
}

# Poll an HTTP endpoint until it returns a successful status code (2xx/3xx unless strict flag).
# Args: url [max_tries] [interval_seconds] [expected_code]
wait_http_ok() {
  local url=$1
  local max=${2:-50}
  local interval=${3:-0.1}
  local expect=${4:-""}
  local i code
  for (( i=1; i<=max; i++ )); do
    code=$(curl -s -o /dev/null -w "%{http_code}" "$url" || true)
    if [[ -n "$expect" ]]; then
      if [[ "$code" == "$expect" ]]; then return 0; fi
    else
      if [[ "$code" =~ ^2|3 ]]; then return 0; fi
    fi
    sleep "$interval"
  done
  echo "[helper] ERROR: $url not ready (last code=$code)" >&2
  return 1
}

# Simple timeout guard; run in background:  timeout_guard <seconds> <pid...>
timeout_guard() {
  local secs=$1; shift
  local targets=("$@")
  ( sleep "$secs"; for t in "${targets[@]}"; do kill "$t" 2>/dev/null || true; done ) &
}

export -f cleanup_ports wait_port_listen wait_http_ok timeout_guard
