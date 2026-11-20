#!/bin/bash
# Horcrux Console same-host viewer, node half (hx_local_viewer_e2e.py): a real
# horcrux-api, a real QEMU guest (Alpine) with the frame ring and D-Bus audio,
# horcrux-viewer and a raw local-socket client. Single standalone node; KVM
# when /dev/kvm is usable. Runs as root (QEMU, /run/horcrux, /dev/shm).
#
# Usage: horcrux-api/tests/hx_local_viewer_e2e.sh <horcrux-api binary> <horcrux-viewer binary>
# Env:   HXC_CACHE (ISO cache, default ~/.cache/hxc), HXL_OUT (results)
set -euo pipefail
BIN=$(readlink -f "${1:?usage: $0 <horcrux-api binary> <horcrux-viewer binary>}")
VIEWER=$(readlink -f "${2:?usage: $0 <horcrux-api binary> <horcrux-viewer binary>}")
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CACHE=${HXC_CACHE:-$HOME/.cache/hxc}
OUT=${HXL_OUT:-/tmp/hxl-e2e}
DATA=$(mktemp -d /tmp/hxl-data.XXXXXX)
SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
mkdir -p "$CACHE" "$OUT"

need=()
command -v qemu-system-x86_64 >/dev/null || need+=(qemu-system-x86 qemu-utils)
command -v curl >/dev/null || need+=(curl)
ldd "$BIN" 2>/dev/null | grep -q 'libvirt.so.* not found' && need+=(libvirt0)
if ! command -v qemu-system-x86_64 >/dev/null || ! qemu-system-x86_64 -display help | grep -qx dbus; then
  need+=(qemu-system-modules-opengl)
fi
command -v ffmpeg >/dev/null || need+=(ffmpeg)
if [ ${#need[@]} -gt 0 ]; then
  for i in 1 2 3; do
    $SUDO apt-get update -qq >/dev/null 2>&1 || true
    apt-cache policy qemu-utils | grep -q 'Candidate: [0-9]' && break
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
mkdir -p "$DATA/images" && cp "$AISO" "$DATA/images/alpine-virt.iso"

cleanup() {
  [ -f "$DATA/api.pid" ] && kill "$(cat "$DATA/api.pid")" 2>/dev/null || true
  pkill -f "[q]emu-system-x86_64.*$DATA" 2>/dev/null || true
  sleep 1
  rm -rf "$DATA"
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
HXL_VIEWER="$VIEWER" HXL_ALPINE_ISO="$DATA/images/alpine-virt.iso" HXL_DATA="$DATA" HXL_OUT="$OUT" \
  python3 -u "$HERE/hx_local_viewer_e2e.py" || rc=$?
if [ $rc -ne 0 ]; then
  echo "---- api.log (tail)"; tail -80 "$DATA/api.log"
fi
cp "$DATA/api.log" "$OUT/api.log" 2>/dev/null || true
exit $rc
