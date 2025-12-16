#!/usr/bin/env bash
# Host-Based Routing Example
# 
# This script demonstrates how to use host-based routing to route requests
# to different backends based on the Host header.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/_lib.sh"

CONFIG_FILE="${SCRIPT_DIR}/../configs/host_based_routing.toml"
AXON_PID=""
BACKEND_PIDS=()

# Cleanup function
cleanup() {
    echo "🧹 Cleaning up..."
    
    # Kill axon if running
    if [ -n "$AXON_PID" ]; then
        kill $AXON_PID 2>/dev/null || true
    fi
    
    # Kill backend servers
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
    
    # Simple Python HTTP server that returns different responses
    python3 -c "
import http.server
import socketserver

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
        import json
        self.wfile.write(json.dumps(response).encode())
    
    def log_message(self, format, *args):
        pass  # Suppress logs

with socketserver.TCPServer(('', $port), CustomHandler) as httpd:
    httpd.serve_forever()
" &
    
    BACKEND_PIDS+=($!)
    sleep 0.5
}

# Start multiple backend servers
start_backend 3001 "api-backend"
start_backend 3002 "admin-backend"
start_backend 4001 "app-backend-1"
start_backend 4002 "app-backend-2"
start_backend 4003 "app-backend-3"
start_backend 5000 "default-backend"

# Start axon
echo "🚀 Starting Axon with host-based routing..."
cargo run --quiet -- serve --config "$CONFIG_FILE" &
AXON_PID=$!

# Wait for axon to start
sleep 2

echo ""
echo "======================================"
echo "  Host-Based Routing Demo"
echo "======================================"
echo ""

# Test different hosts
echo "📝 Testing different hosts..."
echo ""

echo "1️⃣  Testing api.example.com (should route to port 3001):"
curl -s -H "Host: api.example.com" http://localhost:8080/api/users | jq .
echo ""

echo "2️⃣  Testing admin.example.com (should route to port 3002):"
curl -s -H "Host: admin.example.com" http://localhost:8080/admin/dashboard | jq .
echo ""

echo "3️⃣  Testing app.example.com with load balancing (should round-robin to 4001-4003):"
for i in {1..3}; do
    echo "   Request $i:"
    curl -s -H "Host: app.example.com" http://localhost:8080/services/data | jq .
done
echo ""

echo "4️⃣  Testing default route (no host match, should route to port 5000):"
curl -s http://localhost:8080/ | jq .
echo ""

echo "5️⃣  Testing with unknown host (should use default route):"
curl -s -H "Host: unknown.example.com" http://localhost:8080/ | jq .
echo ""

echo "======================================"
echo "✅ Demo completed successfully!"
echo "======================================"
echo ""
echo "Press Ctrl+C to stop all servers..."

# Keep running
wait
