#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG="${SCRIPT_DIR}/../configs/waf.toml"
PORT=8080

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_section() { echo -e "\n${BLUE}==== $1 ====${NC}"; }
log_info() { echo -e "${YELLOW}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

log_section "WAF Testing Suite - Manual Mode"
log_info "Starting backend on port 3000 and Axon on port $PORT"
log_info "Please run in separate terminals:"
log_info "  Terminal 1: python3 -m http.server 3000"
log_info "  Terminal 2: cargo run --release -- serve --config $CONFIG"
log_info ""
read -p "Press Enter when both servers are running..."

# Test counters
PASSED=0
FAILED=0
TOTAL=0

# User-Agent for normal browser
UA="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"

test_waf() {
    local test_name="$1"
    local url="$2"
    local expected_status="$3"
    shift 3
    
    ((TOTAL++))
    log_info "Test $TOTAL: $test_name"
    
    response=$(curl -s -w "\n%{http_code}" -A "$UA" "$@" "http://localhost:${PORT}${url}" 2>/dev/null || echo "000")
    status_code=$(echo "$response" | tail -n1)
    
    if [ "$status_code" = "$expected_status" ]; then
        log_success "✓ Expected $expected_status, got $status_code"
        ((PASSED++))
    else
        log_error "✗ Expected $expected_status, got $status_code"
        ((FAILED++))
    fi
}

log_section "1. Normal Requests (Should Pass - 200)"
test_waf "Normal GET request" "/api/users" "200"
test_waf "Normal POST request" "/api/data" "200" -X POST -d '{"data":"test"}'

log_section "2. SQL Injection (Should Block - 403)"
test_waf "UNION SELECT" "/api/users?id=1' UNION SELECT * FROM users--" "403"
test_waf "OR 1=1" "/api/login?user=admin' OR '1'='1" "403"
test_waf "DROP TABLE" "/api/data?id=1; DROP TABLE users--" "403"
test_waf "SQL in body" "/api/search" "403" -X POST -d "query=1 OR 1=1"

log_section "3. XSS Detection (Should Block - 403)"
test_waf "Script tag" "/api/comment?text=<script>alert('xss')</script>" "403"
test_waf "Event handler" "/api/post?content=<img src=x onerror=alert(1)>" "403"
test_waf "JavaScript protocol" "/api/link?url=javascript:alert(1)" "403"
test_waf "XSS in body" "/api/comment" "403" -X POST -d 'text=<script>alert(1)</script>'

log_section "4. Command Injection (Should Block - 403)"
test_waf "Pipe command" "/api/exec?cmd=ls | cat /etc/passwd" "403"
test_waf "Semicolon command" "/api/run?cmd=echo test; rm -rf /" "403"
test_waf "Backtick command" "/api/shell?cmd=\`whoami\`" "403"
test_waf "Dollar paren" "/api/exec?cmd=\$(cat /etc/passwd)" "403"

log_section "5. Path Traversal (Should Block - 403)"
test_waf "Basic traversal" "/api/file?path=../../../../etc/passwd" "403"
test_waf "URL encoded" "/api/file?path=..%2F..%2F..%2Fetc%2Fpasswd" "403"
test_waf "Double encoded" "/api/file?path=..%252F..%252Fetc%252Fpasswd" "403"
test_waf "Backslash" "/api/file?path=..\\..\\..\\windows\\system32" "403"

log_section "6. Bot Detection"
test_waf "Good bot - Googlebot" "/api/data" "200" -A "Mozilla/5.0 (compatible; Googlebot/2.1)"
test_waf "Good bot - Bingbot" "/api/data" "200" -A "Mozilla/5.0 (compatible; bingbot/2.0)"
test_waf "Bad bot - Python" "/api/data" "403" -A "python-requests/2.28.0"
test_waf "Bad bot - wget" "/api/data" "403" -A "Wget/1.20.3"
test_waf "Bad bot - curl" "/api/data" "403" -A "curl/7.68.0"

log_section "7. Edge Cases (Should Pass - 200)"
test_waf "Safe special chars" "/api/search?q=hello+world&sort=asc" "200"
test_waf "JSON body" "/api/create" "200" -X POST -H "Content-Type: application/json" -d '{"name":"test","value":123}'

# Summary
log_section "Test Results Summary"
log_info "Total tests: $TOTAL"
log_success "Passed: $PASSED"
if [ $FAILED -gt 0 ]; then
    log_error "Failed: $FAILED"
    exit 1
else
    log_success "All $TOTAL tests passed! ✓"
fi
