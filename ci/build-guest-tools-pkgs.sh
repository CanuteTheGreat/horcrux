#!/usr/bin/env bash
# Build dist/linux/x86_64/horcrux-guest-tools.{deb,rpm,apk} (nfpm) from the
# static musl agent. Env: OUT_DIR (default dist), LINUX_AGENT, NFPM_VERSION,
# NFPM_SHA256 (of the linux x86_64 tarball), FFMPEG_DEP (default ffmpeg).
set -euo pipefail
cd "$(dirname "$0")/.."
OUT_DIR="${OUT_DIR:-dist}"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
LINUX_AGENT="${LINUX_AGENT:-$TARGET_DIR/x86_64-unknown-linux-musl/release/horcrux-guest-agent}"
NFPM_VERSION="${NFPM_VERSION:-2.49.0}"
NFPM_SHA256="${NFPM_SHA256:-2afaa5ffb5e3493402e31e7c67a5a1fb42133e3f66fc315ca0051934125561c7}"
[ -f "$LINUX_AGENT" ] || { echo "missing $LINUX_AGENT (cargo build -p horcrux-guest-tools --release --target x86_64-unknown-linux-musl)" >&2; exit 1; }
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml | head -1)"
[ -n "$VERSION" ] || { echo "no workspace version in Cargo.toml" >&2; exit 1; }
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
tarball="nfpm_${NFPM_VERSION}_Linux_x86_64.tar.gz"
if command -v nfpm >/dev/null 2>&1 && [ "$(nfpm --version 2>/dev/null | grep -o '[0-9][0-9.]*' | head -1)" = "$NFPM_VERSION" ]; then
  NFPM=nfpm
else
  curl -fsSL --retry 3 -o "$WORK/$tarball" "https://github.com/goreleaser/nfpm/releases/download/v${NFPM_VERSION}/${tarball}"
  echo "$NFPM_SHA256  $WORK/$tarball" | sha256sum -c --quiet
  tar -xzf "$WORK/$tarball" -C "$WORK" nfpm
  NFPM="$WORK/nfpm"
fi
cp "$LINUX_AGENT" "$WORK/horcrux-guest-agent"
strip "$WORK/horcrux-guest-agent" 2>/dev/null || true
dest="$OUT_DIR/linux/x86_64"
mkdir -p "$dest"
export HX_VERSION="$VERSION" HX_AGENT="$WORK/horcrux-guest-agent"
# nfpm does not expand env vars in contents paths: the recipe is rendered per format.
for fmt in deb rpm apk; do
  dep="${FFMPEG_DEP:-ffmpeg}"
  [ "$fmt" = rpm ] && dep="${FFMPEG_DEP:-/usr/bin/ffmpeg}"
  sed -e "s|\${HX_VERSION}|$HX_VERSION|g" -e "s|\${HX_AGENT}|$HX_AGENT|g" \
      -e "s|\${HX_FFMPEG_DEP}|$dep|g" -e "s|packaging/linux/|$PWD/packaging/linux/|" \
      packaging/linux/nfpm.yaml > "$WORK/nfpm.yaml"
  "$NFPM" package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$dest/horcrux-guest-tools.$fmt"
done
ls -l "$dest"
