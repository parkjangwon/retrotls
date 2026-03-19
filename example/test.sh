#!/bin/bash

# RetroTLS Example Test Script
# Client → RetroTLS → HTTPS API

set -e

PROXY_HOST="127.0.0.1:8080"
API_HOST="https://httpbin.org"

echo "=========================================="
echo "RetroTLS Example Test"
echo "=========================================="
echo ""
echo "Architecture: Client → RetroTLS → HTTPS API"
echo "Proxy: http://$PROXY_HOST"
echo "Upstream: $API_HOST"
echo ""

# Check if retrotls is running
echo "[*] Checking if RetroTLS is running..."
if ! curl -s http://$PROXY_HOST/get > /dev/null 2>&1; then
    echo "[!] RetroTLS is not running on $PROXY_HOST"
    echo "[!] Please start it first:"
    echo "    cd .. && ./target/release/retrotls --config example/config.yaml"
    exit 1
fi
echo "[+] RetroTLS is running"
echo ""

# Test 1: GET request
echo "[Test 1] GET /get"
echo "----------------------------------------"
RESPONSE=$(curl -s http://$PROXY_HOST/get)
if echo "$RESPONSE" | grep -q '"url": "https://httpbin.org/get"'; then
    echo "[PASS] GET request successful"
else
    echo "[FAIL] GET request failed"
    echo "$RESPONSE"
    exit 1
fi
echo ""

# Test 2: POST request
echo "[Test 2] POST /post"
echo "----------------------------------------"
RESPONSE=$(curl -s -X POST http://$PROXY_HOST/post \
    -H "Content-Type: application/json" \
    -d '{"test": "hello retrotls"}')
if echo "$RESPONSE" | grep -q '"test": "hello retrotls"'; then
    echo "[PASS] POST request successful"
else
    echo "[FAIL] POST request failed"
    exit 1
fi
echo ""

# Test 3: Headers
echo "[Test 3] GET /headers (check X-Forwarded-*)"
echo "----------------------------------------"
RESPONSE=$(curl -s http://$PROXY_HOST/headers)
if echo "$RESPONSE" | grep -q '"X-Forwarded-For"'; then
    echo "[PASS] X-Forwarded headers present"
else
    echo "[WARN] X-Forwarded headers not found"
fi
echo ""

# Test 4: Query parameters
echo "[Test 4] GET /get?foo=bar"
echo "----------------------------------------"
RESPONSE=$(curl -s "http://$PROXY_HOST/get?foo=bar")
if echo "$RESPONSE" | grep -q '"foo": "bar"'; then
    echo "[PASS] Query parameters forwarded"
else
    echo "[FAIL] Query parameters test failed"
    exit 1
fi
echo ""

# Test 5: Status codes
echo "[Test 5] GET /status/418 (Teapot)"
echo "----------------------------------------"
STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://$PROXY_HOST/status/418)
if [ "$STATUS" = "418" ]; then
    echo "[PASS] Status code 418 received"
else
    echo "[FAIL] Expected 418, got $STATUS"
    exit 1
fi
echo ""

echo "=========================================="
echo "All tests passed!"
echo "=========================================="
echo ""
echo "Check RetroTLS logs for access log entries."
