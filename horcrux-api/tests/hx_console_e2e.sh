#!/bin/bash
# Horcrux Console (phase C1) end to end: real QEMU guests with the D-Bus
# display, viewed through the node's WebRTC client in headless Chromium
# (hx_console_e2e.py). Single standalone node; KVM when /dev/kvm is usable.
#
# Usage: horcrux-api/tests/hx_console_e2e.sh <horcrux-api binary> [ui dist dir]
# Env:   HXC_CACHE (ISO cache, default ~/.cache/hxc), HXC_WIN_ISO (optional
#        Windows ISO: also view Windows Setup), HXC_OUT (screenshots/results),
#        HXC_PW_PY (python with playwright; installed into /tmp/hxc-pw if unset)
set -euo pipefail
BIN=$(readlink -f "${1:?usage: $0 <horcrux-api binary> [ui dist dir]}")
UI=${2:-}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CACHE=${HXC_CACHE:-$HOME/.cache/hxc}
OUT=${HXC_OUT:-/tmp/hxc-e2e}
DATA=$(mktemp -d /tmp/hxc-data.XXXXXX)
SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
mkdir -p "$CACHE" "$OUT"

need=()
command -v qemu-system-x86_64 >/dev/null || need+=(qemu-system-x86 qemu-utils)
command -v ffmpeg >/dev/null || need+=(ffmpeg)
command -v curl >/dev/null || need+=(curl)
# a binary built elsewhere (dc2-rust) links libvirt/libpam
ldd "$BIN" 2>/dev/null | grep -q 'libvirt.so.* not found' && need+=(libvirt0)
if command -v qemu-system-x86_64 >/dev/null && ! qemu-system-x86_64 -display help | grep -qx dbus; then
  need+=(qemu-system-modules-opengl)
elif ! command -v qemu-system-x86_64 >/dev/null; then
  need+=(qemu-system-modules-opengl)
fi
if [ ${#need[@]} -gt 0 ]; then
  for i in 1 2 3; do
    $SUDO apt-get update -qq >/dev/null 2>&1 || true
    apt-cache policy ffmpeg | grep -q 'Candidate: [0-9]' && break
    sleep 10
  done
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${need[@]}" python3-venv >/dev/null
fi
qemu-system-x86_64 -display help | grep -qx dbus || { echo "FAIL QEMU has no D-Bus display module"; exit 1; }
echo "ok - QEMU $(qemu-system-x86_64 --version | head -1) with the D-Bus display"

PY=${HXC_PW_PY:-}
if [ -z "$PY" ]; then
  if [ ! -x /tmp/hxc-pw/bin/python ]; then
    python3 -m venv /tmp/hxc-pw
    /tmp/hxc-pw/bin/pip install -q playwright
    $SUDO /tmp/hxc-pw/bin/playwright install-deps chromium >/dev/null
  fi
  /tmp/hxc-pw/bin/playwright install chromium >/dev/null
  PY=/tmp/hxc-pw/bin/python
fi

# Tiny Core Linux: a ~25 MB ISO that boots to an X desktop (test fixture,
# downloaded at test time; GPL, not shipped).
ISO="$CACHE/TinyCore-current.iso"
if [ ! -s "$ISO" ]; then
  for v in 17.x 16.x 15.x; do
    if curl -fsSL --retry 3 -o "$ISO.part" "http://tinycorelinux.net/$v/x86/release/TinyCore-current.iso"; then
      mv "$ISO.part" "$ISO"; break
    fi
  done
fi
[ -s "$ISO" ] || { echo "FAIL cannot download the Tiny Core ISO"; exit 1; }
mkdir -p "$DATA/images" && cp "$ISO" "$DATA/images/tinycore.iso"
# Alpine "virt" ISO (~70 MB, root console without a password): the in-OS
# keyboard, pointer and click checks read /dev/input/mice in the guest.
# Tiny Core's X server ignores QEMU's pointer devices, so it only serves the
# pre-OS (boot loader) check. Version and checksum come from Alpine's own
# latest-releases.yaml (overridable: HXC_ALPINE_MIRROR).
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
cp "$AISO" "$DATA/images/alpine-virt.iso"

cleanup() {
  [ -f "$DATA/api.pid" ] && kill "$(cat "$DATA/api.pid")" 2>/dev/null || true
  pkill -f "[q]emu-system-x86_64.*$DATA" 2>/dev/null || true
  sleep 1
  rm -rf "$DATA"
}
trap cleanup EXIT

ui_env=()
[ -n "$UI" ] && ui_env=(HORCRUX_UI_DIR="$(readlink -f "$UI")")
env HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR="$DATA" \
  HORCRUX_DATABASE_URL="sqlite://$DATA/horcrux.db" HORCRUX_SSL_DIR="$DATA/ssl" \
  HORCRUX_VM_STORAGE="$DATA/vms" HORCRUX_CONSOLE_SETTINGS="$DATA/console.json" \
  ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 \
  RUST_LOG=info,horcrux_vm::console=debug "${ui_env[@]}" \
  nohup "$BIN" >> "$DATA/api.log" 2>&1 &
echo $! > "$DATA/api.pid"
for i in $(seq 1 90); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "FAIL API did not start"; tail -50 "$DATA/api.log"; exit 1; }

# the Windows ISO goes into the ISO library too (hard link when possible)
if [ -n "${HXC_WIN_ISO:-}" ] && [ -s "$HXC_WIN_ISO" ]; then
  ln "$HXC_WIN_ISO" "$DATA/images/windows.iso" 2>/dev/null || cp "$HXC_WIN_ISO" "$DATA/images/windows.iso"
  export HXC_WIN_ISO="$DATA/images/windows.iso"
fi

rc=0
HXC_LINUX_ISO="$DATA/images/tinycore.iso" HXC_ALPINE_ISO="$DATA/images/alpine-virt.iso" HXC_OUT="$OUT" HXC_UI=$([ -n "$UI" ] && echo 1 || echo 0) \
  "$PY" -u "$HERE/hx_console_e2e.py" || rc=$?
if [ $rc -ne 0 ]; then
  echo "---- api.log (tail)"; tail -120 "$DATA/api.log"
fi
cp "$DATA/api.log" "$OUT/api.log" 2>/dev/null || true
exit $rc
