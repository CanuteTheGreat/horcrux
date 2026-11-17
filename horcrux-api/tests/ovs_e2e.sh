#!/bin/bash
# Open vSwitch end to end against a real ovsdb-server/ovs-vswitchd in a privileged
# sibling container (its own network namespace; the runner host is untouched):
# bridge/port/flow lifecycle through the API, checked with ovs-vsctl/ovs-ofctl and
# `ip link`, plus the structured 424 on a node without Open vSwitch.
# Usage: horcrux-api/tests/ovs_e2e.sh <horcrux-api binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-ubuntu:24.04}
NAME="horcrux-ovs-e2e-${HX_RUN_TAG:-$$}"
cleanup() { docker rm -fv "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --privileged --memory 1g --memory-swap 1g -v /lib/modules:/lib/modules:ro "$IMAGE" sleep 1800 >/dev/null
bash "$(dirname "$0")/../../ci/sysctl-guard.sh" harden "$NAME"
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/ovs_e2e.py" "$NAME":/tmp/ovs_e2e.py
docker exec "$NAME" bash -c 'rm -f /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*ppa*.sources 2>/dev/null; apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  openvswitch-switch kmod iproute2 curl python3 libvirt0 >/dev/null'

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/ovs && cd /tmp/ovs
cat > start-api.sh <<EOS
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/ovs \
HORCRUX_DATABASE_URL=sqlite:///tmp/ovs/horcrux.db HORCRUX_SSL_DIR=/tmp/ovs/ssl \
ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info \
  nohup horcrux-api >> /tmp/ovs/api.log 2>&1 &
echo \$! > /tmp/ovs/api.pid
for i in \$(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /tmp/ovs/api.log; exit 1
EOS
# phase 1: no ovs-vsctl on the node
mv /usr/bin/ovs-vsctl /usr/bin/ovs-vsctl.real
bash start-api.sh
python3 /tmp/ovs_e2e.py missing || { tail -60 api.log; exit 1; }
kill $(cat /tmp/ovs/api.pid); sleep 1
mv /usr/bin/ovs-vsctl.real /usr/bin/ovs-vsctl
# phase 2: real Open vSwitch
/usr/share/openvswitch/scripts/ovs-ctl start --system-id=random >/tmp/ovs/ovs-ctl.log 2>&1 || { cat /tmp/ovs/ovs-ctl.log; lsmod | grep -i openvswitch || true; exit 1; }
ovs-vsctl show >/dev/null
bash start-api.sh
python3 /tmp/ovs_e2e.py real || { echo "---- api.log"; tail -60 api.log; echo "---- ovs"; ovs-vsctl show; exit 1; }
'
