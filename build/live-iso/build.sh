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
  docker rmi "$TAG-tools" "$TAG-hw" "$TAG-priv" >/dev/null 2>&1 || true
  rm -rf "$HERE/.hw"
  [ -z "${VIMGS:-}" ] || [ -n "${KEEP_IMAGES:-}" ] || docker rmi $VIMGS >/dev/null 2>&1 || true
  [ -n "${REUSE_ROOTFS:-}" ] || [ -n "${KEEP_IMAGES:-}" ] || docker rmi "$TAG-app" "$TAG-rootfs" >/dev/null 2>&1 || true
}
trap cleanup_build EXIT
trap 'exit 143' TERM INT
cd "$HERE"; [ -x project/prepare.sh ] && (cd "$HERE" && sh project/prepare.sh) || true

if [ -z "${REUSE_ROOTFS:-}" ]; then
lap "stage 1: application (cargo/trunk)"
docker build $PLAT -f project/Dockerfile.app --build-arg JOBS=$JOBS -t $TAG-app "$REPO" > "$OUT/app-build.log" 2>&1 \
    || { echo "application build failed (full log: $OUT/app-build.log)"; tail -40 "$OUT/app-build.log"; exit 1; }
# the manager tarball (what app-emulation/horcrux-bin installs) is kept next to the ISO so the daily release can publish it: horcrux-bin fetches
# .../releases/download/daily/horcrux-manager-daily-<arch>.tar.zst on an installed machine, and that asset never existed (404; the 3 test laptops could not
# install the manager until the lab copied the tarball out of the app image by hand)
if [ "$PROJECT" = horcrux ]; then
  mkdir -p "$OUT/mgr"; _mc=$(docker create $PLAT "$TAG-app" true 2>/dev/null) \
    && docker cp "$_mc:/out/usr/share/horcrux/prebuilt/." "$OUT/mgr/" >/dev/null 2>&1; [ -z "$_mc" ] || docker rm -f "$_mc" >/dev/null 2>&1 || true
fi
lap "stage 2: gentoo rootfs (stage3 + binhost + ccache)"
docker build $PLAT $ARCHARGS -f common/Dockerfile.rootfs --build-arg APP_IMAGE=$TAG-app --build-arg PROJECT=$PROJECT \
    --build-arg ISO_LABEL=$ISO_LABEL --build-arg PROJECT_TITLE="$PROJECT_TITLE" -t $TAG-rootfs . 2>&1 | tee "$OUT/rootfs-build.log" | tail -40
fi
# Optional image variants, applied in order on top of the finished rootfs: VARIANT=gnome-light | t2 | gnome-light,t2 (`desktop` = old alias of gnome-light)
# (project/variants/<name>/Dockerfile, ARG BASE = the previous image). The server image stays the default; a variant image
# is a separate ISO named <prefix>[-<hw profile>][-<hw variant>]-<DE|server>-<init>-<channel>-<arch>.iso (common/image-name.sh: horcrux-gnome-light-openrc-daily-x86_64.iso). `t2` needs the repo's gentoo/t2 (named build context).
. "$HERE/common/image-name.sh"      # canonical VARIANT (desktop -> gnome-light), IMG=<DE|server>-<init>, IMG_HW
# a hardware/private variant that sits on top of a desktop rootfs states its desktop environment in project/variants/<v>/de, so its ISO name says
# DE + init (horcrux-gringotts-gnome-light-openrc-<date>-x86_64.iso) although the variant list names no DE variant (REUSE_ROOTFS = the desktop rootfs)
for _v in $(echo "${VARIANT:-}" | tr ',' ' '); do
  [ -f "project/variants/$_v/de" ] && { IMG_DE=$(tr -d ' \n' < "project/variants/$_v/de"); IMG=$IMG_DE-$IMG_INIT; }
