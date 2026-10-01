#!/bin/bash
# Entrypoint for the Horcrux ISO Docker builder container.
#
# The repo is bind-mounted read-only at /build/horcrux-src; this script
# stages a writable copy (build-gentoo-iso.sh writes into the repo tree:
# cargo build output, build/work, build/iso) and then runs the existing,
# already-complete build/scripts/build-gentoo-iso.sh, which downloads its
# own Gentoo stage3, chroots into it, emerges packages, builds a kernel,
# and produces the final ISO.

set -euo pipefail

SRC="/build/horcrux-src"
WORK="/build/horcrux"
OUTPUT_DIR="/output"

BUILD_TYPE="${BUILD_TYPE:-standard}"
SKIP_KERNEL="${SKIP_KERNEL:-true}"   # true: binary kernel (fast); false: compile from source (hours)
JOBS="${JOBS:-$(nproc)}"

echo "============================================"
echo "  Horcrux Gentoo ISO build (in-container)"
echo "  Build type: ${BUILD_TYPE}  Skip kernel build: ${SKIP_KERNEL}"
echo "============================================"

echo "[1/2] Staging writable copy of repo..."
rm -rf "${WORK}"
cp -a "${SRC}" "${WORK}"
mkdir -p "${OUTPUT_DIR}"

cd "${WORK}/build"

SKIP_FLAG=""
if [[ "${SKIP_KERNEL}" == "true" ]]; then
    SKIP_FLAG="--skip-kernel"
fi

echo "[2/2] Running build-gentoo-iso.sh..."
./scripts/build-gentoo-iso.sh \
    -a amd64 \
    -t "${BUILD_TYPE}" \
    -j "${JOBS}" \
    -o ./iso \
    ${SKIP_FLAG}

ISO_SRC="$(find "${WORK}/build/iso" -maxdepth 1 -iname '*.iso' | head -1)"
if [[ -z "${ISO_SRC}" ]]; then
    echo "Error: no ISO found under ${WORK}/build/iso" >&2
    exit 1
fi

cp -f "${ISO_SRC}" "${OUTPUT_DIR}/"
cp -f "${ISO_SRC}.sha256" "${OUTPUT_DIR}/" 2>/dev/null || true
echo "ISO copied to ${OUTPUT_DIR}/$(basename "${ISO_SRC}")"
