#!/usr/bin/env bash
# Root Path Host-Based Routing Test
#
# This script demonstrates routing on root path "/" based on Host header

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/_lib.sh"

CONFIG_FILE="${SCRIPT_DIR}/../configs/root_path_host_routing.toml"
AXON_PID=""
BACKEND_PIDS=()

# Cleanup function
cleanup() {
    echo "🧹 Cleaning up..."
    
    if [ -n "$AXON_PID" ]; then
        kill $AXON_PID 2>/dev/null || true
    fi
    
    for pid in "${BACKEND_PIDS[@]}"; do
        kill $pid 2>/dev/null || true
    done
    
    echo "✅ Cleanup completed"
}

trap cleanup EXIT INT TERM

# Start mock backend servers
start_backend() {
    local port=$1
    local name=$2
    
    echo "🚀 Starting backend '$name' on port $port..."
    
    python3 -c "
import http.server
import socketserver
import json

class CustomHandler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-type', 'application/json')
        self.end_headers()
        response = {
            'backend': '$name',
            'port': $port,
            'path': self.path,
            'host': self.headers.get('Host', 'unknown')
        }
        self.wfile.write(json.dumps(response, indent=2).encode())
    
    def log_message(self, format, *args):
        pass

with socketserver.TCPServer(('', $port), CustomHandler) as httpd:
    httpd.serve_forever()
" &
    
    BACKEND_PIDS+=($!)
    sleep 0.5
}

# Start backend servers
start_backend 3001 "api-backend"
start_backend 3002 "admin-backend"
start_backend 5555 "fallback-backend"

# Start axon
echo "🚀 Starting Axon..."
cargo run --quiet -- serve --config "$CONFIG_FILE" &
AXON_PID=$!

sleep 2

echo ""
echo "=========================================="
echo "  Root Path Host-Based Routing Test"
echo "=========================================="
echo ""

# Test 1: Root path with api.example.com host on /api path
echo "1️⃣  Testing /api path with Host: api.example.com"
echo "   Expected: route to api-backend (port 3001)"
echo ""
response=$(curl -s -H "Host: api.example.com" http://localhost:8080/api)
echo "$response" | jq .
backend=$(echo "$response" | jq -r '.backend')
if [ "$backend" = "api-backend" ]; then
    echo "   ✅ PASS: Routed to api-backend"
else
    echo "   ❌ FAIL: Expected api-backend, got $backend"
fi
echo ""

# Test 2: Admin path with admin.example.com host
echo "2️⃣  Testing /admin path with Host: admin.example.com"
echo "   Expected: route to admin-backend (port 3002)"
echo ""
response=$(curl -s -H "Host: admin.example.com" http://localhost:8080/admin)
echo "$response" | jq .
backend=$(echo "$response" | jq -r '.backend')
if [ "$backend" = "admin-backend" ]; then
    echo "   ✅ PASS: Routed to admin-backend"
else
    echo "   ❌ FAIL: Expected admin-backend, got $backend"
fi
echo ""

# Test 3: Root path with no host or unknown host
echo "3️⃣  Testing root path with no specific host match"
echo "   Expected: route to fallback-backend (port 5555)"
echo ""
response=$(curl -s -H "Host: unknown.example.com" http://localhost:8080/)
echo "$response" | jq .
backend=$(echo "$response" | jq -r '.backend')
if [ "$backend" = "fallback-backend" ]; then
    echo "   ✅ PASS: Routed to fallback-backend"
else
    echo "   ❌ FAIL: Expected fallback-backend, got $backend"
fi
echo ""

# Test 4: Nested path with api.example.com on /api
echo "4️⃣  Testing /api/users/123 with Host: api.example.com"
echo "   Expected: route to api-backend (matches /api route)"
echo ""
response=$(curl -s -H "Host: api.example.com" http://localhost:8080/api/users/123)
echo "$response" | jq .
backend=$(echo "$response" | jq -r '.backend')
if [ "$backend" = "api-backend" ]; then
    echo "   ✅ PASS: Routed to api-backend"
else
    echo "   ❌ FAIL: Expected api-backend, got $backend"
fi
echo ""

# Test 5: Another path with fallback
echo "5️⃣  Testing /other with no matching host"
echo "   Expected: route to fallback-backend"
echo ""
response=$(curl -s http://localhost:8080/other)
echo "$response" | jq .
backend=$(echo "$response" | jq -r '.backend')
if [ "$backend" = "fallback-backend" ]; then
    echo "   ✅ PASS: Routed to fallback-backend"
else
    echo "   ❌ FAIL: Expected fallback-backend, got $backend"
fi
echo ""

echo "=========================================="
echo "✅ All tests completed!"
echo "=========================================="
echo ""
echo "Press Ctrl+C to stop all servers..."

wait
