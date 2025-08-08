#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/load_balance_rr.yaml

# Start two tiny backends with different responses
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b"B1")
HTTPServer(("127.0.0.1", 9101), H).serve_forever()
PY
B1=$!
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b"B2")
HTTPServer(("127.0.0.1", 9102), H).serve_forever()
PY
B2=$!
trap 'kill $B1 $B2 2>/dev/null || true' EXIT

# Start gateway
$BIN serve --config "$CFG" &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
sleep 1

# Make two requests; expect alternating backends
r1=$(curl -sf http://127.0.0.1:8082/svc/)
r2=$(curl -sf http://127.0.0.1:8082/svc/)
[[ "$r1" == "B1" && "$r2" == "B2" ]] && echo "[load_balance_rr] OK" || { echo "[load_balance_rr] FAIL: $r1,$r2"; exit 1; }
