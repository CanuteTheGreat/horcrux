#!/bin/bash
# gentoo/t2/live/build.sh - build the hogshead (Apple T2 MacBook) live ISO. Run on an rw-man host (docker + BuildKit only).
#   gentoo/t2/live/build.sh -o OUTDIR [-p PRIVDIR] [-n NAME]
#   PRIVDIR (never committed): fw/ (Apple brcm firmware), authorized_keys, ts-authkey. Without it the ISO has none of them
#   (= a shareable T2 live ISO; Wi-Fi then needs firmware imported by the user).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); T2=$(cd "$HERE/.." && pwd); REPO=$(cd "$T2/../.." && pwd)
. "$T2/PATCHSET"
OUT=$REPO/build/iso; PRIV=""; NAME=hogshead-live-$(date +%Y%m%d)-x86_64.iso
while getopts o:p:n: o; do case $o in o) OUT=$OPTARG;; p) PRIV=$OPTARG;; n) NAME=$OPTARG;; *) exit 2;; esac; done
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
export DOCKER_BUILDKIT=1
TAG=hs-live-$$
WORK=$(mktemp -d "${AGENT_WORKDIR:-${TMPDIR:-/tmp}}/hs-ctx.XXXXXX")
cid=
cleanup() {
  docker rm -fv "$TAG-asm" >/dev/null 2>&1 || true
  [ -z "$cid" ] || docker rm -fv "$cid" >/dev/null 2>&1 || true
  docker rmi "$TAG-tools" "$TAG-tools2" "$TAG-rootfs" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT; trap 'exit 143' TERM INT
T0=$(date +%s); lap() { echo "== [$(( $(date +%s)-T0 ))s] $*"; }

lap "stage context"
mkdir -p "$WORK/ctx/build/live-iso/common" "$WORK/ctx/gentoo" "$WORK/priv"
cp -r "$REPO/build/live-iso/common/." "$WORK/ctx/build/live-iso/common/"
cp -r "$T2" "$WORK/ctx/gentoo/t2"
if [ -n "$PRIV" ]; then cp -r "$PRIV"/. "$WORK/priv/"; fi
sh "$T2/kernel/fetch-patches.sh" "$WORK/patches"

lap "stage 1: kernel ($KERNEL_PKG + t2linux patches) + OpenZFS + userland"
docker build -f "$WORK/ctx/gentoo/t2/live/Dockerfile" \
  --build-context t2patches="$WORK/patches" --build-context priv="$WORK/priv" \
  --build-arg KERNEL_PKG="$KERNEL_PKG" --build-arg KERNEL_SRC_DIR="$KERNEL_SRC_DIR" --build-arg LOCALVERSION="$LOCALVERSION" \
  -t "$TAG-rootfs" "$WORK/ctx" 2>&1 | tee "$OUT/rootfs-build.log" | tail -25

lap "stage 2: assembly toolbox"
# the tools Dockerfile expects the build/live-iso context layout (common/...)
docker build -q -f "$WORK/ctx/build/live-iso/common/Dockerfile.tools" --build-arg ARCH=x86_64 -t "$TAG-tools" "$WORK/ctx/build/live-iso" >/dev/null
printf 'FROM %s-tools\nCOPY assemble-hogshead-iso.sh /usr/local/bin/assemble-iso.sh\nRUN chmod +x /usr/local/bin/assemble-iso.sh\n' "$TAG" > "$WORK/Dockerfile.tools2"
cp "$HERE/assemble-hogshead-iso.sh" "$WORK/"
docker build -q -f "$WORK/Dockerfile.tools2" -t "$TAG-tools2" "$WORK" >/dev/null

lap "stage 3: export rootfs + assemble ISO"
cid=$(docker create "$TAG-rootfs" /bin/true)
docker export "$cid" | docker run -i -e ARCH=x86_64 --name "$TAG-asm" --memory=10g --cpus=4 "$TAG-tools2" \
  sh -c "mkdir -p /work/root /out && tar -x -C /work/root --exclude='dev/*' && assemble-iso.sh /work/root /out/$NAME HOGSHEAD_LIVE 'Hogshead' hogshead"
docker rm -v "$cid" >/dev/null; cid=
docker cp "$TAG-asm:/out/$NAME" "$OUT/$NAME"
docker cp "$TAG-asm:/work/root/boot/." "$WORK/boot" 2>/dev/null || true
( cd "$OUT" && sha256sum "$NAME" > "$NAME.sha256" && cat "$NAME.sha256" )
lap "done: $OUT/$NAME ($(du -h "$OUT/$NAME" | cut -f1))"
