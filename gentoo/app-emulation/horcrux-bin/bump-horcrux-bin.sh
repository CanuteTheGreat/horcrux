#!/bin/bash
# Cut a new, strictly newer horcrux-bin version for a new manager tarball (see the STALE BINPKGS note in horcrux-bin-0.1.0.ebuild).
#   bump-horcrux-bin.sh [--dir DIR] [--stamp YYYYMMDDHHMM] [--sha256-x86_64 H] [--sha256-aarch64 H] [--keep N]
# Copies horcrux-bin-0.1.0.ebuild to horcrux-bin-0.1.0_p<stamp>.ebuild (stamp = UTC build time of the tarball, default now), fills in the
# tarball checksums, prunes all but the newest N (default 3) stamped ebuilds. Idempotent: when the newest stamped ebuild already carries
# these checksums nothing changes (exit 0, prints "unchanged <file>"). A stamp not newer than the newest existing one is refused (exit 2):
# Portage versions only go up, and an older stamp would be invisible to `emerge -u`.
set -eu
dir="$(cd "$(dirname "$0")" && pwd)"; stamp=""; sx=""; sa=""; keep=3
while [ $# -gt 0 ]; do
  case "$1" in
    --dir) dir=$2; shift 2;;
    --stamp) stamp=$2; shift 2;;
    --sha256-x86_64) sx=$2; shift 2;;
    --sha256-aarch64) sa=$2; shift 2;;
    --keep) keep=$2; shift 2;;
    *) echo "bad argument: $1" >&2; exit 64;;
  esac
done
[ -n "$stamp" ] || stamp=$(date -u +%Y%m%d%H%M)
case "$stamp" in [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]) ;; *) echo "stamp must be YYYYMMDDHHMM: $stamp" >&2; exit 64;; esac
for h in "$sx" "$sa"; do [ -z "$h" ] || [ "${#h}" = 64 ] || { echo "sha256 must be 64 hex digits" >&2; exit 64; }; done
base="$dir/horcrux-bin-0.1.0.ebuild"
[ -s "$base" ] || { echo "no $base" >&2; exit 66; }
newest=$(cd "$dir" && ls horcrux-bin-0.1.0_p[0-9]*.ebuild 2>/dev/null | sed -E 's/.*_p([0-9]+)(-r[0-9]+)?\.ebuild/\1/' | sort -n | tail -1 || true)
if [ -n "$newest" ]; then
  cur=$(cd "$dir" && ls horcrux-bin-0.1.0_p"$newest"*.ebuild | sort -V | tail -1); cur="$dir/$cur"
  if grep -q "^HORCRUX_MANAGER_SHA256_x86_64=\"$sx\"" "$cur" && grep -q "^HORCRUX_MANAGER_SHA256_aarch64=\"$sa\"" "$cur" && [ -n "$sx$sa" ]; then
    echo "unchanged $cur"; exit 0
  fi
  [ "$stamp" -gt "$newest" ] || { echo "stamp $stamp is not newer than the existing $newest" >&2; exit 2; }
fi
out="$dir/horcrux-bin-0.1.0_p$stamp.ebuild"
sed -e "s/^HORCRUX_MANAGER_SHA256_x86_64=\"\"/HORCRUX_MANAGER_SHA256_x86_64=\"$sx\"/" \
    -e "s/^HORCRUX_MANAGER_SHA256_aarch64=\"\"/HORCRUX_MANAGER_SHA256_aarch64=\"$sa\"/" "$base" > "$out"
echo "created $out"
(cd "$dir" && ls horcrux-bin-0.1.0_p[0-9]*.ebuild | sort | head -n -"$keep" | xargs -r rm -v)
