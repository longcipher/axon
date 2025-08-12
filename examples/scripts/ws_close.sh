#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

PORT_GATEWAY=8093
PORT_BACKEND=9105
cleanup_ports $PORT_GATEWAY

python3 "$SCRIPT_DIR/ws_echo_backend.py" &
BACKEND_PID=$!

timeout_guard 25 $BACKEND_PID
wait_port_listen 9105

TMP_CFG=$(mktemp -t ws_closeXXXXXX).toml
cat > "$TMP_CFG" <<'CFG'
listen_addr = "127.0.0.1:8093"

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
import sys, socket, base64, os, struct
port=int(sys.argv[1])
key=base64.b64encode(os.urandom(16)).decode()
s=socket.socket(); s.connect(('127.0.0.1', port))
req=(f"GET /ws/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
s.sendall(req.encode())
resp=s.recv(2048).decode('utf-8','ignore')
if '101' not in resp:
 print('FAIL|handshake'); sys.exit(0)
# send close frame with code 1000
code=1000
payload=struct.pack('!H', code)
mask=os.urandom(4)
frame=bytearray([0x88, 0x80 | len(payload)])
frame.extend(mask)
masked=bytearray(payload)
for i in range(len(payload)):
 masked[i]^=mask[i%4]
frame.extend(masked)
s.sendall(frame)
# expect close echo
hdr=s.recv(2)
if len(hdr)<2: print('FAIL|short'); sys.exit(0)
opcode=hdr[0]&0x0F
ln=hdr[1]&0x7F
if opcode!=0x8: print(f'FAIL|opcode{opcode}'); sys.exit(0)
pl=b''
while len(pl)<ln:
 ch=s.recv(ln-len(pl))
 if not ch: break
 pl+=ch
if len(pl)==2:
 recv_code=struct.unpack('!H', pl)[0]
 print(f'OK|{recv_code}')
else:
 print('FAIL|payload_len')
PY
)
STATUS=${RESULT%%|*}
RC=${RESULT#*|}
if [[ "$STATUS" != "OK" || "$RC" != "1000" ]]; then
  echo "[ws_close] FAIL: $RESULT" >&2
  kill $GW_PID $BACKEND_PID 2>/dev/null || true
  rm -f "$TMP_CFG"
  exit 1
fi

echo "[ws_close] OK"
kill $GW_PID $BACKEND_PID 2>/dev/null || true
rm -f "$TMP_CFG"
wait || true
