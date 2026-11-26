#!/bin/bash
# Real-device proof that a macvtap NIC keeps the MAC Horcrux gave it while
# systemd's default udev link policy (MACAddressPolicy=persistent) is active.
#   CI / any docker host:  horcrux-api/tests/macvtap_e2e.sh <horcrux-vm lib test binary> [image]
#       (runs the rest of this script as root in a privileged sibling container with its own
#        network namespace; the host is untouched)
#   inside a container with the cargo workspace:  HX_E2E_INSIDE=1 bash horcrux-api/tests/macvtap_e2e.sh
# The devices are made with the product's own HostNet code (the ignored test
# `macvtap_real_setup_for_e2e`). The udev policy is the real udev when this
# container's udev sees the devices, otherwise a stand-in with the same rule
# (it rewrites a device whose address was assigned at random, a few ms after
# the device appears). A device made with a random address is rewritten (that
# proves the policy is active in this run); the NIC's macvtap keeps its MAC.
set -u
if [ -z "${HX_E2E_INSIDE:-}" ]; then
  BIN=${1:?usage: $0 <horcrux-vm lib test binary> [image]}
  IMAGE=${2:-ubuntu:24.04}
  . "$(dirname "${BASH_SOURCE[0]}")/sibling-apt.sh"
  NAME="horcrux-macvtap-mac-e2e-${HX_RUN_TAG:-$$}"
  trap 'docker rm -fv "$NAME" >/dev/null 2>&1 || true' EXIT
  docker run -d --name "$NAME" --privileged --memory 1g --memory-swap 1g "$IMAGE" sleep 1800 >/dev/null || exit 1
  hx_sibling_apt_prep "$NAME" || exit 1
  docker cp "$BIN" "$NAME":/macvtap-test || exit 1
  docker cp "$0" "$NAME":/macvtap_e2e.sh || exit 1
  docker exec "$NAME" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq iproute2 libvirt0 libpam0g >/dev/null' || exit 1
  docker exec -e HX_E2E_INSIDE=1 -e HX_E2E_TESTBIN=/macvtap-test "$NAME" bash /macvtap_e2e.sh
  exit $?
fi
[ -n "${HX_E2E_TESTBIN:-}" ] || cd "$(dirname "$0")/../.."
W=${HX_E2E_DIR:-/tmp/hx-macvtap-e2e}
rm -rf "$W"
mkdir -p "$W"
pass=0
fail=0
ok() { echo "ok - $*"; pass=$((pass + 1)); }
bad() { echo "FAIL - $*"; fail=$((fail + 1)); }

MAC=9a:05:6d:b7:63:65
BADMAC=96:ba:38:df:23:ae

ip link add vp0 type veth peer name vr0
ip link set vp0 up
ip link set vr0 up

# udev: the real one if it sees our devices, else the stand-in
mkdir -p /run/udev
if [ -x /lib/systemd/systemd-udevd ] && [ -z "${HX_E2E_TESTBIN:-}" ]; then
  /lib/systemd/systemd-udevd --daemon 2>/dev/null
  sleep 1
fi
RACER=""
ip link add link vp0 name ctl0 type macvtap mode bridge
c0=$(cat /sys/class/net/ctl0/address)
sleep 1.5
c1=$(cat /sys/class/net/ctl0/address)
if [ "$c0" != "$c1" ]; then
  udev=real
  ok "real udev rewrote the random MAC of a plain macvtap: $c0 -> $c1"
else
  udev=standin
  (
    ip -o monitor link 2>/dev/null | while read -r first name _; do
      [ "$first" = Deleted ] && continue
      n=${name%:}
      n=${n%%@*}
      case "$n" in
        tap* | ctl0)
          t=$(cat /sys/class/net/$n/addr_assign_type 2>/dev/null)
          # systemd rewrites only addresses assigned at random (1) or stolen (2)
          if [ "$t" = 1 ] || [ "$t" = 2 ]; then
            sleep 0.1
            ip link set "$n" address $BADMAC 2>/dev/null
          fi
          ;;
      esac
    done
  ) &
  RACER=$!
  sleep 0.5
  ip link del ctl0
  ip link add link vp0 name ctl0 type macvtap mode bridge
  sleep 1
  if [ "$(cat /sys/class/net/ctl0/address)" = $BADMAC ]; then ok "stand-in udev policy rewrote the random MAC of a plain macvtap"; else bad "stand-in udev policy did not rewrite a random MAC"; fi
fi
ip link del ctl0

rm -f "$W/ready" "$W/done"
HX_E2E_PARENT=vp0 HX_E2E_MAC=$MAC HX_E2E_READY=$W/ready HX_E2E_DONE=$W/done \
  ${HX_E2E_TESTBIN:-cargo test -p horcrux-vm --lib --} --ignored --nocapture macvtap_real_setup_for_e2e >"$W/setup.log" 2>&1 &
pid=$!
for _ in $(seq 1 900); do [ -s "$W/ready" ] && break; sleep 1; done
if ! grep -q 'Ok(())' "$W/ready" 2>/dev/null; then
  bad "setup: $(cat "$W/ready" 2>/dev/null)"
  tail -5 "$W/setup.log"
else
  tap=$(cut -d' ' -f1 "$W/ready")
  sleep 2
  got=$(cat /sys/class/net/$tap/address)
  if [ "$got" = $MAC ]; then ok "macvtap $tap keeps $MAC with the udev policy active ($udev)"; else bad "macvtap $tap has MAC $got, want $MAC"; fi
fi
touch "$W/done"
wait $pid

[ -n "$RACER" ] && kill $RACER 2>/dev/null
ip link del vp0 2>/dev/null
echo "passed=$pass failed=$fail"
[ $fail = 0 ]