done
ROOTFS_IMG=$TAG-rootfs; VIMGS=""
# LAYERS: core (project/variants/core: the headless server base, sys-apps/horcrux-core) -> desktop edition (gnome-light, ...) -> machine layer.
# The core layer is applied to EVERY image of a product that ships one (the plain server image is "core" alone), before the variants; it is idempotent, so a
# REUSE_ROOTFS base that already has it passes through. NO_CORE=1 skips it (Patronus has no core layer; hardware-profile experiments).
CHAIN=$(echo "${VARIANT:-}" | tr ',' ' ' | sed 's/\bserver\b//g')
# (the Surface image is its own base with its own overlay checkout + chrony/backup in its meta package: it takes the core as a dependency of its meta package in a later
# revision, not as an image layer; the layer would clash with its `rm -rf /var/db/repos/horcrux`)
case " $CHAIN " in *" surface-kernel "*|*" surface "*) NO_CORE=${NO_CORE:-1};; esac
[ -z "${NO_CORE:-}" ] && [ -f project/variants/core/Dockerfile ] && CHAIN="core $CHAIN"
if [ -n "$(echo $CHAIN)" ]; then
  for v in $CHAIN; do
    [ -f "project/variants/$v/Dockerfile" ] || { echo "unknown VARIANT=$v (no project/variants/$v/Dockerfile)"; exit 2; }
    lap "stage 2b: variant $v"
    VCTX="--build-context hxgentoo=$REPO/gentoo ${BUILD_CONTEXTS:-}"; [ -d "$REPO/gentoo/t2" ] && VCTX="$VCTX --build-context t2=$REPO/gentoo/t2"
    # named contexts for variants: `hxoverlay` = the Horcrux Portage overlay (repo's gentoo/), `priv` = PRIVATE inputs (PRIV_DIR, never in the
    # repo: authorized_keys + ts-authkey for the gringotts image; empty = a public image without them)
    if [ -d "$REPO/gentoo" ]; then
      # hxoverlay = the overlay the long cached layers were built with (HXO_EARLY, default the whole repo gentoo/), hxfull = the whole overlay
      # (variants copy it after the long layers so overlay edits do not invalidate them)
      VCTX="$VCTX --build-context hxoverlay=${HXO_EARLY:-$REPO/gentoo} --build-context hxfull=$REPO/gentoo"
    fi
    PRIVCTX=${PRIV_DIR:-}; [ -n "$PRIVCTX" ] || { PRIVCTX=$(mktemp -d); trap 'rmdir "$PRIVCTX" 2>/dev/null; cleanup_build' EXIT; }
    VCTX="$VCTX --build-context priv=$PRIVCTX"
    docker image inspect "$TAG-app" >/dev/null 2>&1 && VCTX="$VCTX --build-context app=docker-image://$TAG-app"
    docker build $PLAT $ARCHARGS $VCTX -f "project/variants/$v/Dockerfile" --build-arg BASE=$ROOTFS_IMG --build-arg VARIANT=$v \
        -t $TAG-rootfs-$v . 2>&1 | tee "$OUT/variant-$v-build.log" | tail -40
    [ "${PIPESTATUS[0]}" = 0 ] || { echo "variant $v build failed (log: $OUT/variant-$v-build.log)"; exit 1; }
    ROOTFS_IMG=$TAG-rootfs-$v; VIMGS="$VIMGS $TAG-rootfs-$v"
  done
