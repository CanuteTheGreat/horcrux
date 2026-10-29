#!/bin/sh
# fetch-patches.sh <outdir>: clone the pinned t2linux patch set and keep only the *.patch we apply (+ extra_config)
set -eu
HERE=$(cd "$(dirname "$0")/.." && pwd); . "$HERE/PATCHSET"
out=$1; tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git clone -q "$T2_PATCHES_REPO" "$tmp/p"
git -C "$tmp/p" checkout -q "$T2_PATCHES_COMMIT"
rm -rf "$out"; mkdir -p "$out"
for p in "$tmp"/p/*.patch; do
  b=$(basename "$p"); skip=0
  for s in $SKIP_PATCHES; do case $b in $s) skip=1;; esac; done
  [ $skip = 1 ] || cp "$p" "$out/$b"
done
cp "$tmp/p/extra_config" "$out/extra_config"
echo "patches: $(ls "$out"/*.patch | wc -l) from $T2_PATCHES_COMMIT" >&2
