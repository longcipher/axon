#!/usr/bin/env bash
set -euo pipefail

# HTTP/3 example script
#
# Demonstrates starting Axon with HTTP/3 enabled and issuing a request using
# curl's HTTP/3 support (requires curl built with quiche / nghttp3 & wolfssl / openssl).
# If the local curl lacks --http3 support the script skips (treated as success)
# to avoid spurious CI failures on platforms without HTTP/3-enabled curl.

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
ROOT_DIR="${SCRIPT_DIR}/../../.."
source "${SCRIPT_DIR}/_lib.sh"

BIN=${BIN:-"${ROOT_DIR}/target/debug/axon"}
CFG_DIR="$(mktemp -d)"
pushd "$CFG_DIR" >/dev/null

echo "[http3] Working directory: $CFG_DIR"

cleanup() { pkill -P $$ 2>/dev/null || true; popd >/dev/null || true; rm -rf "$CFG_DIR" || true; }
trap cleanup EXIT

if ! curl --help 2>&1 | grep -q -- '--http3'; then
  echo "[http3] curl without --http3 support detected; skipping example (ok)"
  exit 0
fi

# Generate self-signed cert (valid for localhost) using openssl
echo "[http3] Generating self-signed certificate"
openssl req -x509 -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout key.pem -out cert.pem -days 1 >/dev/null 2>&1

# Start a tiny backend server that returns a fixed body
cat > backend.py <<'PY'
import http.server, socketserver
PORT = 9095
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith('/api'):
            body = b'ok-h3'
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.end_headers()
    def log_message(self, *args, **kwargs):
        return
with socketserver.TCPServer(('127.0.0.1', PORT), H) as httpd:
    httpd.serve_forever()
PY
python3 backend.py &
BACK_PID=$!
wait_port_listen 9095

# Copy example config template into working dir
cp "${ROOT_DIR}/examples/configs/http3_proxy.toml" config.toml

echo "[http3] Starting Axon (HTTP/3 enabled)"
"$BIN" serve --config config.toml &
GATEWAY_PID=$!
timeout_guard 30 $GATEWAY_PID $BACK_PID
wait_port_listen 8095

echo "[http3] Performing HTTP/3 request via curl"
BODY=$(curl --http3-only -sk https://127.0.0.1:8095/api/test || true)
CODE=$(curl --http3-only -sk -o /dev/null -w '%{http_code}' https://127.0.0.1:8095/api/test || true)

if [[ "$CODE" != "200" ]]; then
  echo "[http3] ERROR: Expected 200 from gateway over HTTP/3, got $CODE" >&2
  exit 1
fi
if [[ "$BODY" != "ok-h3" ]]; then
  echo "[http3] ERROR: Unexpected body: $BODY" >&2
  exit 1
fi

echo "[http3] Success: Received expected response over HTTP/3"
