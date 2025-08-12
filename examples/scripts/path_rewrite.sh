#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/path_rewrite.toml

# Free required ports
cleanup_ports 9301 8086

# Start backend serving only /real
python3 - <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
        def do_GET(self):
                if self.path == '/real':
                        self.send_response(200); self.end_headers(); self.wfile.write(b"OK"); return
                self.send_response(404); self.end_headers(); self.wfile.write(b"NO")
HTTPServer(("127.0.0.1", 9301), H).serve_forever()
PY
backend_pid=$!
trap 'kill $backend_pid 2>/dev/null || true' EXIT

$BIN serve --config "$CFG" &
gateway_pid=$!
timeout_guard 30 "$gateway_pid"
trap 'kill $gateway_pid 2>/dev/null || true' EXIT

wait_port_listen 9301 || { echo "[path_rewrite] FAIL: backend not listening" >&2; exit 1; }
wait_port_listen 8086 || { echo "[path_rewrite] FAIL: gateway not listening" >&2; exit 1; }

wait_http_ok http://127.0.0.1:9301/real 50 0.1 200 || { echo "[path_rewrite] FAIL: backend /real not ready" >&2; exit 1; }
wait_http_ok http://127.0.0.1:8086/svc/ 100 0.1 200 || { echo "[path_rewrite] FAIL: gateway rewrite not ready" >&2; exit 1; }

out=$(curl -sf http://127.0.0.1:8086/svc/)
if [[ "$out" == "OK" ]]; then
    echo "[path_rewrite] OK"
else
    echo "[path_rewrite] FAIL: $out"; exit 1
fi
