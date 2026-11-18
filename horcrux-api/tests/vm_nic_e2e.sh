#!/bin/bash
# VM networking + hardware end to end, for real: runs the built horcrux-api in
# a throwaway sibling container with CAP_NET_ADMIN and /dev/net/tun (its own
# network namespace, nothing on the runner host is touched) and real QEMU
# (TCG), then drives everything through the HTTP API:
#
#   - Host Bridges: create a managed VLAN-aware bridge with an address,
#     re-created after it disappears (host reboot) when the API restarts
#   - ISO library: download an ISO from a URL
#   - a VM with a VirtIO NIC on that bridge, VLAN 20, booting from the
#     network: its tap device is a port of the bridge with PVID 20, and the
#     guest's PXE ROM really sends DHCP from the VM's MAC on VLAN 20
#   - live CD-ROM insert/eject in the running guest (QMP query-block)
#   - VM firewall: an outbound DROP udp/67 rule really stops the guest's DHCP
#     from leaving the bridge (bridge-family nftables on the VM's taps), and
#     deleting the rule lets it through
#   - NIC changes refused while running; Shutdown (ignored ACPI -> 409, then
#     force), Reboot, Reset; taps removed on stop
#
# The CI Test job container is unprivileged, so this can't run there.
# Usage: horcrux-api/tests/vm_nic_e2e.sh <path-to-horcrux-api-binary> [image]
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/sibling-apt.sh"
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-26.04}
NAME="horcrux-vmnic-e2e-${HX_RUN_TAG:-$$}"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

# macvtap NICs: QEMU gets /dev/tap<ifindex>, a character device whose major is
# allocated when the macvtap module loads. Load it (making one macvtap in a
# throwaway network namespace does) and let the sibling use exactly that major.
docker run -d --name "$NAME-mvt" --cap-add NET_ADMIN "$IMAGE" sleep 300 >/dev/null
hx_sibling_apt_prep "$NAME-mvt"
docker exec "$NAME-mvt" sh -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq iproute2 >/dev/null \
  && ip link add hxd0 type dummy && ip link add link hxd0 name hxm0 type macvtap' || true
docker rm -f "$NAME-mvt" >/dev/null 2>&1 || true
MVT_MAJOR=$(awk '$2 == "macvtap" {print $1}' /proc/devices)
[ -n "$MVT_MAJOR" ] || { echo "FAIL: no macvtap character device major (macvtap module missing on this runner)"; exit 1; }

docker run -d --name "$NAME" --cap-add NET_ADMIN --device /dev/net/tun \
  --device-cgroup-rule "c $MVT_MAJOR:* rwm" --memory 2g "$IMAGE" sleep 1800 >/dev/null
hx_sibling_apt_prep "$NAME"
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/vm_nic_e2e.py" "$NAME":/tmp/vm_nic_e2e.py
docker exec "$NAME" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  qemu-system-x86 qemu-utils ipxe-qemu iproute2 tcpdump nftables curl python3 libvirt0 genisoimage >/dev/null'

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/nic && cd /tmp/nic
cat > /tmp/nic/start-api.sh <<EOS
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/nic \
HORCRUX_DATABASE_URL=sqlite:///tmp/nic/horcrux.db HORCRUX_SSL_DIR=/tmp/nic/ssl \
HORCRUX_VM_STORAGE=/tmp/nic/vms ADMIN_PASSWORD=admin RUST_LOG=info \
  nohup horcrux-api >> /tmp/nic/api.log 2>&1 &
echo \$! > /tmp/nic/api.pid
for i in \$(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /tmp/nic/api.log; exit 1
EOS
bash /tmp/nic/start-api.sh
python3 /tmp/vm_nic_e2e.py || { echo "---- api.log"; tail -80 /tmp/nic/api.log; exit 1; }
'
