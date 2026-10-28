#!/bin/bash
# Advanced VM hardware end to end on real QEMU (TCG) in a throwaway sibling
# container: extra serial ports, virtiofs (real virtiofsd), ivshmem, SMBIOS,
# VM Generation ID, NUMA, CPU/memory hotplug, protection, lock and a
# hookscript uploaded as a snippet; every API call is checked against QMP,
# the QEMU command line and the host (vm_advanced_e2e.py).
# Usage: horcrux-api/tests/vm_advanced_e2e.sh <horcrux-api binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-latest}
NAME="horcrux-vmadv-e2e-${HX_RUN_TAG:-$$}"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --memory 3g "$IMAGE" sleep 1800 >/dev/null
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/vm_advanced_e2e.py" "$NAME":/tmp/vm_advanced_e2e.py
docker exec "$NAME" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  qemu-system-x86 qemu-system-common qemu-utils iproute2 curl python3 libvirt0 >/dev/null
  have() { command -v virtiofsd >/dev/null || [ -x /usr/lib/qemu/virtiofsd ] || [ -x /usr/libexec/virtiofsd ]; }
  have || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq virtiofsd >/dev/null || true
  have || { echo "no virtiofsd available in this image"; exit 1; }'

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/dev && cd /tmp/dev
cat > /tmp/dev/start-api.sh <<EOS
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/dev \\
HORCRUX_DATABASE_URL=sqlite:///tmp/dev/horcrux.db HORCRUX_SSL_DIR=/tmp/dev/ssl \\
HORCRUX_VM_STORAGE=/tmp/dev/vms ADMIN_PASSWORD=admin RUST_LOG=info \\
  nohup horcrux-api >> /tmp/dev/api.log 2>&1 &
echo \$! > /tmp/dev/api.pid
for i in \$(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /tmp/dev/api.log; exit 1
EOS
bash /tmp/dev/start-api.sh
python3 /tmp/vm_advanced_e2e.py || { echo "---- api.log"; tail -80 /tmp/dev/api.log; exit 1; }
'
