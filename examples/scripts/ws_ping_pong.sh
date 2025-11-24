#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

PORT_GATEWAY=8092
PORT_BACKEND=9105
cleanup_ports $PORT_GATEWAY

python3 "$SCRIPT_DIR/ws_echo_backend.py" &
BACKEND_PID=$!

timeout_guard 25 $BACKEND_PID
wait_port_listen 9105

TMP_CFG=$(mktemp -t ws_ping_pongXXXXXX).toml
cat > "$TMP_CFG" <<'CFG'
listen_addr = "127.0.0.1:8092"

[protocols]
http2_enabled = false
websocket_enabled = true
http3_enabled = false

[routes."/ws/"]
type = "websocket"
target = "http://127.0.0.1:9105"
path_rewrite = "/"
max_message_size = 65536
max_frame_size = 65536
subprotocols = ["chat"]
CFG

cargo run --quiet -- serve --config "$TMP_CFG" &
GW_PID=$!

timeout_guard 25 $GW_PID
wait_port_listen $PORT_GATEWAY

RESULT=$(python3 - $PORT_GATEWAY <<'PY'
import sys, socket, base64, os, time
port=int(sys.argv[1])
key=base64.b64encode(os.urandom(16)).decode()
s=socket.socket(); s.connect(('127.0.0.1', port))
req=(f"GET /ws/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
s.sendall(req.encode())
resp=s.recv(2048).decode('utf-8','ignore')
if '101' not in resp:
 print('FAIL|handshake'); sys.exit(0)
# send ping (opcode 0x9) masked with small payload
payload=b'pp'
mask=os.urandom(4)
frame=bytearray([0x89, 0x80 | len(payload)])
frame.extend(mask)
masked=bytearray(payload)
for i in range(len(payload)):
 masked[i]^=mask[i%4]
frame.extend(masked)
s.sendall(frame)
# expect pong (opcode 0xA) with same payload
hdr=s.recv(2)
if len(hdr)<2: print('FAIL|short'); sys.exit(0)
opcode=hdr[0]&0x0F
ln=hdr[1]&0x7F
if opcode!=0xA: print(f'FAIL|opcode{opcode}'); sys.exit(0)
pl=s.recv(ln) if ln else b''
print('OK|' + pl.decode('utf-8','ignore'))
PY
)
STATUS=${RESULT%%|*}
PAYLOAD=${RESULT#*|}
if [[ "$STATUS" != "OK" || "$PAYLOAD" != "pp" ]]; then
  echo "[ws_ping_pong] FAIL: $RESULT" >&2
  kill $GW_PID $BACKEND_PID 2>/dev/null || true
  rm -f "$TMP_CFG"
  exit 1
fi

echo "[ws_ping_pong] OK"
kill $GW_PID $BACKEND_PID 2>/dev/null || true
rm -f "$TMP_CFG"
wait || true
