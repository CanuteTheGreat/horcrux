#!/bin/bash
# HA failover end to end, for real (see ha_failover_e2e.py for what is
# checked): three Horcrux nodes in throwaway sibling containers (own network
# namespaces, one docker network, a shared docker volume at /shared standing
# in for shared storage), each with its OWN token secret, running the built
# horcrux-api with real QEMU. Nothing on the runner host is touched.
#
# Usage: horcrux-api/tests/ha_failover_e2e.sh <path-to-horcrux-api-binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-latest}
HERE=$(cd "$(dirname "$0")" && pwd)
TAG="${HXHA_TAG:-hxha-$$}"
NET="$TAG-net"
VOL="$TAG-shared"
NODES=("$TAG-n1" "$TAG-n2" "$TAG-n3")

cleanup() {
  for n in "${NODES[@]}"; do
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
i=0
for n in "${NODES[@]}"; do
  i=$((i+1))
  docker run -d --name "$n" --hostname "node$i" --network "$NET" --cap-add NET_ADMIN "${DEV[@]}" \
    --memory 2g -v "$VOL":/shared "$IMAGE" sleep 7200 >/dev/null
done

i=0
for n in "${NODES[@]}"; do
  i=$((i+1))
  docker exec "$n" mkdir -p /hx /var/lib/hx /shared/vms
  docker cp "$BIN" "$n":/usr/local/bin/horcrux-api
  docker cp "$HERE/hx_node.py" "$n":/hx/hx_node.py
  # Each node has its own token secret: only the cluster join can make
  # their service tokens verify on each other.
  SECRET=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
  cat > /tmp/$TAG-start-api.sh <<EOS
#!/bin/bash
# (re)start this node's API with fast HA timings
set -e
if [ -f /var/lib/hx/api.pid ]; then kill \$(cat /var/lib/hx/api.pid) 2>/dev/null || true; sleep 2; fi
HORCRUX_HOST=0.0.0.0 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/var/lib/hx \\
HORCRUX_DATABASE_URL=sqlite:///var/lib/hx/horcrux.db HORCRUX_SSL_DIR=/var/lib/hx/ssl \\
HORCRUX_VM_STORAGE=/shared/vms ADMIN_PASSWORD=admin JWT_SECRET=$SECRET \\
RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info \\
HORCRUX_CLUSTER_POLL_SECS=2 HORCRUX_CLUSTER_STALE_SECS=6 HORCRUX_HA_TICK_SECS=2 \\
HORCRUX_HA_SELF_FENCE_SECS=8 HORCRUX_HA_RECOVERY_MARGIN_SECS=4 \\
  nohup horcrux-api >> /var/lib/hx/api.log 2>&1 &
echo \$! > /var/lib/hx/api.pid
rm -f /tmp/hx-token-*
for i in \$(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /var/lib/hx/api.log; exit 1
EOS
  docker cp /tmp/$TAG-start-api.sh "$n":/hx/start-api.sh
done
rm -f /tmp/$TAG-start-api.sh

for n in "${NODES[@]}"; do
  docker exec "$n" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    qemu-system-x86 qemu-utils ovmf iproute2 nftables curl python3 libvirt0 procps >/dev/null' &
done
wait
for n in "${NODES[@]}"; do docker exec "$n" bash /hx/start-api.sh; done

declare -A ENVV
for k in 1 2 3; do
  ip=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "${NODES[$((k-1))]}")
  echo "node$k=$ip"
  export "IP$k=$ip" "N$k=${NODES[$((k-1))]}"
done
export NET
rc=0
python3 "$HERE/ha_failover_e2e.py" || rc=$?
if [ $rc -ne 0 ]; then
  for n in "${NODES[@]}"; do echo "---- $n api.log"; docker exec "$n" tail -80 /var/lib/hx/api.log || true; done
fi
exit $rc
