#!/usr/bin/env bash
# Build the Horcrux Guest Tools Windows MSI on Linux.
#   cross-compiles horcrux-guest-agent for x86_64-pc-windows-gnu (release), strips it,
#   builds the guest drivers (ci/build-guest-drivers.sh: virtual display + gamepad)
#   and packs packaging/windows/horcrux-guest-tools.wxs with wixl (msitools).
# Needs: rust target x86_64-pc-windows-gnu, gcc-mingw-w64-x86-64, msitools,
#        clang + lld (drivers).
# Env: OUT_DIR (default dist), TARGET (default x86_64-pc-windows-gnu), CARGO_BUILD_ARGS,
#      SKIP_BUILD=1 to reuse an existing build in the cargo target dir,
#      DRIVERS_DIR to pack already built drivers (default: build them into $OUT_DIR/drivers).
# Output: $OUT_DIR/horcrux-guest-tools-<version>-x64.msi
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${TARGET:-x86_64-pc-windows-gnu}"
OUT_DIR="${OUT_DIR:-dist}"
VERSION="$(awk '/^\[workspace.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/[" ]/,"",$3);print $3;exit}' Cargo.toml)"
[ -n "$VERSION" ] || { echo "cannot read the workspace version from Cargo.toml" >&2; exit 1; }
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ -z "${SKIP_BUILD:-}" ]; then
  # shellcheck disable=SC2086
  cargo build -p horcrux-guest-tools --release --target "$TARGET" ${CARGO_BUILD_ARGS:-}
fi
EXE="$TARGET_DIR/$TARGET/release/horcrux-guest-agent.exe"
[ -f "$EXE" ] || { echo "missing $EXE" >&2; exit 1; }
mkdir -p "$OUT_DIR"
if [ -z "${DRIVERS_DIR:-}" ]; then
  OUT_DIR="$OUT_DIR/drivers" bash ci/build-guest-drivers.sh
  DRIVERS_DIR="$OUT_DIR/drivers"
fi
DRIVERS_DIR="$(cd "$DRIVERS_DIR" && pwd)"
for f in hxidd/hxidd.dll hxidd/hxidd.inf hxpad/hxpad.dll hxpad/hxpad.inf; do
  [ -f "$DRIVERS_DIR/$f" ] || { echo "missing $DRIVERS_DIR/$f" >&2; exit 1; }
done
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cp "$EXE" "$WORK/horcrux-guest-agent.exe"
mkdir -p "$WORK/drivers"
cp -r "$DRIVERS_DIR/hxidd" "$DRIVERS_DIR/hxpad" "$WORK/drivers/"
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
# Horcrux virtual display: auto (when the host asks: the VM's GPU has no
# monitor or no outputs), on, off
#virtual_display = auto
#virtual_display_modes = 3840x2160@60,3840x2160@120,2560x1440@60,2560x1440@144,2560x1440@240,1920x1080@60,1920x1080@144,1920x1080@240,1680x1050@60,1600x900@60,1366x768@60,1280x720@60
#virtual_display_default_hz = 60
#virtual_display_max_monitors = 4
#virtual_display_persist = true
#virtual_display_primary = true
#virtual_display_wait_ms = 5000
# capture = idd reads the virtual display's frames straight from its driver
#gamepads = 1
#driver_cert_subject = Horcrux Guest Drivers
#driver_device_timeout_ms = 30000
CONF
MSI="$OUT_DIR/horcrux-guest-tools-$VERSION-x64.msi"
ROOT="$PWD"
mkdir -p "$OUT_DIR"; MSI="$(cd "$OUT_DIR" && pwd)/horcrux-guest-tools-$VERSION-x64.msi"
# wixl resolves File Source paths relative to the working directory: run it in $WORK.
(cd "$WORK" && wixl -v -a x64 -D "Version=$VERSION" -D "AgentExe=horcrux-guest-agent.exe" \
     -D "DefaultConf=guest-tools.conf" -D "Drivers=drivers" -o "$MSI" "$ROOT/packaging/windows/horcrux-guest-tools.wxs")
echo "built $MSI ($(stat -c %s "$MSI") bytes, version $VERSION)"
