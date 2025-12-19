#!/bin/bash
#
# Run Catalyst to build Horcrux LiveCD
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPEC_DIR="$SCRIPT_DIR/specs"
OUTPUT_DIR="$(dirname "$SCRIPT_DIR")/iso"

STAGE1_SPEC="$SPEC_DIR/livecd-stage1-amd64-horcrux.spec"
STAGE2_SPEC="$SPEC_DIR/livecd-stage2-amd64-horcrux.spec"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

check_catalyst_running() {
    if pgrep -f "catalyst.*\.spec" >/dev/null; then
        log_error "Another catalyst process is running. Please wait for it to finish."
        echo "Active catalyst processes:"
        ps aux | grep -E "catalyst.*\.spec" | grep -v grep
        return 1
    fi
    return 0
}

build_stage1() {
    log_info "Building Horcrux LiveCD Stage 1..."
    log_info "Spec file: $STAGE1_SPEC"
    echo ""

    sudo catalyst -a -f "$STAGE1_SPEC" 2>&1 | tee /tmp/horcrux-catalyst-stage1.log

    if [[ ${PIPESTATUS[0]} -eq 0 ]]; then
        log_success "Stage 1 complete!"
        return 0
    else
        log_error "Stage 1 failed. Check /tmp/horcrux-catalyst-stage1.log"
        return 1
    fi
}

build_stage2() {
    log_info "Building Horcrux LiveCD Stage 2 (ISO)..."
    log_info "Spec file: $STAGE2_SPEC"
    echo ""

    sudo catalyst -a -f "$STAGE2_SPEC" 2>&1 | tee /tmp/horcrux-catalyst-stage2.log

    if [[ ${PIPESTATUS[0]} -eq 0 ]]; then
        log_success "Stage 2 complete!"

        # Copy ISO to output directory
        local iso_file="/var/tmp/catalyst/builds/horcrux-amd64-latest.iso"
        if [[ -f "$iso_file" ]]; then
            mkdir -p "$OUTPUT_DIR"
            cp "$iso_file" "$OUTPUT_DIR/"
            log_success "ISO copied to: $OUTPUT_DIR/$(basename "$iso_file")"
        fi
        return 0
    else
        log_error "Stage 2 failed. Check /tmp/horcrux-catalyst-stage2.log"
        return 1
    fi
}

show_help() {
    echo "Horcrux Catalyst Build Script"
    echo ""
    echo "Usage: $0 [stage1|stage2|all]"
    echo ""
    echo "Commands:"
    echo "  stage1    Build LiveCD stage 1 (squashfs root)"
    echo "  stage2    Build LiveCD stage 2 (bootable ISO)"
    echo "  all       Build both stages (default)"
    echo ""
}

main() {
    local cmd="${1:-all}"

    case "$cmd" in
        stage1)
            check_catalyst_running || exit 1
            build_stage1
            ;;
        stage2)
            check_catalyst_running || exit 1
            build_stage2
            ;;
        all)
            check_catalyst_running || exit 1
            build_stage1 && build_stage2
            ;;
        -h|--help|help)
            show_help
            ;;
        *)
            log_error "Unknown command: $cmd"
            show_help
            exit 1
            ;;
    esac
}

main "$@"
