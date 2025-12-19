#!/bin/bash
#
# Horcrux Catalyst Build Script
# Builds a bootable Gentoo-based ISO using Catalyst
#
# Usage: sudo ./build.sh [options]
#
# Options:
#   --fetch-seed    Download seed stage3 tarball
#   --snapshot      Create portage snapshot
#   --stage3        Build stage3 (optional, can use seed)
#   --livecd        Build LiveCD stages
#   --all           Run all steps
#   --clean         Clean catalyst work directories
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
SPECS_DIR="$SCRIPT_DIR/specs"
FILES_DIR="$SCRIPT_DIR/files"

# Catalyst directories
CATALYST_BASE="/var/tmp/catalyst"
BUILDS_DIR="$CATALYST_BASE/builds"
SNAPSHOTS_DIR="$CATALYST_BASE/snapshots"

# Gentoo mirrors
GENTOO_MIRROR="https://distfiles.gentoo.org"
STAGE3_URL="$GENTOO_MIRROR/releases/amd64/autobuilds"

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

check_root() {
    if [[ $EUID -ne 0 ]]; then
        log_error "This script must be run as root"
        exit 1
    fi
}

fetch_seed_stage3() {
    log_info "Fetching seed stage3 tarball..."

    mkdir -p "$BUILDS_DIR/horcrux"

    # Get latest stage3 path
    local latest_url="$STAGE3_URL/latest-stage3-amd64-openrc.txt"
    log_info "Checking: $latest_url"

    local stage3_path=$(curl -sL "$latest_url" | grep -v '^#' | grep 'stage3' | head -1 | awk '{print $1}')

    if [[ -z "$stage3_path" ]]; then
        log_error "Could not determine latest stage3 path"
        exit 1
    fi

    local stage3_url="$STAGE3_URL/$stage3_path"
    local stage3_file="$BUILDS_DIR/horcrux/stage3-amd64-openrc-latest.tar.xz"

    log_info "Downloading: $stage3_url"

    if [[ -f "$stage3_file" ]]; then
        log_warn "Stage3 already exists, skipping download"
    else
        curl -L -o "$stage3_file" "$stage3_url"
        log_success "Stage3 downloaded: $stage3_file"
    fi

    # Verify download
    if [[ -f "$stage3_file" ]]; then
        local size=$(stat -c%s "$stage3_file")
        log_info "Stage3 size: $((size / 1024 / 1024)) MB"
    fi
}

create_snapshot() {
    log_info "Creating portage snapshot..."

    mkdir -p "$SNAPSHOTS_DIR"

    local snapshot_file="$SNAPSHOTS_DIR/gentoo-latest.sqfs"

    if [[ -f "$snapshot_file" ]]; then
        log_warn "Snapshot already exists, skipping"
        return 0
    fi

    # Check if local portage tree exists
    local portage_tree="/var/db/repos/gentoo"
    if [[ ! -d "$portage_tree" ]]; then
        log_error "Portage tree not found at $portage_tree"
        log_info "Run: emerge --sync"
        exit 1
    fi

    log_info "Creating squashfs snapshot from $portage_tree"

    # Create squashfs snapshot directly from the local portage tree
    mksquashfs "$portage_tree" "$snapshot_file" \
        -comp zstd \
        -Xcompression-level 19 \
        -no-xattrs \
        -noappend \
        -progress

    if [[ -f "$snapshot_file" ]]; then
        local size=$(stat -c%s "$snapshot_file")
        log_success "Portage snapshot created: $snapshot_file ($((size / 1024 / 1024)) MB)"
    else
        log_error "Failed to create portage snapshot"
        exit 1
    fi
}

build_stage3() {
    log_info "Building stage3..."

    if [[ ! -f "$SPECS_DIR/stage3-amd64-horcrux.spec" ]]; then
        log_error "Stage3 spec file not found"
        exit 1
    fi

    catalyst -f "$SPECS_DIR/stage3-amd64-horcrux.spec"

    log_success "Stage3 build complete"
}

build_livecd_stage1() {
    log_info "Building LiveCD stage1 (squashfs)..."

    if [[ ! -f "$SPECS_DIR/livecd-stage1-amd64-horcrux.spec" ]]; then
        log_error "LiveCD stage1 spec file not found"
        exit 1
    fi

    catalyst -f "$SPECS_DIR/livecd-stage1-amd64-horcrux.spec"

    log_success "LiveCD stage1 build complete"
}

build_livecd_stage2() {
    log_info "Building LiveCD stage2 (ISO)..."

    if [[ ! -f "$SPECS_DIR/livecd-stage2-amd64-horcrux.spec" ]]; then
        log_error "LiveCD stage2 spec file not found"
        exit 1
    fi

    catalyst -f "$SPECS_DIR/livecd-stage2-amd64-horcrux.spec"

    log_success "LiveCD stage2 build complete"
}

build_livecd() {
    build_livecd_stage1
    build_livecd_stage2

    # Copy final ISO to output directory
    local iso_file=$(find "$BUILDS_DIR" -name "*.iso" -type f | head -1)
    if [[ -n "$iso_file" ]]; then
        mkdir -p "$PROJECT_ROOT/build/iso"
        cp "$iso_file" "$PROJECT_ROOT/build/iso/"
        log_success "ISO copied to: $PROJECT_ROOT/build/iso/"
    fi
}

clean_catalyst() {
    log_info "Cleaning catalyst work directories..."

    rm -rf "$CATALYST_BASE/tmp"/*

    log_success "Catalyst cleaned"
}

show_help() {
    echo "Horcrux Catalyst Build Script"
    echo ""
    echo "Usage: sudo $0 [options]"
    echo ""
    echo "Options:"
    echo "  --fetch-seed    Download seed stage3 tarball"
    echo "  --snapshot      Create portage snapshot"
    echo "  --stage3        Build stage3 (optional)"
    echo "  --livecd        Build LiveCD stages"
    echo "  --all           Run all steps (fetch, snapshot, livecd)"
    echo "  --clean         Clean catalyst work directories"
    echo "  --help          Show this help"
    echo ""
    echo "Quick start:"
    echo "  sudo $0 --all"
}

main() {
    check_root

    if [[ $# -eq 0 ]]; then
        show_help
        exit 0
    fi

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --fetch-seed)
                fetch_seed_stage3
                ;;
            --snapshot)
                create_snapshot
                ;;
            --stage3)
                build_stage3
                ;;
            --livecd)
                build_livecd
                ;;
            --all)
                fetch_seed_stage3
                create_snapshot
                build_livecd
                ;;
            --clean)
                clean_catalyst
                ;;
            --help|-h)
                show_help
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                show_help
                exit 1
                ;;
        esac
        shift
    done
}

main "$@"
