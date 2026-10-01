#!/bin/bash
# Build the Horcrux Gentoo LiveCD/installer ISO using Docker.
#
# Mirrors gentoo's patronus/gentoo/docker/build-iso-docker.sh pattern:
# cross-platform host (ARM64 build machine) -> --platform=linux/amd64
# container (QEMU/Rosetta emulated) -> privileged chroot-based ISO build.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OUTPUT_DIR="${OUTPUT_DIR:-${REPO_ROOT}/build/iso}"
IMAGE_TAG="horcrux-iso-builder:latest"

# standard: fast binary-kernel build for dev/test/smoke-testing (minutes).
# Set SKIP_KERNEL=false for a from-source kernel build (hours) -- closer
# to a real production/release build, same test/release split as Patronus's
# RUST_VARIANT switch.
BUILD_TYPE="${BUILD_TYPE:-standard}"
SKIP_KERNEL="${SKIP_KERNEL:-true}"

echo "============================================"
echo "  Horcrux LiveCD ISO Builder (Docker)"
echo "============================================"
echo "Build type: ${BUILD_TYPE}"
echo "Skip kernel build: ${SKIP_KERNEL} ($( [[ "${SKIP_KERNEL}" == true ]] && echo 'binary kernel -- dev/test, fast' || echo 'compiled from source -- release-grade, hours' ))"
echo ""

if ! command -v docker &> /dev/null; then
    echo "Error: Docker is required but not installed"
    exit 1
fi

mkdir -p "${OUTPUT_DIR}"

echo "[1/3] Building Docker image..."
docker build -t "${IMAGE_TAG}" \
    -f "${SCRIPT_DIR}/Dockerfile.iso-builder" \
    "${REPO_ROOT}/build"

echo "[2/3] Building ISO in container (this takes a while)..."
docker run --rm \
    --privileged \
    -v "${REPO_ROOT}:/build/horcrux-src:ro" \
    -v "${OUTPUT_DIR}:/output" \
    -e BUILD_TYPE="${BUILD_TYPE}" \
    -e SKIP_KERNEL="${SKIP_KERNEL}" \
    "${IMAGE_TAG}"

echo "[3/3] Verifying output..."
ISO_FILE="$(find "${OUTPUT_DIR}" -maxdepth 1 -iname '*.iso' | head -1)"

if [[ -n "${ISO_FILE}" && -f "${ISO_FILE}" ]]; then
    echo ""
    echo "============================================"
    echo "  Build Complete!"
    echo "============================================"
    echo "ISO: ${ISO_FILE}"
    echo "Size: $(du -h "${ISO_FILE}" | cut -f1)"
    echo ""
    echo "Test with QEMU:"
    echo "  qemu-system-x86_64 -cdrom ${ISO_FILE} -m 2048 -enable-kvm"
else
    echo "Error: ISO build failed"
    exit 1
fi
