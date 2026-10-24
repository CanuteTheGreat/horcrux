#!/bin/bash
# SDN networks + network policies end to end, for real: runs the built
# horcrux-api in a throwaway privileged sibling container (its own network
# namespace; nothing on the runner host is touched) with the real CNI
# bridge + host-local plugins and nftables, then drives the HTTP API
# (sdn_cni_e2e.py):
#
#   - network names can't escape the conf dir, bad subnets are refused
#   - two network namespaces attached through POST /api/cni/attach get
#     real IPs on the bridge and reach each other; the attachment is
#     recorded and Delete on the network is refused while they are attached
#   - a deny-ingress policy really blocks traffic (bridge horcrux_netpol in
#     the kernel ruleset), an allow rule lets one port through
#   - after an API restart (and the kernel table removed, as after a host
#     reboot) networks, attachments and policies are back and enforced
#   - detach releases the host-local leases, delete removes the conflist
#   - the UI contract test's SDN parts run against this server with a real
#     network namespace to attach
#
# The CI Test job container is unprivileged, so this can't run there.
# Usage: horcrux-api/tests/sdn_cni_e2e.sh <horcrux-api binary> [api_contract test binary] [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [api_contract binary] [image]}
CONTRACT=${2:-}
IMAGE=${3:-catthehacker/ubuntu:act-latest}
NAME="horcrux-sdn-e2e-$$"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --privileged --memory 1g --memory-swap 1g "$IMAGE" sleep 1800 >/dev/null
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/sdn_cni_e2e.py" "$NAME":/tmp/sdn_cni_e2e.py
if [ -n "$CONTRACT" ]; then
  docker cp "$CONTRACT" "$NAME":/usr/local/bin/api_contract
fi
docker exec "$NAME" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  containernetworking-plugins nftables iproute2 iputils-ping curl python3 libvirt0 >/dev/null'

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/sdn/cni.d && cd /tmp/sdn
CNI_BIN=$(for d in /opt/cni/bin /usr/lib/cni /usr/libexec/cni; do [ -x $d/bridge ] && echo $d && break; done)
test -n "$CNI_BIN" || { echo "no CNI bridge plugin installed"; exit 1; }
cat > /tmp/sdn/start-api.sh <<EOS
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/sdn \
HORCRUX_DATABASE_URL=sqlite:///tmp/sdn/horcrux.db HORCRUX_SSL_DIR=/tmp/sdn/ssl \
HORCRUX_CNI_BIN_DIR=$CNI_BIN HORCRUX_CNI_CONF_DIR=/tmp/sdn/cni.d \
ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info \
  nohup horcrux-api >> /tmp/sdn/api.log 2>&1 &
echo \$! > /tmp/sdn/api.pid
for i in \$(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /tmp/sdn/api.log; exit 1
EOS
bash /tmp/sdn/start-api.sh
python3 /tmp/sdn_cni_e2e.py || { echo "---- api.log"; tail -80 /tmp/sdn/api.log; echo "---- nft"; nft list ruleset || true; exit 1; }
'
