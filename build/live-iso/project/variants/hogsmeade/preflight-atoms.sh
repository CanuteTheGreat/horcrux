#!/bin/bash
# Dump the category/package lists of ::gentoo (fresh webrsync snapshot) and ::guru from a throwaway container on the cached base image,
# exactly the way the hogsmeade part1 RUN step syncs them. Usage: preflight-atoms.sh <base image> <out dir>
set -euo pipefail
BASE=$1; OUT=$2; mkdir -p "$OUT"
if [ -s "$OUT/gentoo.txt" ] && [ -s "$OUT/guru.txt" ] && [ -n "$(find "$OUT/gentoo.txt" -mmin -720)" ]; then echo "atom lists fresh"; exit 0; fi
sudo docker run --rm -v "$OUT":/out "$BASE" sh -c '
  set -e; unset ARCH
  emerge-webrsync -q >/dev/null 2>&1 || emerge --sync -q >/dev/null 2>&1
  emerge --noreplace --usepkg --getbinpkg app-eselect/eselect-repository dev-vcs/git >/dev/null 2>&1
  eselect repository enable guru >/dev/null 2>&1; emerge --sync guru >/dev/null 2>&1
  for r in gentoo guru; do (cd /var/db/repos/$r && ls -d */*/ | tr -d / | sed "s#\(.*\)#\1#" >/dev/null; for d in */*/; do echo "${d%/}"; done) > /out/$r.txt; done
  wc -l /out/gentoo.txt /out/guru.txt'
