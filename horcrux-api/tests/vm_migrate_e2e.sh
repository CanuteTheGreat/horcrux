#!/bin/bash
# Two-node VM migration end to end, for real (see vm_migrate_e2e.py for what
# is checked): two Horcrux nodes in throwaway sibling containers, each with
# its own network namespace (CAP_NET_ADMIN, /dev/net/tun, /dev/kvm when the
# runner has it), on one private docker network, sharing a docker volume at
# /shared, running the built horcrux-api with a shared JWT_SECRET (the
# cluster's trust root for node-to-node calls) and real QEMU + OVMF + swtpm
# with CirrOS guests. Nothing on the runner host is touched.
#
# Usage: horcrux-api/tests/vm_migrate_e2e.sh <path-to-horcrux-api-binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-latest}
HERE=$(cd "$(dirname "$0")" && pwd)
TAG="${HXMIG_TAG:-hxmig-$$}"
N1="$TAG-n1"
N2="$TAG-n2"
NET="$TAG-net"
VOL="$TAG-shared"
CIRROS_URL=${CIRROS_URL:-https://download.cirros-cloud.net/0.6.2/cirros-0.6.2-x86_64-disk.img}

cleanup() {
  for n in "$N1" "$N2"; do
    if [ "${KEEP:-0}" != 1 ]; then docker rm -f "$n" >/dev/null 2>&1 || true; fi
  done
  docker network rm "$NET" >/dev/null 2>&1 || true
  docker volume rm "$VOL" >/dev/null 2>&1 || true
}
trap cleanup EXIT

DEV=(--device /dev/net/tun)
if [ -e /dev/kvm ]; then DEV+=(--device /dev/kvm); echo "KVM: yes"; else echo "KVM: no (TCG)"; fi
docker network create "$NET" >/dev/null
docker volume create "$VOL" >/dev/null
for n in "$N1" "$N2"; do
  host=node1; [ "$n" = "$N2" ] && host=node2
  docker run -d --name "$n" --hostname "$host" --network "$NET" --cap-add NET_ADMIN "${DEV[@]}" \
    --memory 3g -v "$VOL":/shared "$IMAGE" sleep 7200 >/dev/null
done

curl -sfL --retry 3 -o /tmp/$TAG-cirros.img "$CIRROS_URL"
JWT=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
for n in "$N1" "$N2"; do
  docker exec "$n" mkdir -p /hx /var/lib/hx/images /shared/vms
  docker cp "$BIN" "$n":/usr/local/bin/horcrux-api
  docker cp "$HERE/hx_node.py" "$n":/hx/hx_node.py
  docker cp /tmp/$TAG-cirros.img "$n":/var/lib/hx/images/cirros.img
  cat > /tmp/$TAG-start-api.sh <<EOS
#!/bin/bash
# start-api.sh [vm storage dir]: (re)start this node's API
set -e
STORAGE=\${1:-/var/lib/hx/vms}
if [ -f /var/lib/hx/api.pid ]; then kill \$(cat /var/lib/hx/api.pid) 2>/dev/null || true; sleep 2; fi
HORCRUX_HOST=0.0.0.0 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/var/lib/hx \\
HORCRUX_DATABASE_URL=sqlite:///var/lib/hx/horcrux.db HORCRUX_SSL_DIR=/var/lib/hx/ssl \\
HORCRUX_VM_STORAGE=\$STORAGE ADMIN_PASSWORD=admin JWT_SECRET=$JWT \\
RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info \\
HORCRUX_MIGRATION_PEER_TTL_SECS=${PEER_TTL:-20} \\
  nohup horcrux-api >> /var/lib/hx/api.log 2>&1 &
echo \$! > /var/lib/hx/api.pid
rm -f /tmp/hx-token-*
for i in \$(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /var/lib/hx/api.log; exit 1
EOS
  docker cp /tmp/$TAG-start-api.sh "$n":/hx/start-api.sh
done
rm -f /tmp/$TAG-cirros.img /tmp/$TAG-start-api.sh

for n in "$N1" "$N2"; do
  docker exec "$n" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    qemu-system-x86 qemu-utils ovmf swtpm swtpm-tools iproute2 nftables curl python3 libvirt0 genisoimage >/dev/null' &
done
wait
for n in "$N1" "$N2"; do docker exec "$n" bash /hx/start-api.sh; done

IP1=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$N1")
IP2=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$N2")
echo "node1=$IP1 node2=$IP2"
rc=0
N1=$N1 N2=$N2 IP1=$IP1 IP2=$IP2 python3 "$HERE/vm_migrate_e2e.py" || rc=$?
if [ $rc -ne 0 ]; then
  for n in "$N1" "$N2"; do echo "---- $n api.log"; docker exec "$n" tail -120 /var/lib/hx/api.log || true; done
fi
exit $rc
