#!/usr/bin/env bash
# Build the Horcrux Console (desktop client) macOS installer: a universal
# (arm64 + x86_64) "Horcrux Console.app" in a signed-or-unsigned .pkg.
# Runs ON macOS (needs Xcode command line tools: the Apple SDK frameworks
# VideoToolbox, CoreMedia, CoreVideo, GameController and AppKit are linked,
# and pkgbuild/productbuild/lipo are Apple tools). Linux can only
# `cargo check` these targets (see .forgejo/workflows/console-desktop.yml).
# Env: OUT_DIR (default dist), SKIP_BUILD=1 to reuse a build,
#      PKG_SIGN_IDENTITY (optional "Developer ID Installer: ..." identity).
# Output: $OUT_DIR/horcrux-console-<version>-macos.pkg
set -euo pipefail
cd "$(dirname "$0")/.."
[ "$(uname -s)" = Darwin ] || { echo "build-console-pkg.sh runs on macOS only" >&2; exit 1; }
OUT_DIR="${OUT_DIR:-dist}"
VERSION="$(awk '/^\[workspace.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/[" ]/,"",$3);print $3;exit}' Cargo.toml)"
[ -n "$VERSION" ] || { echo "cannot read the workspace version from Cargo.toml" >&2; exit 1; }
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ -z "${SKIP_BUILD:-}" ]; then
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target add "$t" >/dev/null
    MACOSX_DEPLOYMENT_TARGET=11.0 cargo build -p horcrux-desktop --release --target "$t"
  done
fi
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/root/Applications/Horcrux Console.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/horcrux-console" \
  "$TARGET_DIR/aarch64-apple-darwin/release/horcrux-console" \
  "$TARGET_DIR/x86_64-apple-darwin/release/horcrux-console"
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
# the settings example (= the defaults); each user's own settings live in
# ~/Library/Application Support/horcrux/console.toml
cp packaging/linux/horcrux-console.toml "$APP/Contents/Resources/console.toml.example"
# ad-hoc signature so Apple silicon runs the universal binary
codesign --force --sign - "$APP/Contents/MacOS/horcrux-console"
mkdir -p "$OUT_DIR"
pkgbuild --root "$WORK/root" --identifier org.horcrux.console --version "$VERSION" \
  --install-location / "$WORK/component.pkg"
PKG="$(cd "$OUT_DIR" && pwd)/horcrux-console-$VERSION-macos.pkg"
if [ -n "${PKG_SIGN_IDENTITY:-}" ]; then
  productbuild --package "$WORK/component.pkg" --sign "$PKG_SIGN_IDENTITY" "$PKG"
else
  productbuild --package "$WORK/component.pkg" "$PKG"
fi
pkgutil --payload-files "$PKG" | grep -q 'Horcrux Console.app/Contents/MacOS/horcrux-console'
echo "built $PKG (version $VERSION)"
