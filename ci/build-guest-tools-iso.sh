#!/usr/bin/env bash
# Build the Horcrux guest tools CD image (what Horcrux attaches to a VM so
# the tools can be installed from inside it):
#   windows/horcrux-guest-tools-<version>-x64.msi   (ci/build-guest-tools-msi.sh)
#   windows/horcrux-guest-tools-<version>-arm64.msi (same, TARGET=aarch64-pc-windows-gnullvm; when built)
#   linux/horcrux-guest-agent                       (static x86_64 musl build)
#   linux/horcrux-guest-agent.initd + .service + guest-tools.conf
#   linux/x86_64/horcrux-guest-tools.{deb,rpm,apk}  (ci/build-guest-tools-pkgs.sh)
# Env: OUT_DIR (default dist), MSI (path, default the newest dist/*.msi),
#      PKG_DIR (default $OUT_DIR/linux/x86_64, built when missing),
#      LINUX_AGENT (path, default the musl release build), LABEL (HXTOOLS).
# Needs xorriso.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT_DIR="${OUT_DIR:-dist}"
LABEL="${LABEL:-HXTOOLS}"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
LINUX_AGENT="${LINUX_AGENT:-$TARGET_DIR/x86_64-unknown-linux-musl/release/horcrux-guest-agent}"
MSI="${MSI:-$(ls -t "$OUT_DIR"/horcrux-guest-tools-*-x64.msi 2>/dev/null | head -1)}"
[ -f "$LINUX_AGENT" ] || { echo "missing $LINUX_AGENT (cargo build -p horcrux-guest-tools --release --target x86_64-unknown-linux-musl)" >&2; exit 1; }
[ -f "$MSI" ] || { echo "missing the MSI (ci/build-guest-tools-msi.sh)" >&2; exit 1; }
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/windows" "$WORK/linux"
cp "$MSI" "$WORK/windows/"
for a in "$OUT_DIR"/horcrux-guest-tools-*-arm64.msi; do [ -f "$a" ] && cp "$a" "$WORK/windows/"; done
cp "$LINUX_AGENT" "$WORK/linux/horcrux-guest-agent"
strip "$WORK/linux/horcrux-guest-agent" 2>/dev/null || true
cp packaging/linux/horcrux-guest-agent.initd packaging/linux/horcrux-guest-agent.service \
   packaging/linux/guest-tools.conf "$WORK/linux/"
PKG_DIR="${PKG_DIR:-$OUT_DIR/linux/x86_64}"
for f in deb rpm apk; do
  [ -f "$PKG_DIR/horcrux-guest-tools.$f" ] || { OUT_DIR="$OUT_DIR" LINUX_AGENT="$LINUX_AGENT" bash ci/build-guest-tools-pkgs.sh; break; }
done
mkdir -p "$WORK/linux/x86_64"
for f in deb rpm apk; do cp "$PKG_DIR/horcrux-guest-tools.$f" "$WORK/linux/x86_64/"; done
mkdir -p "$OUT_DIR"
ISO="$OUT_DIR/horcrux-guest-tools.iso"
xorriso -as mkisofs -quiet -V "$LABEL" -J -R -o "$ISO" "$WORK"
( cd "$OUT_DIR" && sha256sum "$(basename "$ISO")" > "$(basename "$ISO").sha256" )
echo "built $ISO ($(stat -c %s "$ISO") bytes)"
