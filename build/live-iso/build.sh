#!/bin/bash
# Gentoo live/installer ISO builder: x86_64 (UEFI + BIOS hybrid), aarch64 / riscv64 (UEFI only).
#   ARCH=x86_64|aarch64|riscv64 (default: the host's; per-arch facts in common/arch/<arch>.env)
#   build/live-iso/build.sh [-o OUTDIR] [-n ISO-NAME-SUFFIX]
# Needs docker (BuildKit) only. No privileged containers, no AWS.
# Layers: <project> app image -> gentoo rootfs image -> export -> assemble ISO.
# BuildKit cache mounts keep binpkgs / ccache / cargo target between runs.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
OUT=$REPO/build/iso; SUFFIX=$(date +%Y.%m.%d)
JOBS=${JOBS:-4}
while getopts o:n: o; do case $o in o) OUT=$OPTARG;; n) SUFFIX=$OPTARG;; *) exit 2;; esac; done
. "$HERE/project/project.env"
case $(uname -m) in x86_64|amd64) HOST_ARCH=x86_64;; aarch64|arm64) HOST_ARCH=aarch64;; riscv64) HOST_ARCH=riscv64;; *) HOST_ARCH=$(uname -m);; esac
ARCH=${ARCH:-$HOST_ARCH}
[ -f "$HERE/common/arch/$ARCH.env" ] || { echo "unsupported ARCH=$ARCH (see common/arch/)"; exit 2; }
. "$HERE/common/arch/$ARCH.env"
# Native builds only (x86_64 on rw-man-01..03, aarch64 on rw-man-04/05). A foreign ARCH needs --platform + binfmt
# (qemu-user); the app stage then cannot use the host toolchain images, so it is refused unless forced.
if [ "$ARCH" != "$HOST_ARCH" ] && [ -z "${ALLOW_EMULATED:-}" ]; then echo "ARCH=$ARCH on a $HOST_ARCH host needs emulation: set ALLOW_EMULATED=1 (slow)"; exit 2; fi
PLAT=""; [ "$ARCH" != "$HOST_ARCH" ] && PLAT="--platform $DOCKER_PLATFORM"
ARCHARGS="--build-arg ARCH=$ARCH --build-arg STAGE3_TAG=$STAGE3_TAG"
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
export DOCKER_BUILDKIT=1
T0=$(date +%s); lap() { echo "== [$(( $(date +%s)-T0 ))s] $*"; }
TAG=${REUSE_ROOTFS:-${BUILD_TAG:-$PROJECT-live-$$}}   # BUILD_TAG + KEEP_IMAGES=1: dev loop, keep the images for REUSE_ROOTFS
cid=
# Always-run cleanup (success, failure, SIGTERM): this build's assembly container (+ its anonymous volumes), the export
# container, the per-build tools image and, unless kept for the dev loop, the app/rootfs images. Layers stay in the
# BuildKit cache, which is capped by the janitor, so a rebuild is still fast.
cleanup_build() {
  docker rm -fv "$TAG-asm" >/dev/null 2>&1 || true
  [ -z "$cid" ] || docker rm -fv "$cid" >/dev/null 2>&1 || true
  docker rmi "$TAG-tools" >/dev/null 2>&1 || true
  [ -z "${VARIANT:-}" ] || docker rmi "$TAG-rootfs-$VARIANT" >/dev/null 2>&1 || true
  [ -n "${REUSE_ROOTFS:-}" ] || [ -n "${KEEP_IMAGES:-}" ] || docker rmi "$TAG-app" "$TAG-rootfs" >/dev/null 2>&1 || true
}
trap cleanup_build EXIT
trap 'exit 143' TERM INT
cd "$HERE"; [ -x project/prepare.sh ] && (cd "$HERE" && sh project/prepare.sh) || true

if [ -z "${REUSE_ROOTFS:-}" ]; then
lap "stage 1: application (cargo/trunk)"
docker build $PLAT -f project/Dockerfile.app --build-arg JOBS=$JOBS -t $TAG-app "$REPO" > "$OUT/app-build.log" 2>&1 \
    || { echo "application build failed (full log: $OUT/app-build.log)"; tail -40 "$OUT/app-build.log"; exit 1; }
lap "stage 2: gentoo rootfs (stage3 + binhost + ccache)"
docker build $PLAT $ARCHARGS -f common/Dockerfile.rootfs --build-arg APP_IMAGE=$TAG-app --build-arg PROJECT=$PROJECT \
    --build-arg ISO_LABEL=$ISO_LABEL --build-arg PROJECT_TITLE="$PROJECT_TITLE" -t $TAG-rootfs . 2>&1 | tee "$OUT/rootfs-build.log" | tail -40
fi
# Optional image variant (VARIANT=desktop): a second layer on top of the finished rootfs, project/variants/<name>/Dockerfile.
# The server image stays the default; the variant image is a separate ISO (<prefix>-<variant>-<date>-<arch>.iso).
ROOTFS_IMG=$TAG-rootfs; VNAME=""
if [ -n "${VARIANT:-}" ] && [ "$VARIANT" != server ]; then
  [ -f "project/variants/$VARIANT/Dockerfile" ] || { echo "unknown VARIANT=$VARIANT (no project/variants/$VARIANT/Dockerfile)"; exit 2; }
  lap "stage 2b: variant $VARIANT"
  docker build $PLAT $ARCHARGS -f "project/variants/$VARIANT/Dockerfile" --build-arg BASE=$TAG-rootfs --build-arg VARIANT=$VARIANT \
      -t $TAG-rootfs-$VARIANT . 2>&1 | tee "$OUT/variant-$VARIANT-build.log" | tail -40
  [ "${PIPESTATUS[0]}" = 0 ] || { echo "variant build failed (log: $OUT/variant-$VARIANT-build.log)"; exit 1; }
  ROOTFS_IMG=$TAG-rootfs-$VARIANT; VNAME=$VARIANT-
fi
lap "stage 3: assembly toolbox"
# Per-build tag: a shared live-iso-tools:latest could be untagged by the runner disk guard (or replaced by a
# parallel build) between this build and the docker run below (Horcrux run 7066: "pull access denied").
docker build $PLAT $ARCHARGS -q -f common/Dockerfile.tools -t $TAG-tools .
lap "stage 4: export rootfs + assemble ISO (no bind mounts: works from a job container using the host docker)"
ISO_NAME=$ISO_PREFIX-${VNAME}$SUFFIX-$ARCH.iso
docker rm -f $TAG-asm >/dev/null 2>&1 || true
cid=$(docker create $ROOTFS_IMG /bin/true)
docker export "$cid" | docker run -i $PLAT -e ARCH=$ARCH --name $TAG-asm --memory=8g --cpus=$JOBS $TAG-tools \
    sh -c "mkdir -p /work/root /out && tar -x -C /work/root --exclude='dev/*' && assemble-iso.sh /work/root /out/$ISO_NAME '$ISO_LABEL' '$PROJECT_TITLE' '$PROJECT'"
docker rm -v "$cid" >/dev/null; cid=
docker cp $TAG-asm:/out/$ISO_NAME "$OUT/$ISO_NAME"
( cd "$OUT" && sha256sum "$ISO_NAME" > "$ISO_NAME.sha256" && cat "$ISO_NAME.sha256" )
lap "done: $OUT/$ISO_NAME ($(du -h "$OUT/$ISO_NAME" | cut -f1))"
