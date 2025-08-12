#!/usr/bin/env bash
set -euo pipefail

BIN=${BIN:-"cargo run --"}
CFG=examples/configs/hot_reload.toml

# Ensure no previous instance is occupying the port (leftover from aborted runs)
lsof -ti tcp:8089 2>/dev/null | xargs kill -9 2>/dev/null || true

# Recreate base config to ensure a clean start (avoid leftover appended routes)
cat > "$CFG" <<'EOF'
## Base hot reload scenario configuration (route /r2/ will be added dynamically by this script)
listen_addr = "127.0.0.1:8089"

[health_check]
enabled = false

[routes."/r1/"]
type = "static"
root = "examples/static"
EOF

$BIN serve --config "$CFG" &
PID=$!
( sleep 40; kill $PID 2>/dev/null || true ) &
KILLER=$!
trap 'kill $PID $KILLER 2>/dev/null || true' EXIT
# Wait for server to bind (max 30 * 100ms = 3s)
for i in {1..30}; do
	if lsof -iTCP:8089 -sTCP:LISTEN >/dev/null 2>&1; then
		break
	fi
	sleep 0.1
done

code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8089/r2/)
[[ "$code" == "404" ]] || { echo "[hot_reload] FAIL: unexpected initial $code"; exit 1; }

echo "[hot_reload] Adding new route /r2/ (atomic replace)"
# Wait past debounce window to ensure our change is seen as a distinct update
sleep 3

# Atomically replace config with the new route included (rename -> reduces chance of partial read)
TMP=$(mktemp)
cat > "$TMP" <<'EOF'
## Base hot reload scenario configuration (route /r2/ dynamically added)
listen_addr = "127.0.0.1:8089"

[health_check]
enabled = false

[routes."/r1/"]
type = "static"
root = "examples/static"

[routes."/r2/"]
type = "static"
root = "examples/static"
EOF
mv "$TMP" "$CFG"
sync

# Poll for hot reload (up to 100 * 200ms = 20s)
for i in {1..100}; do
	code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8089/r2/ || true)
	if [[ "$code" == "200" ]]; then
		echo "[hot_reload] OK (reloaded after $((i*200))ms)"
		exit 0
	fi
	sleep 0.2
done

echo "[hot_reload] FAIL: $code (after extended wait)"
exit 1
