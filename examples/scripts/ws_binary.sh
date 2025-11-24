#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

PORT_GATEWAY=8091
PORT_BACKEND=9105 # reuse existing backend implementation (now supports binary)
cleanup_ports $PORT_GATEWAY

# Start backend if not already running (separate port gateway uses its own listen)
python3 "$SCRIPT_DIR/ws_echo_backend.py" &
BACKEND_PID=$!

timeout_guard 25 $BACKEND_PID
wait_port_listen 9105

# Start gateway with same config but override listen port via temp config
TMP_CFG=$(mktemp -t ws_binaryXXXXXX).toml
cat > "$TMP_CFG" <<'CFG'
listen_addr = "127.0.0.1:8091"

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
subprotocols = ["chat", "echo"]
CFG

cargo run --quiet -- serve --config "$TMP_CFG" &
GW_PID=$!

timeout_guard 25 $GW_PID
wait_port_listen $PORT_GATEWAY

# Python client sending a small binary frame (opcode 0x2)
RESULT=$(python3 - $PORT_GATEWAY <<'PY'
import sys, socket, base64, os
port=int(sys.argv[1])
key=base64.b64encode(os.urandom(16)).decode()
s=socket.socket(); s.connect(('127.0.0.1', port))
req=(f"GET /ws/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
s.sendall(req.encode())
resp=s.recv(2048).decode('utf-8','ignore')
if '101' not in resp:
 print('FAIL|handshake'); sys.exit(0)
# Build binary frame
payload=b'\x01\x02\xFFtest'  # arbitrary bytes
mask=os.urandom(4)
frame=bytearray([0x82, 0x80 | len(payload)])
frame.extend(mask)
masked=bytearray(payload)
for i in range(len(payload)):
 masked[i]^=mask[i%4]
frame.extend(masked)
s.sendall(frame)
# Read echoed binary
hdr=s.recv(2)
if len(hdr)<2: print('FAIL|short'); sys.exit(0)
opcode=hdr[0]&0x0F
ln=hdr[1]&0x7F
if opcode!=0x2: print(f'FAIL|opcode{opcode}'); sys.exit(0)
data=b''
while len(data)<ln:
 ch=s.recv(ln-len(data))
 if not ch: break
 data+=ch
print('OK|' + data.hex())
PY
)
STATUS=${RESULT%%|*}
DATA_HEX=${RESULT#*|}
if [[ "$STATUS" != "OK" ]]; then
  echo "[ws_binary] FAIL: $RESULT" >&2
  kill $GW_PID $BACKEND_PID 2>/dev/null || true
  rm -f "$TMP_CFG"
  exit 1
fi
# Validate expected prefix hex 0102ff74657374 (payload hex)
if [[ "$DATA_HEX" != "0102ff74657374" ]]; then
  echo "[ws_binary] FAIL: unexpected payload $DATA_HEX" >&2
  kill $GW_PID $BACKEND_PID 2>/dev/null || true
  rm -f "$TMP_CFG"
  exit 1
fi

echo "[ws_binary] OK"
kill $GW_PID $BACKEND_PID 2>/dev/null || true
rm -f "$TMP_CFG"
wait || true