fi
[ -z "${ISO_VNAME:-}" ] || VNAME=$ISO_VNAME
[ -z "${STOP_AFTER_VARIANTS:-}" ] || { lap "STOP_AFTER_VARIANTS: image $ROOTFS_IMG kept"; KEEP_IMAGES=1; exit 0; }
if [ -n "${HW_PROFILE:-}" ]; then
lap "stage 2b: hardware profile layer ($HW_PROFILE, gentoo/$HW_PROFILE)"
[ -d "$REPO/gentoo/$HW_PROFILE" ] || { echo "no such hardware profile: gentoo/$HW_PROFILE"; exit 2; }
rm -rf "$HERE/.hw"; cp -a "$REPO/gentoo/$HW_PROFILE" "$HERE/.hw"
docker build $PLAT $ARCHARGS -f common/Dockerfile.profile --build-arg BASE=$ROOTFS_IMG --build-arg HW_PROFILE=$HW_PROFILE -t $TAG-hw . 2>&1 | tee "$OUT/profile-build.log" | tail -30
rm -rf "$HERE/.hw"
docker image inspect $TAG-hw >/dev/null 2>&1 || { echo "profile layer failed (see $OUT/profile-build.log)"; exit 1; }
ROOTFS_IMG=$TAG-hw
fi
VNAME_PRIV=""
if [ -n "${PRIV_DIR:-}" ]; then
lap "stage 2c: PRIVATE layer ($PRIV_DIR)"
[ -d "$PRIV_DIR" ] || { echo "PRIV_DIR is not a directory"; exit 2; }
# every per-machine image carries the OWNER'S name for the machine (his machines are Harry Potter places; ctg-infrastructure hosts/<name>/HOSTNAME),
# shown prefilled by the installer for confirmation. A generic default ('surface-pro') must never be possible again.
[ -s "$PRIV_DIR/hostname" ] || { echo "PRIV_DIR/hostname is missing: put the machine's name there (copy ctg-infrastructure hosts/<name>/HOSTNAME)"; exit 2; }
case "$(tr -d ' \t\r\n' < "$PRIV_DIR/hostname")" in surface-pro|surface|localhost|ubuntu|debian|gentoo|"") echo "PRIV_DIR/hostname is a generic name, not the owner's name for this machine"; exit 2 ;; esac
docker build $PLAT $ARCHARGS --build-context priv="$PRIV_DIR" -f common/Dockerfile.private --build-arg BASE=$ROOTFS_IMG -t $TAG-priv . 2>&1 | tail -12
docker image inspect $TAG-priv >/dev/null 2>&1 || { echo "private layer failed"; exit 1; }
ROOTFS_IMG=$TAG-priv; VNAME_PRIV="private-"
fi
lap "stage 3: assembly toolbox"
# Per-build tag: a shared live-iso-tools:latest could be untagged by the runner disk guard (or replaced by a
# parallel build) between this build and the docker run below (Horcrux run 7066: "pull access denied").
docker build $PLAT $ARCHARGS -q -f common/Dockerfile.tools -t $TAG-tools .
lap "stage 4: export rootfs + assemble ISO (no bind mounts: works from a job container using the host docker)"
ISO_NAME=$ISO_PREFIX${HW_PROFILE:+-$HW_PROFILE}-${ISO_VNAME:-${IMG_HW}$IMG-}$SUFFIX-$ARCH.iso   # owner rule: horcrux-surface-xfce-openrc-<date>-x86_64.iso (ISO_VNAME=surface-xfce-openrc-)
docker rm -f $TAG-asm >/dev/null 2>&1 || true
cid=$(docker create $ROOTFS_IMG /bin/true)
docker export "$cid" | docker run -i $PLAT -e ARCH=$ARCH -e "SQUASH_COMP=${SQUASH_COMP:-}" --name $TAG-asm --memory=8g --cpus=$JOBS $TAG-tools \
    sh -c "mkdir -p /work/root /out && tar -x -C /work/root --anchored --exclude='dev/*' && assemble-iso.sh /work/root /out/$ISO_NAME '$ISO_LABEL' '$PROJECT_TITLE' '$PROJECT'"
docker rm -v "$cid" >/dev/null; cid=
docker cp $TAG-asm:/out/$ISO_NAME "$OUT/$ISO_NAME"
( cd "$OUT" && sha256sum "$ISO_NAME" > "$ISO_NAME.sha256" && cat "$ISO_NAME.sha256" )
lap "done: $OUT/$ISO_NAME ($(du -h "$OUT/$ISO_NAME" | cut -f1))"
