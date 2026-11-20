#!/usr/bin/env bash
# Build the Horcrux Console (desktop client) Windows MSI on Linux.
#   cross-compiles horcrux-desktop (binary horcrux-console.exe) for
#   x86_64-pc-windows-gnu (release), strips it, and packs
#   packaging/windows/horcrux-console.wxs with wixl (msitools).
# Needs: rust target x86_64-pc-windows-gnu, gcc-mingw-w64-x86-64, msitools.
# Env: OUT_DIR (default dist), TARGET (default x86_64-pc-windows-gnu), CARGO_BUILD_ARGS,
#      SKIP_BUILD=1 to reuse an existing build in the cargo target dir.
# Output: $OUT_DIR/horcrux-console-<version>-x64.msi
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${TARGET:-x86_64-pc-windows-gnu}"
OUT_DIR="${OUT_DIR:-dist}"
VERSION="$(awk '/^\[workspace.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/["  ]/,"",$3);print $3;exit}' Cargo.toml)"
[ -n "$VERSION" ] || { echo "cannot read the workspace version from Cargo.toml" >&2; exit 1; }
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ -z "${SKIP_BUILD:-}" ]; then
  # shellcheck disable=SC2086
  cargo build -p horcrux-desktop --release --target "$TARGET" ${CARGO_BUILD_ARGS:-}
fi
EXE="$TARGET_DIR/$TARGET/release/horcrux-console.exe"
[ -f "$EXE" ] || { echo "missing $EXE" >&2; exit 1; }
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cp "$EXE" "$WORK/horcrux-console.exe"
"${STRIP:-x86_64-w64-mingw32-strip}" "$WORK/horcrux-console.exe"
# the settings example is the defaults (horcrux-desktop's
# packaged_example_is_the_defaults test keeps it so)
cp packaging/linux/horcrux-console.toml "$WORK/console.toml.example"
mkdir -p "$OUT_DIR"
MSI="$(cd "$OUT_DIR" && pwd)/horcrux-console-$VERSION-x64.msi"
ROOT="$PWD"
# wixl resolves File Source paths relative to the working directory: run it in $WORK.
(cd "$WORK" && wixl -v -a x64 -D "Version=$VERSION" -D "ConsoleExe=horcrux-console.exe" \
     -D "ExampleConf=console.toml.example" -o "$MSI" "$ROOT/packaging/windows/horcrux-console.wxs")
echo "built $MSI ($(stat -c %s "$MSI") bytes, version $VERSION)"
