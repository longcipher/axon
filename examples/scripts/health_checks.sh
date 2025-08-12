#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/health_checks.toml

# Free ports to avoid conflicts
cleanup_ports 8084 9201 9202

# Backend 1 healthy endpoint
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_HEAD(self):
        if self.path == '/health':
            self.send_response(200); self.end_headers(); return
        self.send_response(200); self.end_headers()
    def do_GET(self):
        if self.path == '/health':
            self.send_response(200); self.end_headers(); self.wfile.write(b"ok"); return
        self.send_response(200); self.end_headers(); self.wfile.write(b"B1")
HTTPServer(("127.0.0.1", 9201), H).serve_forever()
PY
backend1_pid=$!

# Backend 2 unhealthy endpoint (404)
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_HEAD(self):
        if self.path == '/health':
            self.send_response(404); self.end_headers(); return
        self.send_response(200); self.end_headers()
    def do_GET(self):
        if self.path == '/health':
            self.send_response(404); self.end_headers(); self.wfile.write(b"fail"); return
        self.send_response(200); self.end_headers(); self.wfile.write(b"B2")
HTTPServer(("127.0.0.1", 9202), H).serve_forever()
PY
backend2_pid=$!
trap 'kill $backend1_pid $backend2_pid 2>/dev/null || true' EXIT

# Wait for backend ports to listen
wait_port_listen 9201
wait_port_listen 9202

# Start gateway and wait for first health cycle
$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 40 "$gateway_pid"
trap 'kill $gateway_pid $backend1_pid $backend2_pid 2>/dev/null || true' EXIT

wait_port_listen 8084

# Wait for health checker to mark unhealthy backend (poll up to 10s)
success=0
for i in {1..20}; do
        sleep 0.5
        out1=$(curl -sf http://127.0.0.1:8084/api/ || true)
        out2=$(curl -sf http://127.0.0.1:8084/api/ || true)
        if [[ "$out1" == "B1" && "$out2" == "B1" ]]; then
                success=1
                break
        fi
done

if [[ $success -eq 1 ]]; then
    echo "[health_checks] OK"
else
    echo "[health_checks] FAIL: $out1,$out2"
    exit 1
fi
