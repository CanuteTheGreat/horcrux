#!/usr/bin/env bash
# Build the Horcrux Guest Tools Windows MSI on Linux.
#   cross-compiles horcrux-guest-agent for x86_64-pc-windows-gnu (release), strips it,
#   and packs packaging/windows/horcrux-guest-tools.wxs with wixl (msitools).
# Needs: rust target x86_64-pc-windows-gnu, gcc-mingw-w64-x86-64, cmake (builds the bundled
#        libopus of the audio encoder, linked statically), msitools.
# Env: OUT_DIR (default dist), TARGET (default x86_64-pc-windows-gnu), CARGO_BUILD_ARGS,
#      SKIP_BUILD=1 to reuse an existing build in the cargo target dir.
# Output: $OUT_DIR/horcrux-guest-tools-<version>-x64.msi
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${TARGET:-x86_64-pc-windows-gnu}"
OUT_DIR="${OUT_DIR:-dist}"
VERSION="$(awk '/^\[workspace.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/[" ]/,"",$3);print $3;exit}' Cargo.toml)"
[ -n "$VERSION" ] || { echo "cannot read the workspace version from Cargo.toml" >&2; exit 1; }
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ -z "${SKIP_BUILD:-}" ]; then
  command -v cmake >/dev/null || { echo "cmake is needed to build the bundled libopus" >&2; exit 1; }
  # shellcheck disable=SC2086
  cargo build -p horcrux-guest-tools --release --target "$TARGET" ${CARGO_BUILD_ARGS:-}
fi
EXE="$TARGET_DIR/$TARGET/release/horcrux-guest-agent.exe"
[ -f "$EXE" ] || { echo "missing $EXE" >&2; exit 1; }
mkdir -p "$OUT_DIR"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cp "$EXE" "$WORK/horcrux-guest-agent.exe"
"${STRIP:-x86_64-w64-mingw32-strip}" "$WORK/horcrux-guest-agent.exe"
cat > "$WORK/guest-tools.conf" <<'CONF'
# Horcrux guest tools settings (key = value). Every key has a built-in default;
# uncomment to override. See docs/design/horcrux-console.md and `horcrux-guest-agent --help`.
#port = \\.\Global\org.horcrux.console.0
#h264_profile = main
#capture = auto
#encoder = auto
#clipboard_poll_ms = 250
#service_login_screen = true
#audio_source = default
#audio_opus_application = lowdelay
#audio_bitrate_kbps = 128
CONF
MSI="$OUT_DIR/horcrux-guest-tools-$VERSION-x64.msi"
ROOT="$PWD"
mkdir -p "$OUT_DIR"; MSI="$(cd "$OUT_DIR" && pwd)/horcrux-guest-tools-$VERSION-x64.msi"
# wixl resolves File Source paths relative to the working directory: run it in $WORK.
(cd "$WORK" && wixl -v -a x64 -D "Version=$VERSION" -D "AgentExe=horcrux-guest-agent.exe" \
     -D "DefaultConf=guest-tools.conf" -o "$MSI" "$ROOT/packaging/windows/horcrux-guest-tools.wxs")
echo "built $MSI ($(stat -c %s "$MSI") bytes, version $VERSION)"
