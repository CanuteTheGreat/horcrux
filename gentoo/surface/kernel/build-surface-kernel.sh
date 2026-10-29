#!/bin/bash
# Fetch vanilla linux-$KERNEL_VERSION and the pinned linux-surface patch series, apply them IN ORDER and (optionally)
# build. Only needed for Surface Pro 3+/Laptop/Book (SAM, IPTS/ITHC touch, cameras); the Surface Pro 1/2 runs the
# stock Gentoo kernel (see ../README.md). Why 7.2: OpenZFS 2.4.x supports <= 7.2 (docs/T2-MACS.md), and the series
# exists for 7.2. Run on a build host (never on a 4 GB tablet):   ./build-surface-kernel.sh WORKDIR [apply|build]
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); . "$HERE/linux-surface.pin"
W=${1:?workdir}; MODE=${2:-apply}; mkdir -p "$W"; cd "$W"
[ -f "linux-$KERNEL_VERSION.tar.xz" ] || curl -fsSLO "https://cdn.kernel.org/pub/linux/kernel/v${KERNEL_VERSION%%.*}.x/linux-$KERNEL_VERSION.tar.xz"
[ -d "linux-$KERNEL_VERSION" ] || tar xf "linux-$KERNEL_VERSION.tar.xz"
mkdir -p patches
n=0
for f in $(curl -fsSL "https://api.github.com/repos/linux-surface/linux-surface/contents/patches/$LS_SERIES?ref=$LS_COMMIT" | python3 -c 'import json,sys; print(" ".join(x["name"] for x in json.load(sys.stdin) if x["name"].endswith(".patch")))'); do
    [ -f "patches/$f" ] || curl -fsSL -o "patches/$f" "https://raw.githubusercontent.com/linux-surface/linux-surface/$LS_COMMIT/patches/$LS_SERIES/$f"
done
cd "linux-$KERNEL_VERSION"
for p in ../patches/*.patch; do
    # apply for real, in order (a per-file --dry-run reports false failures for patches that touch one file twice)
    if patch -Np1 --no-backup-if-mismatch < "$p" >../apply.log 2>&1; then echo "applied  $(basename "$p")"; n=$((n+1)); else echo "FAILED   $(basename "$p")"; tail -5 ../apply.log; exit 1; fi
done
echo "all $n patches applied to linux-$KERNEL_VERSION"
[ "$MODE" = build ] || exit 0
make defconfig >/dev/null; ./scripts/kconfig/merge_config.sh -m .config "$HERE/linux-surface-$LS_SERIES.config" >/dev/null
make olddefconfig >/dev/null; make -j"$(nproc)" bzImage modules
