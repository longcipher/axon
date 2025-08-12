#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

echo "[run_all] Building binary once..."
cargo build --quiet
export BIN="target/debug/axon"

FAILED=()
for s in examples/scripts/*.sh; do
  [[ "$s" =~ run_all.sh$ ]] && continue
  [[ "$s" =~ _lib.sh$ ]] && continue
  echo "==> $s"
  if ! BIN="$BIN" "$s"; then
    FAILED+=("$s")
  fi
done

if (( ${#FAILED[@]} > 0 )); then
  echo "[run_all] FAILED scripts:" >&2
  printf ' - %s\n' "${FAILED[@]}" >&2
  exit 1
fi

echo "[run_all] All example scripts succeeded"
