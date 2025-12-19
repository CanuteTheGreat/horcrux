#!/bin/bash
#
# Build all Horcrux ISOs for all supported architectures
#
# Usage: ./build-all.sh [OPTIONS]
#
# Options:
#   -t, --type TYPE    Build type: minimal, standard, full (default: standard)
#   -o, --output DIR   Output directory
#   -p, --parallel     Build architectures in parallel
#   -h, --help         Show this help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_SCRIPT="$SCRIPT_DIR/build-iso.sh"

# Supported architectures
ARCHITECTURES=(
    "x86_64"
    "aarch64"
    "riscv64"
)

# Default values
BUILD_TYPE="${BUILD_TYPE:-standard}"
OUTPUT_DIR="${OUTPUT_DIR:-$SCRIPT_DIR/../iso}"
PARALLEL="${PARALLEL:-false}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

show_help() {
    head -15 "$0" | tail -10 | sed 's/^#//'
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -t|--type)
            BUILD_TYPE="$2"
            shift 2
            ;;
        -o|--output)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        -p|--parallel)
            PARALLEL="true"
            shift
            ;;
        -h|--help)
            show_help
            ;;
        *)
            log_error "Unknown option: $1"
            show_help
            ;;
    esac
done

log_info "=============================================="
log_info "Building Horcrux ISOs for all architectures"
log_info "=============================================="
log_info "Build type:     $BUILD_TYPE"
log_info "Output:         $OUTPUT_DIR"
log_info "Parallel:       $PARALLEL"
log_info "Architectures:  ${ARCHITECTURES[*]}"
log_info "=============================================="

mkdir -p "$OUTPUT_DIR"

build_arch() {
    local arch=$1
    log_info "Building for $arch..."

    if "$BUILD_SCRIPT" -a "$arch" -t "$BUILD_TYPE" -o "$OUTPUT_DIR"; then
        log_success "Built $arch ISO successfully"
        return 0
    else
        log_error "Failed to build $arch ISO"
        return 1
    fi
}

# Build for each architecture
failed=()

if [[ "$PARALLEL" == "true" ]]; then
    # Parallel build
    pids=()
    for arch in "${ARCHITECTURES[@]}"; do
        build_arch "$arch" &
        pids+=($!)
    done

    # Wait for all builds
    for i in "${!pids[@]}"; do
        if ! wait "${pids[$i]}"; then
            failed+=("${ARCHITECTURES[$i]}")
        fi
    done
else
    # Sequential build
    for arch in "${ARCHITECTURES[@]}"; do
        if ! build_arch "$arch"; then
            failed+=("$arch")
        fi
    done
fi

# Print summary
echo ""
log_info "=============================================="
log_info "Build Summary"
log_info "=============================================="

if [[ ${#failed[@]} -eq 0 ]]; then
    log_success "All builds completed successfully!"
    log_info ""
    log_info "Built ISOs:"
    ls -lh "$OUTPUT_DIR"/*.iso 2>/dev/null || true
else
    log_error "Some builds failed: ${failed[*]}"
    exit 1
fi
