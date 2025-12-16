#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/_lib.sh"

CONFIG="${SCRIPT_DIR}/../configs/waf.toml"
PORT=8080

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_section() {
    echo -e "\n${BLUE}==== $1 ====${NC}"
}

log_info() {
    echo -e "${YELLOW}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_section "WAF Testing Suite"
log_info "This script tests all WAF features"

# Start a simple backend server on port 3000
log_info "Starting backend server on port 3000..."
python3 -c "
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
    def log_message(self, format, *args):
        pass  # Suppress logging

with socketserver.TCPServer(('', 3000), Handler) as httpd:
    httpd.serve_forever()
" > /dev/null 2>&1 &
BACKEND_PID=$!
sleep 1

# Start Axon with WAF config
log_info "Starting Axon with WAF configuration..."
cargo build --release 2>&1 | grep -v "Compiling\|Finished" || true
cargo run --release -- serve --config "$CONFIG" > /tmp/axon_waf.log 2>&1 &
AXON_PID=$!
sleep 3

# Cleanup function
cleanup() {
    log_info "Cleaning up..."
    kill $AXON_PID 2>/dev/null || true
    kill $BACKEND_PID 2>/dev/null || true
    rm -f /tmp/axon_waf.log
}
trap cleanup EXIT

# Test counters
PASSED=0
FAILED=0

# Test function
test_waf() {
    local test_name="$1"
    local url="$2"
    local expected_status="$3"
    local extra_args="$4"
    
    log_info "Testing: $test_name"
    
    # Use a normal browser User-Agent to avoid bot detection
    local ua="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"
    
    if [ -n "$extra_args" ]; then
        response=$(curl -s -w "\n%{http_code}" -A "$ua" $extra_args "http://localhost:${PORT}${url}" 2>/dev/null || echo "000")
    else
        response=$(curl -s -w "\n%{http_code}" -A "$ua" "http://localhost:${PORT}${url}" 2>/dev/null || echo "000")
    fi
    
    status_code=$(echo "$response" | tail -n1)
    
    if [ "$status_code" = "$expected_status" ]; then
        log_success "✓ $test_name - Got expected status $status_code"
        ((PASSED++))
    else
        log_error "✗ $test_name - Expected $expected_status but got $status_code"
        ((FAILED++))
    fi
}

log_section "1. Testing Normal Requests (Should Pass)"
test_waf "Normal GET request" "/api/users" "200" ""
test_waf "Normal POST request" "/api/data" "200" "-X POST -d '{\"data\":\"test\"}'"

log_section "2. Testing SQL Injection Detection (Should Block)"
test_waf "SQL Injection - UNION SELECT" "/api/users?id=1' UNION SELECT * FROM users--" "403" ""
test_waf "SQL Injection - OR 1=1" "/api/login?user=admin' OR '1'='1" "403" ""
test_waf "SQL Injection - DROP TABLE" "/api/data?id=1; DROP TABLE users--" "403" ""
test_waf "SQL Injection in body" "/api/search" "403" "-X POST -d 'query=1 OR 1=1'"

log_section "3. Testing XSS Detection (Should Block)"
test_waf "XSS - Script tag" "/api/comment?text=<script>alert('xss')</script>" "403" ""
test_waf "XSS - Event handler" "/api/post?content=<img src=x onerror=alert(1)>" "403" ""
test_waf "XSS - JavaScript protocol" "/api/link?url=javascript:alert(1)" "403" ""
test_waf "XSS in body" "/api/comment" "403" "-X POST -d 'text=<script>document.cookie</script>'"

log_section "4. Testing Command Injection Detection (Should Block)"
test_waf "Command Injection - Pipe" "/api/exec?cmd=ls | cat /etc/passwd" "403" ""
test_waf "Command Injection - Semicolon" "/api/run?cmd=echo test; rm -rf /" "403" ""
test_waf "Command Injection - Backticks" "/api/shell?cmd=\`whoami\`" "403" ""
test_waf "Command Injection - Dollar paren" "/api/exec?cmd=\$(cat /etc/passwd)" "403" ""

log_section "5. Testing Path Traversal Detection (Should Block)"
test_waf "Path Traversal - Basic" "/api/file?path=../../../../etc/passwd" "403" ""
test_waf "Path Traversal - URL encoded" "/api/file?path=..%2F..%2F..%2Fetc%2Fpasswd" "403" ""
test_waf "Path Traversal - Double encoded" "/api/file?path=..%252F..%252Fetc%252Fpasswd" "403" ""
test_waf "Path Traversal - Backslash" "/api/file?path=..\\..\\..\\windows\\system32" "403" ""

log_section "6. Testing Bot Detection (Should Block Bad Bots)"
test_waf "Good Bot - Googlebot" "/api/data" "200" "-A 'Mozilla/5.0 (compatible; Googlebot/2.1)'"
test_waf "Good Bot - Bingbot" "/api/data" "200" "-A 'Mozilla/5.0 (compatible; bingbot/2.0)'"
test_waf "Bad Bot - Scraper" "/api/data" "403" "-A 'python-requests/2.28.0'"
test_waf "Bad Bot - wget" "/api/data" "403" "-A 'Wget/1.20.3'"

log_section "7. Testing Edge Cases (Should Pass)"
test_waf "URL with safe special chars" "/api/search?q=hello+world&sort=asc" "200" ""
test_waf "Request with JSON body" "/api/create" "200" "-X POST -H 'Content-Type: application/json' -d '{\"name\":\"test\",\"value\":123}'"

# Summary
log_section "Test Results Summary"
TOTAL=$((PASSED + FAILED))
log_info "Total tests: $TOTAL"
log_success "Passed: $PASSED"
if [ $FAILED -gt 0 ]; then
    log_error "Failed: $FAILED"
    exit 1
else
    log_success "All tests passed! ✓"
fi
