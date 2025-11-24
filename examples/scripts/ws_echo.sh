#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

PORT_GATEWAY=8090
PORT_BACKEND=9105
cleanup_ports $PORT_GATEWAY $PORT_BACKEND

# Start echo backend
python3 "$SCRIPT_DIR/ws_echo_backend.py" &
BACKEND_PID=$!

timeout_guard 20 $BACKEND_PID

wait_port_listen $PORT_BACKEND

# Start gateway
cargo run --quiet -- serve --config examples/configs/ws_echo.toml &
GW_PID=$!

timeout_guard 25 $GW_PID
wait_port_listen $PORT_GATEWAY

# Always use internal minimal Python client (no external dependencies)
ECHO_MSG="hello-axon"
SUBPROTO="chat"
RESULT=$(python3 - "$ECHO_MSG" "$PORT_GATEWAY" "$SUBPROTO" <<'PY'
import sys, socket, base64, os
msg = sys.argv[1]
port = int(sys.argv[2])
proto = sys.argv[3]
key = base64.b64encode(os.urandom(16)).decode()
s = socket.socket(); s.connect(('127.0.0.1', port))
request = (
    f"GET /ws/ HTTP/1.1\r\n"
    f"Host: 127.0.0.1:{port}\r\n"
    "Upgrade: websocket\r\n"
    "Connection: Upgrade\r\n"
    f"Sec-WebSocket-Key: {key}\r\n"
    "Sec-WebSocket-Version: 13\r\n"
    f"Sec-WebSocket-Protocol: {proto}\r\n\r\n"
)
s.sendall(request.encode())
resp = s.recv(2048).decode('utf-8', 'ignore')
if '101' not in resp:
    print('ERROR|handshake_failed')
    sys.exit(0)
lines = [l for l in resp.split('\r\n') if l]
neg = None
for l in lines:
    if l.lower().startswith('sec-websocket-protocol:'):
        neg = l.split(':',1)[1].strip()
        break
if not neg:
    print('ERROR|no_subprotocol')
    sys.exit(0)
# build and send masked text frame
payload = msg.encode()
frame = bytearray([0x81])
ln = len(payload)
if ln > 125:
    print('ERROR|oversize')
    sys.exit(0)
mask_key = os.urandom(4)
frame.append(0x80 | ln)
frame.extend(mask_key)
masked = bytearray(payload)
for i in range(ln):
    masked[i] ^= mask_key[i % 4]
frame.extend(masked)
s.sendall(frame)
hdr = s.recv(2)
if len(hdr) < 2:
    print('ERROR|short_header')
    sys.exit(0)
opcode = hdr[0] & 0x0F
length = hdr[1] & 0x7F
data = b''
while len(data) < length:
    chunk = s.recv(length - len(data))
    if not chunk:
        break
    data += chunk
print(f"OK|{neg}|{data.decode(errors='ignore')}")
PY
)

STATUS=${RESULT%%|*}
REST=${RESULT#*|}
if [[ "$STATUS" != "OK" ]]; then
  echo "[ws_echo] FAIL: $RESULT" >&2
  kill $GW_PID $BACKEND_PID 2>/dev/null || true
  exit 1
fi
NEGOTIATED=${REST%%|*}
ECHOED=${REST##*|}
if [[ "$NEGOTIATED" != "$SUBPROTO" ]]; then
  echo "[ws_echo] FAIL: negotiated subprotocol '$NEGOTIATED' expected '$SUBPROTO'" >&2
  kill $GW_PID $BACKEND_PID 2>/dev/null || true
  exit 1
fi
if [[ "$ECHOED" != "$ECHO_MSG" ]]; then
  echo "[ws_echo] FAIL: expected echo '$ECHO_MSG' got '$ECHOED'" >&2
  kill $GW_PID $BACKEND_PID 2>/dev/null || true
  exit 1
fi

echo "[ws_echo] OK (subprotocol $NEGOTIATED)"
kill $GW_PID $BACKEND_PID 2>/dev/null || true
wait || true
