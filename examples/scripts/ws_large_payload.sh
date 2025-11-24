#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

PORT_GATEWAY=8094
# Find a free backend port (attempt range 9106-9110)
for p in 9106 9107 9108 9109 9110; do
    if ! lsof -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then
        PORT_BACKEND=$p; break
    fi
done
PORT_BACKEND=${PORT_BACKEND:-9106}
cleanup_ports $PORT_GATEWAY

PORT=$PORT_BACKEND python3 "$SCRIPT_DIR/ws_echo_backend.py" &
BACKEND_PID=$!
timeout_guard 25 $BACKEND_PID
wait_port_listen $PORT_BACKEND

TMP_CFG=$(mktemp -t ws_largeXXXXXX).toml
cat > "$TMP_CFG" <<CFG
listen_addr = "127.0.0.1:8094"

[protocols]
http2_enabled = false
websocket_enabled = true
http3_enabled = false

[routes."/ws/"]
type = "websocket"
target = "http://127.0.0.1:${PORT_BACKEND}"
path_rewrite = "/"
max_message_size = 65536
max_frame_size = 65536
subprotocols = ["chat"]
idle_timeout_secs = 3
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
payload=('x'*120).encode()
mask=os.urandom(4)
frame=bytearray([0x81, 0x80 | len(payload)])
frame.extend(mask)
masked=bytearray(payload)
for i in range(len(payload)):
 masked[i]^=mask[i%4]
frame.extend(masked)
s.sendall(frame)
# read echo
hdr=s.recv(2)
if len(hdr)<2: print('FAIL|short'); sys.exit(0)
ln=hdr[1]&0x7F
data=b''
while len(data)<ln:
 ch=s.recv(ln-len(data))
 if not ch: break
 data+=ch
if len(data)!=120: print('FAIL|len'); sys.exit(0)
# now idle until server closes due to idle_timeout (expect close within ~3s)
s.settimeout(6)
try:
    extra=s.recv(2)
    if len(extra)==0:
        print('OK|closed')
    else:
        op=extra[0]&0x0F
        if op==0x8:
            print('OK|close_frame')
        else:
            print('FAIL|unexpected_opcode')
except socket.timeout:
    print('FAIL|no_timeout')
PY
)
STATUS=${RESULT%%|*}
if [[ "$STATUS" != "OK" ]]; then
  echo "[ws_large_payload] FAIL: $RESULT" >&2
  kill $GW_PID $BACKEND_PID 2>/dev/null || true
  rm -f "$TMP_CFG"
  exit 1
fi
echo "[ws_large_payload] OK"
kill $GW_PID $BACKEND_PID 2>/dev/null || true
rm -f "$TMP_CFG"
wait || true
