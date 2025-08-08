#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob
for s in examples/scripts/*.sh; do
  [[ "$s" =~ run_all.sh$ ]] && continue
  echo "==> $s"
  "$s"
done
