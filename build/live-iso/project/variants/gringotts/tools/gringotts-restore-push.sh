#!/bin/sh
# gringotts-restore-push.sh - run on CRYSTALCAVE (never on gringotts): restic-restore the pre-wipe snapshot of gringotts from B2,
# push the archives to the new install over SSH/Tailscale and run `gringotts-restore` there. No B2 credentials and no restic password
# ever reach gringotts; wg0.conf goes over the ssh stdin (never printed).
#
#   gringotts-restore-push.sh TARGET [--tag pre-horcrux-wipe] [--nft] [--no-wg]
#     TARGET      ssh destination of the new install, e.g. root@gringotts (Tailscale name) or root@<lan-ip>
# Environment (tests): RESTIC=<cmd>  (default: ~/.hermes/scripts/restic-workstations.sh gringotts)
#                      SRC_DIR=<dir> skip restic, use an already restored directory (the QEMU harness does the restic half on crystalcave)
#                      SSH_OPTS="-p 2222 -i key"  WG_FILE=<file>
set -eu
TARGET=${1:?usage: gringotts-restore-push.sh TARGET [--tag T] [--nft] [--no-wg]}; shift
TAG=pre-horcrux-wipe; NFT=""; WG=1
while [ $# -gt 0 ]; do case $1 in --tag) TAG=$2; shift;; --nft) NFT=--nft;; --no-wg) WG=0;; *) echo "unknown $1"; exit 2;; esac; shift; done
RESTIC=${RESTIC:-$HOME/.hermes/scripts/restic-workstations.sh gringotts}
WG_FILE=${WG_FILE:-$HOME/.hermes/secrets/gringotts/wg0.conf}
SSH="ssh ${SSH_OPTS:-} -o StrictHostKeyChecking=accept-new"
SCRATCH=""
cleanup() { [ -z "$SCRATCH" ] || rm -rf "$SCRATCH"; }
trap cleanup EXIT
if [ -n "${SRC_DIR:-}" ]; then
  DIR=$SRC_DIR
else
  SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/gringotts-restic-XXXXXX"); chmod 700 "$SCRATCH"
  echo "== restic restore latest (tag $TAG) -> scratch"
  $RESTIC restore latest --tag "$TAG" --target "$SCRATCH" >/dev/null
  DIR=$SCRATCH
fi
find_one() { find "$DIR" -name "$1" -type f | head -1; }
HOME_T=$(find_one home-dotfiles.tar.zst); FF_T=$(find_one firefox-snap-profile.tar.zst); ETC_T=$(find_one etc-selected.tar.zst)
[ -n "$HOME_T" ] || { echo "home-dotfiles.tar.zst not found in the restored snapshot ($DIR)"; exit 1; }
for f in "$HOME_T" $FF_T $ETC_T; do zstd -t -q "$f" || { echo "corrupt archive: $f"; exit 1; }; done
echo "== archives verified: $(basename "$HOME_T") ${FF_T:+$(basename "$FF_T")} ${ETC_T:+$(basename "$ETC_T")}"
REMOTE=/var/tmp/gringotts-backup
$SSH "$TARGET" "rm -rf $REMOTE; mkdir -p $REMOTE; chmod 700 $REMOTE"
for f in "$HOME_T" $FF_T $ETC_T; do $SSH "$TARGET" "cat > $REMOTE/$(basename "$f")" < "$f"; done
rc=0
echo "== pushed; running gringotts-restore on the target"
if [ "$WG" = 1 ] && [ -s "$WG_FILE" ]; then
  $SSH "$TARGET" "gringotts-restore --backup $REMOTE --wg-stdin $NFT" < "$WG_FILE" || rc=$?
else
  $SSH "$TARGET" "gringotts-restore --backup $REMOTE $NFT" || rc=$?
fi

$SSH "$TARGET" "find $REMOTE -type f -exec shred -u {} + ; rm -rf $REMOTE" || true
echo "== target copy of the archives removed; scratch removed on exit"
exit $rc
