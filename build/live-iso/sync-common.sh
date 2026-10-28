#!/bin/bash
# One source of truth for the shared live-installer: Horcrux's build/live-iso/common is the master,
# Patronus's gentoo/live-iso/common is a vendored copy. This script is identical in both repos.
#   sync-common.sh --write-hash            (master) refresh common/TREE.sha256 after editing common/
#   sync-common.sh --from <horcrux-repo>   (vendor) copy the master's common/ here and verify
#   sync-common.sh --check                 verify common/ matches its recorded TREE.sha256 (catches hand edits)
#   sync-common.sh --upstream              (vendor) also compare with Horcrux main over the Forgejo API (skips if unreachable)
# CI runs --check in both repos (and --upstream in Patronus); a mismatch fails the unit-test gate, never mails.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); C=$HERE/common
tree_hash() { ( cd "$1" && find . -type f ! -name TREE.sha256 ! -path '*/__pycache__/*' ! -name '*.pyc' -printf '%m %p\n' | LC_ALL=C sort | while read -r m f; do echo "$m $f $(sha256sum "$f" | cut -d' ' -f1)"; done | sha256sum | cut -d' ' -f1 ); }
case "${1:-}" in
  --write-hash) tree_hash "$C" > "$C/TREE.sha256"; echo "common/TREE.sha256 = $(cat "$C/TREE.sha256")" ;;
  --from) src=${2:?path to the Horcrux checkout}/build/live-iso/common
          [ -f "$src/TREE.sha256" ] || { echo "no TREE.sha256 in $src (run --write-hash in Horcrux first)"; exit 1; }
          [ "$(tree_hash "$src")" = "$(cat "$src/TREE.sha256")" ] || { echo "the master's common/ does not match its TREE.sha256"; exit 1; }
          mkdir -p "$C"; rsync -a --delete --exclude='__pycache__' "$src/" "$C/"; "$HERE/sync-common.sh" --check ;;
  --check) want=$(cat "$C/TREE.sha256" 2>/dev/null || echo missing); got=$(tree_hash "$C")
           if [ "$want" = "$got" ]; then echo "live-installer common/ matches TREE.sha256 ($got)"
           else echo "::error::common/ differs from its recorded TREE.sha256 (hand edit?). Edit the master in Horcrux build/live-iso/common, run --write-hash there and --from here."; exit 1; fi ;;
  --upstream) "$HERE/sync-common.sh" --check
              url="${GITHUB_SERVER_URL:-https://git.canutethegreat.com}/api/v1/repos/CanuteTheGreat/horcrux/raw/build/live-iso/common/TREE.sha256?ref=main"
              up=$(curl -fsS -H "Authorization: token ${FORGEJO_TOKEN:-${UPSTREAM_TOKEN:-}}" "$url" 2>/dev/null | tr -d '[:space:]' || true)
              [ -n "$up" ] || { echo "upstream TREE.sha256 not reachable from this job; skipped"; exit 0; }
              [ "$up" = "$(cat "$C/TREE.sha256")" ] && echo "matches Horcrux main ($up)" || { echo "::error::common/ is out of date versus Horcrux main ($up): run sync-common.sh --from <horcrux checkout>"; exit 1; } ;;
  *) sed -n 2,8p "$0"; exit 2 ;;
esac
