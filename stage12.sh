#!/bin/bash
# stage 1+2 of build.sh only (app image + server rootfs), tagged for REUSE_ROOTFS. Runs on a rw-man host.
set -euo pipefail
cd "$(dirname "$0")/build/live-iso"
HERE=$PWD; REPO=$(cd ../.. && pwd)
. project/project.env
ARCH=x86_64; . common/arch/$ARCH.env
export DOCKER_BUILDKIT=1
TAG=${1:-hsm}
ARCHARGS="--build-arg ARCH=$ARCH --build-arg STAGE3_TAG=$STAGE3_TAG"
echo "== $(date -Is) stage1 app"
sudo -E docker build -f project/Dockerfile.app --build-arg JOBS=${JOBS:-4} -t $TAG-app "$REPO" > "$REPO/stage1.log" 2>&1 || { tail -30 "$REPO/stage1.log"; exit 1; }
echo "== $(date -Is) stage2 rootfs"
sudo -E docker build $ARCHARGS -f common/Dockerfile.rootfs --build-arg APP_IMAGE=$TAG-app --build-arg PROJECT=$PROJECT \
    --build-arg ISO_LABEL=$ISO_LABEL --build-arg PROJECT_TITLE="$PROJECT_TITLE" -t $TAG-rootfs . > "$REPO/stage2.log" 2>&1 || { tail -40 "$REPO/stage2.log"; exit 1; }
echo "== $(date -Is) DONE rootfs $TAG-rootfs"
