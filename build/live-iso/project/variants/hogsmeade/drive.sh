#!/bin/bash
# HOGSMEADE private image build driver. Runs on the build host inside an agent-workdir lease:
#   W/repo = git archive of the branch, W/overlay = canutethegreat-overlay checkout, W/priv = private payload (agent_authorized_keys, hostname).
#   START_AT=base|part1|part2|iso (default base). A later start reuses the images of the earlier stages (hsm-rootfs-gnome-light, hsm-kinstall,
#   hsmfin-rootfs): use it when a change touches only the later layers. NOTE every commit changes the cargo app context, so START_AT=base
#   re-runs the app build and with it every layer; part2 re-emerges horcrux-tools/hogsmeade-tools/comfyui/private from the CURRENT tree.
set -euo pipefail
W=$PWD; R=$W/repo; cd $R/build/live-iso; OUT=$W/out; mkdir -p $OUT; export JOBS=6 DOCKER_BUILDKIT=1
D="sudo -E docker"
ctx="--build-context hxgentoo=$R/gentoo --build-context hxgentoo2=$R/gentoo --build-context ctgoverlay=$W/overlay --build-context hsmprivate=$W/priv"
HSM=$R/build/live-iso/project/variants/hogsmeade
START_AT=${START_AT:-base}
ord() { case "$1" in base) echo 1;; part1) echo 2;; part2) echo 3;; iso) echo 4;; *) echo "unknown START_AT=$1" >&2; exit 2;; esac; }
want() { [ "$(ord "$START_AT")" -le "$(ord "$1")" ]; }
echo "== $(date -Is) preflight (static paths+atoms; ::gentoo/::guru lists from a throwaway container when the base image exists) START_AT=$START_AT"
if $D image inspect hsm-rootfs-gnome-light >/dev/null 2>&1; then $HSM/preflight-atoms.sh hsm-rootfs-gnome-light $W/atoms >/dev/null || echo "WARNING: atom lists not refreshed"; fi
python3 $HSM/preflight.py --ctx $R/build/live-iso --work $W --atoms $W/atoms || { echo PREFLIGHT-FAILED; exit 1; }
test -s $W/priv/hostname && test -s $W/priv/agent_authorized_keys || { echo "PRIVATE PAYLOAD INCOMPLETE (hostname, agent_authorized_keys)"; exit 1; }
BASE=hsm-rootfs-gnome-light
if want base; then
  # KEEP_DEV=1: the machine image compiles kernel modules, the NVIDIA userland and apps on top of the desktop layer: headers and static libs stay
  echo "== $(date -Is) stage1+2+core+gnome-light (KEEP_DEV=1)"; touch $W/STAGE-BASE
  sudo -E env KEEP_DEV=1 VARIANT=gnome-light STOP_AFTER_VARIANTS=1 BUILD_TAG=hsm ./build.sh -o $OUT > $W/base.log 2>&1 || { tail -40 $W/base.log; exit 1; }
fi
$D image inspect $BASE >/dev/null
if want part1; then
  echo "== $(date -Is) part1 kernel/zfs/nvidia"
  $D build $ctx -f project/variants/hogsmeade/Dockerfile --target hsm-kinstall --build-arg BASE=$BASE -t hsm-kinstall . > $W/part1.log 2>&1 || { tail -60 $W/part1.log; exit 1; }
fi
$D image inspect hsm-kinstall >/dev/null
if want part2; then
  echo "== $(date -Is) part2"
  $D build $ctx -f project/variants/hogsmeade/Dockerfile.part2 --build-arg BASE=hsm-kinstall -t hsmfin-rootfs . > $W/part2.log 2>&1 || { tail -60 $W/part2.log; exit 1; }
fi
$D image inspect hsmfin-rootfs >/dev/null
echo "== $(date -Is) ISO"
rm -f $OUT/*.iso $OUT/*.iso.sha256
sudo -E env REUSE_ROOTFS=hsmfin-rootfs PRIV_DIR=$W/priv ISO_VNAME=gnome-light-openrc-hogsmeade-private- ./build.sh -o $OUT -n ${ISO_DATE:-2026.11.10} > $W/iso.log 2>&1 || { tail -40 $W/iso.log; exit 1; }
ls -la $OUT; echo "== $(date -Is) BUILD-DONE"
