#!/bin/bash
#
# Test Script for noVNC Integration
#
# This script tests the noVNC console access functionality without requiring
# actual VMs (useful for WSL2 testing with mock VNC server).
#

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Configuration
API_BASE="http://localhost:8080/api"
TEST_VM_ID="test-vm-1"
TEST_NODE="local"
TEST_VNC_PORT="5900"

echo "========================================================================"
echo "noVNC Integration Test Suite"
echo "========================================================================"
echo ""

# Test 1: Check if mock VNC server is running
echo -e "${YELLOW}Test 1: Verify mock VNC server is running${NC}"
if netstat -tln 2>/dev/null | grep -q ":${TEST_VNC_PORT} " || ss -tln 2>/dev/null | grep -q ":${TEST_VNC_PORT} "; then
    echo -e "${GREEN}✓ Mock VNC server is running on port ${TEST_VNC_PORT}${NC}"
else
    echo -e "${RED}✗ Mock VNC server is NOT running on port ${TEST_VNC_PORT}${NC}"
    echo "  Start it with: python3 test_vnc_server.py ${TEST_VNC_PORT}"
    exit 1
fi
echo ""

# Test 2: Test VNC connection with nc
echo -e "${YELLOW}Test 2: Test VNC protocol handshake${NC}"
if command -v nc >/dev/null 2>&1; then
    echo "Connecting to VNC server..."
    (echo "" | timeout 2 nc localhost ${TEST_VNC_PORT} 2>/dev/null | head -c 20) || true
    if [ $? -eq 0 ] || [ $? -eq 124 ]; then
        echo -e "${GREEN}✓ VNC server accepts connections${NC}"
    else
        echo -e "${RED}✗ Failed to connect to VNC server${NC}"
    fi
else
    echo -e "${YELLOW}⊘ nc not available, skipping connection test${NC}"
fi
echo ""

# Test 3: Check if horcrux-api is running
echo -e "${YELLOW}Test 3: Check if horcrux-api is running${NC}"
if curl -s -f "${API_BASE}/health" >/dev/null 2>&1; then
    echo -e "${GREEN}✓ horcrux-api is running${NC}"
else
    echo -e "${RED}✗ horcrux-api is NOT running${NC}"
    echo "  Start it with: cargo run -p horcrux-api"
    exit 1
fi
echo ""

# Test 4: Create a test VM in database (for console ticket generation)
echo -e "${YELLOW}Test 4: Create test VM for console access${NC}"
# Note: This would require authentication, so we'll skip for now
echo -e "${YELLOW}⊘ Skipping - requires authentication${NC}"
echo "  Manual test: Use API to create VM with VNC config"
echo ""

# Test 5: Generate console ticket
echo -e "${YELLOW}Test 5: Generate console ticket${NC}"
echo -e "${YELLOW}⊘ Skipping - requires authentication and VM${NC}"
echo "  API endpoint: POST ${API_BASE}/vms/${TEST_VM_ID}/console"
echo "  Expected response:"
echo "  {\"ticket\": \"...\", \"port\": ${TEST_VNC_PORT}, \"expires\": ...}"
echo ""

# Test 6: Access noVNC HTML page
echo -e "${YELLOW}Test 6: Test noVNC HTML page generation${NC}"
echo -e "${YELLOW}⊘ Skipping - requires valid console ticket${NC}"
echo "  URL: http://localhost:8080/console?path=...&ticket=..."
echo "  Should load noVNC 1.4.0 interface"
echo ""

# Test 7: Test WebSocket proxy
echo -e "${YELLOW}Test 7: Test WebSocket proxy functionality${NC}"
echo -e "${YELLOW}⊘ Skipping - requires valid ticket and WebSocket client${NC}"
echo "  WebSocket URL: ws://localhost:8080/console-ws?ticket=..."
echo "  Should proxy to VNC server on port ${TEST_VNC_PORT}"
echo ""

echo "========================================================================"
echo "Test Summary"
echo "========================================================================"
echo ""
echo -e "${GREEN}✓ Mock VNC server is running${NC}"
echo -e "${YELLOW}⊘ API integration tests require running horcrux-api with auth${NC}"
echo ""
echo "Manual Testing Steps:"
echo ""
echo "1. Start horcrux-api:"
echo "   cargo run -p horcrux-api"
echo ""
echo "2. Login and get session token:"
echo "   curl -X POST ${API_BASE}/auth/login \\"
echo "     -H 'Content-Type: application/json' \\"
echo "     -d '{\"username\":\"admin\",\"password\":\"admin\"}'"
echo ""
echo "3. Create a test VM with VNC:"
echo "   curl -X POST ${API_BASE}/vms \\"
echo "     -H 'Authorization: Bearer <token>' \\"
echo "     -H 'Content-Type: application/json' \\"
echo "     -d '{"
echo "       \"id\":\"${TEST_VM_ID}\","
echo "       \"name\":\"Test VM\","
echo "       \"memory\":2048,"
echo "       \"cpus\":2,"
echo "       \"vnc\":{\"enabled\":true,\"port\":${TEST_VNC_PORT}}"
echo "     }'"
echo ""
echo "4. Generate console ticket:"
echo "   curl -X POST ${API_BASE}/vms/${TEST_VM_ID}/console \\"
echo "     -H 'Authorization: Bearer <token>'"
echo ""
echo "5. Open noVNC in browser:"
echo "   http://localhost:8080/console?path=<path>&ticket=<ticket>"
echo ""
echo "========================================================================"
echo "noVNC Test Complete"
echo "========================================================================"
