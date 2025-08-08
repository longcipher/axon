#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/health_checks.yaml

# Backend 1 healthy endpoint
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/health':
            self.send_response(200); self.end_headers(); self.wfile.write(b"ok"); return
        self.send_response(200); self.end_headers(); self.wfile.write(b"B1")
HTTPServer(("127.0.0.1", 9201), H).serve_forever()
PY
B1=$!

# Backend 2 unhealthy endpoint (404)
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/health':
            self.send_response(404); self.end_headers(); self.wfile.write(b"fail"); return
        self.send_response(200); self.end_headers(); self.wfile.write(b"B2")
HTTPServer(("127.0.0.1", 9202), H).serve_forever()
PY
B2=$!
trap 'kill $B1 $B2 2>/dev/null || true' EXIT

# Start gateway and wait for first health cycle
$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 2

# Expect only backend1 to be selected over multiple requests
out1=$(curl -sf http://127.0.0.1:8084/api/)
out2=$(curl -sf http://127.0.0.1:8084/api/)
[[ "$out1" == "B1" && "$out2" == "B1" ]] && echo "[health_checks] OK" || { echo "[health_checks] FAIL: $out1,$out2"; exit 1; }
