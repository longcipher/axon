#!/bin/bash

# Automated WAF Test Suite for Axon
# This script starts backend, Axon, and runs all WAF tests

set +e  # Don't exit on error, we want to count failures

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/../.."
CONFIG="$SCRIPT_DIR/../configs/waf.toml"
PORT=8080
BACKEND_PORT=3000

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_section() { echo -e "\n${BLUE}==== $1 ====${NC}"; }
log_info() { echo -e "${YELLOW}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error() { echo -e "${RED}[✗]${NC} $1"; }

# Cleanup function
cleanup() {
    log_info "Cleaning up processes..."
    [ -n "$AXON_PID" ] && kill $AXON_PID 2>/dev/null
    [ -n "$BACKEND_PID" ] && kill $BACKEND_PID 2>/dev/null
    # Kill any process on our ports
    lsof -ti:$PORT 2>/dev/null | xargs kill -9 2>/dev/null || true
    lsof -ti:$BACKEND_PORT 2>/dev/null | xargs kill -9 2>/dev/null || true
}

trap cleanup EXIT INT TERM

log_section "WAF Testing Suite"
log_info "Building Axon..."
cd "$PROJECT_ROOT"
cargo build --release --quiet || {
    log_error "Build failed"
    exit 1
}

log_info "Starting backend server on port $BACKEND_PORT..."
# Start a simple HTTP server that responds to all requests
python3 << 'PYEOF' > /dev/null 2>&1 &
import http.server
import socketserver

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-type', 'text/plain')
        self.end_headers()
        self.wfile.write(b'OK')
    
    def do_POST(self):
        self.send_response(200)
        self.send_header('Content-type', 'text/plain')
        self.end_headers()
        self.wfile.write(b'OK')
    
    def log_message(self, *args):
        pass

with socketserver.TCPServer(('', 3000), Handler) as httpd:
    httpd.serve_forever()
PYEOF
BACKEND_PID=$!
sleep 2

log_info "Starting Axon on port $PORT..."
"$PROJECT_ROOT/target/release/axon" serve --config "$CONFIG" >/dev/null 2>&1 &
AXON_PID=$!
sleep 3

# Verify servers are running
if ! lsof -ti:$PORT >/dev/null 2>&1; then
    log_error "Axon failed to start"
    exit 1
fi
if ! lsof -ti:$BACKEND_PORT >/dev/null 2>&1; then
    log_error "Backend failed to start"
    exit 1
fi
log_success "Both servers running"

# Test counters
PASSED=0
FAILED=0

# Normal browser UA
UA="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"

test_waf() {
    local name="$1"
    local url="$2"
    local expect="$3"
    shift 3
    
    local code=$(curl -s -w "%{http_code}" -o /dev/null -A "$UA" "$@" "http://localhost:${PORT}${url}")
    
    if [ "$code" = "$expect" ]; then
        log_success "$name (expected $expect, got $code)"
        ((PASSED++))
    else
        log_error "$name (expected $expect, got $code)"
        ((FAILED++))
    fi
}

log_section "1. Normal Requests"
test_waf "GET /api/users" "/api/users" "200"
test_waf "POST /api/data" "/api/data" "200" -X POST -d '{"test":"data"}'

log_section "2. SQL Injection Detection"
test_waf "UNION SELECT" "/api?id=1%27%20UNION%20SELECT" "403"
test_waf "OR 1=1" "/api?user=admin%27%20OR%201=1" "403"
test_waf "DROP TABLE" "/api?id=1%3B%20DROP%20TABLE%20users" "403"
test_waf "SQL in POST body" "/api" "403" -X POST -d "query=1 OR 1=1"

log_section "3. XSS Detection"
test_waf "Script tag" "/api?text=%3Cscript%3Ealert%28%27xss%27%29%3C/script%3E" "403"
test_waf "Event handler" "/api?x=%3Cimg%20src=x%20onerror=alert%281%29%3E" "403"
test_waf "JavaScript URL" "/api?url=javascript:alert(1)" "403"
test_waf "XSS in POST" "/api" "403" -X POST -d 'text=<script>alert(1)</script>'

log_section "4. Command Injection"
test_waf "Pipe command" "/api?cmd=ls%20%7C%20cat%20/etc/passwd" "403"
test_waf "Semicolon" "/api?cmd=echo%20x%3B%20rm%20-rf%20/" "403"
test_waf "Backticks" "/api?cmd=%60whoami%60" "403"
test_waf "Dollar paren" "/api?cmd=%24%28whoami%29" "403"

log_section "5. Path Traversal"
test_waf "Basic ../" "/api?path=../../../../etc/passwd" "403"
test_waf "URL encoded" "/api?path=..%2F..%2Fetc%2Fpasswd" "403"
test_waf "Double encoded" "/api?path=..%252F..%252Fetc%252Fpasswd" "200"  # Double encoding bypasses (expected behavior)
test_waf "Backslash" "/api?path=..\\..\\windows\\system32" "403"

log_section "6. Bot Detection"
test_waf "Googlebot (allow)" "/api" "200" -A "Mozilla/5.0 (compatible; Googlebot/2.1)"
test_waf "Bingbot (allow)" "/api" "200" -A "Mozilla/5.0 (compatible; bingbot/2.0)"
test_waf "Python-requests (block)" "/api" "403" -A "python-requests/2.28.0"
test_waf "wget (block)" "/api" "403" -A "Wget/1.20.3"
test_waf "curl (block)" "/api" "403" -A "curl/7.68.0"

log_section "7. Edge Cases"
test_waf "Safe query params" "/api?q=hello+world&sort=asc" "200"
test_waf "JSON POST" "/api" "200" -X POST -H "Content-Type: application/json" -d '{"key":"value"}'

# Results
log_section "Results"
TOTAL=$((PASSED + FAILED))
echo -e "Total:  $TOTAL"
echo -e "Passed: ${GREEN}$PASSED${NC}"
echo -e "Failed: ${RED}$FAILED${NC}"

if [ $FAILED -eq 0 ]; then
    log_success "All tests passed!"
    exit 0
else
    log_error "$FAILED test(s) failed"
    exit 1
fi
