#!/bin/bash
# System administration end to end, for real: the built horcrux-api runs as
# root in a throwaway privileged sibling container with a real OpenRC (a real
# init system whose services the API starts/stops), chrony, a dummy network
# interface and apt, and system_admin_e2e.py drives the HTTP API and checks
# the host itself (hostname, /etc files, `ip addr`, `rc-status`, `dpkg`).
# Usage: horcrux-api/tests/system_admin_e2e.sh <horcrux-api binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-latest}
NAME="horcrux-sysadm-e2e-$$"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --privileged --memory 1g --memory-swap 1g "$IMAGE" sleep 1800 >/dev/null
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/system_admin_e2e.py" "$NAME":/tmp/system_admin_e2e.py
docker cp "$(dirname "$0")/fixtures/ceph" "$NAME":/tmp/ceph-fx
# `apt-get update` exits 0 even when a mirror index failed to download (then
# universe packages like openrc have "no installation candidate"): retry until
# the package is really known.
docker exec "$NAME" bash -c 'for i in 1 2 3 4 5; do
  apt-get update -qq >/dev/null 2>&1
  apt-cache policy openrc | grep -q "Candidate: [0-9]" && break
  echo "apt index incomplete (attempt $i), retrying"; sleep 10
done
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  openrc chrony tzdata iproute2 curl python3 libvirt0 nftables >/dev/null'

docker exec "$NAME" bash -c '
set -euo pipefail
# A real OpenRC: bring the runtime state up like a boot would.
mkdir -p /run/openrc && touch /run/openrc/softlevel
openrc default >/tmp/openrc-boot.log 2>&1 || true
cat > /etc/init.d/hxtest <<EOS
#!/sbin/openrc-run
description="Horcrux system-admin e2e service"
command=/bin/sleep
command_args="1000"
command_background=true
pidfile=/run/hxtest.pid
EOS
chmod +x /etc/init.d/hxtest
rc-status -s | head -5
mkdir -p /tmp/sa
cat > /tmp/sa/start-api.sh <<EOS
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/sa \\
HORCRUX_DATABASE_URL=sqlite:///tmp/sa/horcrux.db HORCRUX_SSL_DIR=/tmp/sa/ssl \\
ADMIN_PASSWORD=admin RUST_LOG=info \\
  nohup horcrux-api >> /tmp/sa/api.log 2>&1 &
echo \$! > /tmp/sa/api.pid
for i in \$(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /tmp/sa/api.log; exit 1
EOS
bash /tmp/sa/start-api.sh
python3 /tmp/system_admin_e2e.py || { echo "---- api.log"; tail -80 /tmp/sa/api.log; exit 1; }
'
