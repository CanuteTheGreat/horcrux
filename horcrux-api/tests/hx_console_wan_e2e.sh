#!/bin/bash
# Horcrux Console phase C3 (WAN) end to end: a real QEMU guest viewed from a
# real Chromium that runs in its own network namespace behind a veth link
# shaped with netem (fixed RTT, loss, rate), directly (LAN) and through the
# node's built-in relay (TURN over UDP, then TCP when UDP is blocked).
# Prints LATENCY/PATH lines per path (docs/design/horcrux-console.md
# section 6) and RESULTS {...}.
#
# Usage: horcrux-api/tests/hx_console_wan_e2e.sh <horcrux-api binary> [ui dist dir]
# Needs root (or sudo), CAP_NET_ADMIN, iproute2 with netem, iptables, KVM.
# Env: HXC_CACHE (ISO cache), HXC_OUT, HXC_PW_PY (python with playwright),
#      HXC_WAN_CONDITIONS (comma list, default all).
set -euo pipefail
BIN=$(readlink -f "${1:?usage: $0 <horcrux-api binary> [ui dist dir]}")
UI=${2:-}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CACHE=${HXC_CACHE:-$HOME/.cache/hxc}
OUT=${HXC_OUT:-/tmp/hxc-wan-e2e}
DATA=$(mktemp -d /tmp/hxc-wan-data.XXXXXX)
SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
NS=hxcwan
mkdir -p "$CACHE" "$OUT"

need=()
command -v qemu-system-x86_64 >/dev/null || need+=(qemu-system-x86 qemu-utils qemu-system-modules-opengl)
command -v ffmpeg >/dev/null || need+=(ffmpeg)
command -v curl >/dev/null || need+=(curl)
command -v ip >/dev/null || need+=(iproute2)
command -v tc >/dev/null || need+=(iproute2)
command -v iptables >/dev/null || need+=(iptables)
ldd "$BIN" 2>/dev/null | grep -q 'libvirt.so.* not found' && need+=(libvirt0)
if [ ${#need[@]} -gt 0 ]; then
  for i in 1 2 3; do
    $SUDO apt-get update -qq >/dev/null 2>&1 || true
    apt-cache policy iproute2 | grep -q 'Candidate: [0-9]' && break
    sleep 10
  done
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${need[@]}" python3-venv >/dev/null
fi
qemu-system-x86_64 -display help | grep -qx dbus || { echo "FAIL QEMU has no D-Bus display module"; exit 1; }
# netem lives in a kernel module on some hosts; the qdisc add below fails loudly if it is missing
echo "ok - $(qemu-system-x86_64 --version | head -1), $(tc -V 2>&1 | head -1)"

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

# Alpine "virt" ISO: root console without a password (test fixture,
# downloaded at test time, checksum from Alpine's latest-releases.yaml).
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

# The "WAN": the client lives in netns $NS, linked to the node by a veth
# pair (node 10.77.0.1, client 10.77.0.2). Shaping is set per condition by
# the test (tc netem on both ends).
cleanup() {
  [ -f "$DATA/api.pid" ] && kill "$(cat "$DATA/api.pid")" 2>/dev/null || true
  pkill -f "[q]emu-system-x86_64.*$DATA" 2>/dev/null || true
  $SUDO ip netns del $NS 2>/dev/null || true
  $SUDO ip link del hxwan0 2>/dev/null || true
  sleep 1
  rm -rf "$DATA"
}
trap cleanup EXIT
$SUDO ip netns del $NS 2>/dev/null || true
$SUDO ip link del hxwan0 2>/dev/null || true
$SUDO ip netns add $NS
$SUDO ip link add hxwan0 type veth peer name hxwan1
$SUDO ip link set hxwan1 netns $NS
$SUDO ip addr add 10.77.0.1/24 dev hxwan0
$SUDO ip link set hxwan0 up
$SUDO ip -n $NS addr add 10.77.0.2/24 dev hxwan1
$SUDO ip -n $NS link set hxwan1 up
$SUDO ip -n $NS link set lo up
$SUDO tc qdisc add dev hxwan0 root netem delay 0ms
$SUDO ip netns exec $NS tc qdisc add dev hxwan1 root netem delay 0ms
$SUDO ip netns exec $NS ping -c1 -W2 10.77.0.1 >/dev/null || { echo "FAIL veth link"; exit 1; }
echo "ok - client network namespace $NS behind a netem-shaped veth link"

ui_env=()
[ -n "$UI" ] && ui_env=(HORCRUX_UI_DIR="$(readlink -f "$UI")")
env HORCRUX_HOST=0.0.0.0 HORCRUX_PORT=8006 HORCRUX_DATA_DIR="$DATA" \
  HORCRUX_DATABASE_URL="sqlite://$DATA/horcrux.db" HORCRUX_SSL_DIR="$DATA/ssl" \
  HORCRUX_VM_STORAGE="$DATA/vms" HORCRUX_CONSOLE_SETTINGS="$DATA/console.json" \
  ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 \
  RUST_LOG=info,horcrux_vm::console=debug,horcrux_turn=debug "${ui_env[@]}" \
  nohup "$BIN" >> "$DATA/api.log" 2>&1 &
echo $! > "$DATA/api.pid"
for i in $(seq 1 90); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "FAIL API did not start"; tail -50 "$DATA/api.log"; exit 1; }

rc=0
HXC_NS=$NS HXC_ALPINE_ISO="$DATA/images/alpine-virt.iso" HXC_OUT="$OUT" SUDO="$SUDO" \
  "$PY" -u "$HERE/hx_console_wan_e2e.py" || rc=$?
if [ $rc -ne 0 ]; then
  echo "---- api.log (tail)"; tail -150 "$DATA/api.log"
fi
cp "$DATA/api.log" "$OUT/api.log" 2>/dev/null || true
exit $rc
