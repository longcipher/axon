#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/load_balance_rr.toml

cleanup_ports 8082 9101 9102

# Start two tiny backends with different responses
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b"B1")
HTTPServer(("127.0.0.1", 9101), H).serve_forever()
PY
backend1_pid=$!
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b"B2")
HTTPServer(("127.0.0.1", 9102), H).serve_forever()
PY
backend2_pid=$!
trap 'kill $backend1_pid $backend2_pid 2>/dev/null || true' EXIT

wait_port_listen 9101
wait_port_listen 9102

# Start gateway
$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid $backend1_pid $backend2_pid 2>/dev/null || true' EXIT
wait_port_listen 8082
wait_http_ok http://127.0.0.1:8082/svc/ 60 0.1 200 || { echo "[load_balance_rr] FAIL: gateway not ready"; exit 1; }

# Make two requests; expect alternating backends
# Make four sequential requests to observe alternation regardless of starting backend
r1=$(curl -sf http://127.0.0.1:8082/svc/)
r2=$(curl -sf http://127.0.0.1:8082/svc/)
r3=$(curl -sf http://127.0.0.1:8082/svc/)
r4=$(curl -sf http://127.0.0.1:8082/svc/)
seq="$r1$r2$r3$r4"
if [[ "$seq" == "B1B2B1B2" || "$seq" == "B2B1B2B1" ]]; then
    echo "[load_balance_rr] OK"
else
    echo "[load_balance_rr] FAIL: $r1,$r2,$r3,$r4"; exit 1
fi
