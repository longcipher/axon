#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/path_rewrite.yaml

# Backend expects /real path only
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/real':
            self.send_response(200); self.end_headers(); self.wfile.write(b"OK"); return
        self.send_response(404); self.end_headers(); self.wfile.write(b"NO")
HTTPServer(("127.0.0.1", 9301), H).serve_forever()
PY
B1=$!
trap 'kill $B1 2>/dev/null || true' EXIT

$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

# Call /svc/, expect to hit backend /real via rewrite
out=$(curl -sf http://127.0.0.1:8086/svc/)
[[ "$out" == "OK" ]] && echo "[path_rewrite] OK" || { echo "[path_rewrite] FAIL: $out"; exit 1; }
