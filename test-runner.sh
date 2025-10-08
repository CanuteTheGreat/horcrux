#!/bin/bash
# Horcrux Integration Test Runner
# This script runs the full integration test suite

set -e

echo "========================================"
echo "Horcrux Integration Test Suite"
echo "========================================"
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if API server is running
echo -n "Checking if API server is running... "
if curl -s http://localhost:8006/api/health > /dev/null 2>&1; then
    echo -e "${GREEN}OK${NC}"
else
    echo -e "${RED}FAILED${NC}"
    echo ""
    echo "API server is not running. Please start it first:"
    echo "  cd horcrux-api && cargo run"
    exit 1
fi

echo ""
echo "Running unit tests..."
echo "========================================"
cargo test --lib

echo ""
echo "Running integration tests..."
echo "========================================"
cargo test --test integration_tests -- --test-threads=1

echo ""
echo -e "${GREEN}========================================"
echo "All tests passed!"
echo -e "========================================${NC}"
echo ""

# Optional: Generate test report
if command -v cargo-tarpaulin &> /dev/null; then
    echo "Generating code coverage report..."
    cargo tarpaulin --out Html --output-dir target/coverage
    echo "Coverage report generated at: target/coverage/index.html"
fi
