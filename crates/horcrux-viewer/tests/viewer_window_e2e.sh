#!/bin/bash
# Horcrux Console native viewer WINDOW on a real Wayland compositor: a real
# horcrux-api + QEMU guest (Alpine) with the guest tools and the frame ring,
# a headless sway (pixman renderer, no GPU) showing the real horcrux-viewer
# window, a compositor screenshot (grim) compared with the guest's own
# screen, and keys typed into the compositor (virtual-keyboard protocol)
# that must reach the guest through the viewer. See viewer_window_e2e.py.
# Single standalone node; KVM when /dev/kvm is usable. Runs as root.
#
# Usage: viewer_window_e2e.sh <horcrux-api> <horcrux-viewer> <horcrux-guest-agent (x86_64 musl)>
# Env:   HXC_CACHE (ISO cache, default ~/.cache/hxc), HXW_OUT (results and
#        screenshots, default /tmp/hxw-e2e); the HXW_* settings of
#        viewer_window_e2e.py.
set -euo pipefail
BIN=$(readlink -f "${1:?usage: $0 <horcrux-api> <horcrux-viewer> <horcrux-guest-agent>}")
VIEWER=$(readlink -f "${2:?usage: $0 <horcrux-api> <horcrux-viewer> <horcrux-guest-agent>}")
AGENT=$(readlink -f "${3:?usage: $0 <horcrux-api> <horcrux-viewer> <horcrux-guest-agent>}")
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CACHE=${HXC_CACHE:-$HOME/.cache/hxc}
OUT=${HXW_OUT:-/tmp/hxw-e2e}
DATA=$(mktemp -d /tmp/hxw-data.XXXXXX)
RUNTIME=$(mktemp -d /tmp/hxw-run.XXXXXX)
chmod 700 "$RUNTIME"
SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
mkdir -p "$CACHE" "$OUT"

need=()
for t in qemu-system-x86_64:qemu-system-x86 qemu-img:qemu-utils curl:curl ffmpeg:ffmpeg xorriso:xorriso \
         sway:sway grim:grim python3:python3; do
  command -v "${t%%:*}" >/dev/null || need+=("${t##*:}")
done
[ -f /usr/share/X11/xkb/rules/evdev ] || need+=(xkb-data)
ldd "$BIN" 2>/dev/null | grep -q 'libvirt.so.* not found' && need+=(libvirt0)
if ! command -v qemu-system-x86_64 >/dev/null || ! qemu-system-x86_64 -display help | grep -qx dbus; then
  need+=(qemu-system-modules-opengl)
fi
if [ ${#need[@]} -gt 0 ]; then
  for i in 1 2 3; do
    $SUDO apt-get update -qq >/dev/null 2>&1 || true
    apt-cache policy sway | grep -q 'Candidate: [0-9]' && break
    sleep 10
  done
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${need[@]}" >/dev/null
fi
qemu-system-x86_64 -display help | grep -qx dbus || { echo "FAIL QEMU has no D-Bus display module"; exit 1; }

AMIRROR=${HXC_ALPINE_MIRROR:-https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/x86_64}
AISO="$CACHE/alpine-virt.iso"
if [ ! -s "$AISO" ]; then
  meta=$(curl -fsSL --retry 3 "$AMIRROR/latest-releases.yaml")
  afile=$(printf '%s\n' "$meta" | awk '/flavor: alpine-virt/{f=1} f&&/file:/{print $2; exit}')
  asum=$(printf '%s\n' "$meta" | awk '/flavor: alpine-virt/{f=1} f&&/sha256:/{print $2; exit}')
  curl -fsSL --retry 3 -o "$AISO.part" "$AMIRROR/$afile"
  echo "$asum  $AISO.part" | sha256sum -c --quiet || { echo "FAIL Alpine ISO checksum"; exit 1; }
  mv "$AISO.part" "$AISO"
fi
mkdir -p "$DATA/images" "$DATA/tools/linux"
cp "$AISO" "$DATA/images/alpine-virt.iso"
# the guest tools CD the node attaches ([console] guest_tools_iso), same
# layout as ci/build-guest-tools-iso.sh (label HXTOOLS, linux/horcrux-guest-agent)
cp "$AGENT" "$DATA/tools/linux/horcrux-guest-agent"
chmod 755 "$DATA/tools/linux/horcrux-guest-agent"
xorriso -as mkisofs -quiet -V HXTOOLS -J -R -o "$DATA/images/tools.iso" "$DATA/tools"

cleanup() {
  [ -f "$DATA/api.pid" ] && kill "$(cat "$DATA/api.pid")" 2>/dev/null || true
  [ -f "$RUNTIME/sway.pid" ] && kill "$(cat "$RUNTIME/sway.pid")" 2>/dev/null || true
  pkill -f "[q]emu-system-x86_64.*$DATA" 2>/dev/null || true
  sleep 1
  rm -rf "$DATA" "$RUNTIME"
}
trap cleanup EXIT

env HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR="$DATA" \
  HORCRUX_DATABASE_URL="sqlite://$DATA/horcrux.db" HORCRUX_SSL_DIR="$DATA/ssl" \
  HORCRUX_VM_STORAGE="$DATA/vms" HORCRUX_CONSOLE_SETTINGS="$DATA/console.json" \
  ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 \
  RUST_LOG=info,horcrux_vm::console=debug \
  nohup "$BIN" >> "$DATA/api.log" 2>&1 &
echo $! > "$DATA/api.pid"
for i in $(seq 1 90); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "FAIL API did not start"; tail -50 "$DATA/api.log"; exit 1; }
test -S /run/horcrux/console.sock || { echo "FAIL no local console socket"; tail -50 "$DATA/api.log"; exit 1; }
echo "ok - the node serves the native viewer socket /run/horcrux/console.sock"

rc=0
HXW_VIEWER="$VIEWER" HXW_ALPINE_ISO="$DATA/images/alpine-virt.iso" HXW_TOOLS_ISO="$DATA/images/tools.iso" \
  HXW_DATA="$DATA" HXW_RUNTIME_DIR="$RUNTIME" HXW_OUT="$OUT" \
  python3 -u "$HERE/viewer_window_e2e.py" || rc=$?
if [ $rc -ne 0 ]; then
  echo "---- api.log (tail)"; tail -60 "$DATA/api.log"
fi
cp "$DATA/api.log" "$OUT/api.log" 2>/dev/null || true
exit $rc
