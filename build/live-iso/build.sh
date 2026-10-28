#!/bin/bash
# Gentoo live/installer ISO builder (x86_64, UEFI + BIOS hybrid).
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
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
export DOCKER_BUILDKIT=1
T0=$(date +%s); lap() { echo "== [$(( $(date +%s)-T0 ))s] $*"; }
TAG=${REUSE_ROOTFS:-${BUILD_TAG:-$PROJECT-live-$$}}   # BUILD_TAG + KEEP_IMAGES=1: dev loop, keep the images for REUSE_ROOTFS
cd "$HERE"; [ -x project/prepare.sh ] && (cd "$HERE" && sh project/prepare.sh) || true

if [ -z "${REUSE_ROOTFS:-}" ]; then
lap "stage 1: application (cargo/trunk)"
docker build -q -f project/Dockerfile.app --build-arg JOBS=$JOBS -t $TAG-app "$REPO"
lap "stage 2: gentoo rootfs (stage3 + binhost + ccache)"
docker build -f common/Dockerfile.rootfs --build-arg APP_IMAGE=$TAG-app --build-arg PROJECT=$PROJECT \
    --build-arg ISO_LABEL=$ISO_LABEL --build-arg PROJECT_TITLE="$PROJECT_TITLE" -t $TAG-rootfs . 2>&1 | tee "$OUT/rootfs-build.log" | tail -40
fi
lap "stage 3: assembly toolbox"
docker build -q -f common/Dockerfile.tools -t live-iso-tools:latest .
lap "stage 4: export rootfs + assemble ISO (no bind mounts: works from a job container using the host docker)"
ISO_NAME=$ISO_PREFIX-$SUFFIX-x86_64.iso
docker rm -f $TAG-asm >/dev/null 2>&1 || true
cid=$(docker create $TAG-rootfs /bin/true)
docker export "$cid" | docker run -i --name $TAG-asm --memory=8g --cpus=$JOBS live-iso-tools:latest \
    sh -c "mkdir -p /work/root /out && tar -x -C /work/root --exclude='dev/*' && assemble-iso.sh /work/root /out/$ISO_NAME '$ISO_LABEL' '$PROJECT_TITLE' '$PROJECT'"
docker rm "$cid" >/dev/null
docker cp $TAG-asm:/out/$ISO_NAME "$OUT/$ISO_NAME"
docker rm -f $TAG-asm >/dev/null
[ -n "${REUSE_ROOTFS:-}" ] || [ -n "${KEEP_IMAGES:-}" ] || docker rmi $TAG-app $TAG-rootfs >/dev/null 2>&1 || true
( cd "$OUT" && sha256sum "$ISO_NAME" > "$ISO_NAME.sha256" && cat "$ISO_NAME.sha256" )
lap "done: $OUT/$ISO_NAME ($(du -h "$OUT/$ISO_NAME" | cut -f1))"
