#!/bin/bash
# VM devices end to end, for real QEMU (TCG) in a throwaway sibling container:
# memory balloon (virtio-balloon + the `balloon` QMP command), the guest agent
# channel (virtio-serial socket on the host) and the USB controller /
# passed-through device from VM hardware; the API and QMP are checked against
# each other (vm_devices_e2e.py).
# Usage: horcrux-api/tests/vm_devices_e2e.sh <horcrux-api binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-latest}
NAME="horcrux-vmdev-e2e-${HX_RUN_TAG:-$$}"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --memory 2g "$IMAGE" sleep 1800 >/dev/null
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/vm_devices_e2e.py" "$NAME":/tmp/vm_devices_e2e.py
docker exec "$NAME" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  qemu-system-x86 qemu-utils iproute2 curl python3 libvirt0 >/dev/null'

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
python3 /tmp/vm_devices_e2e.py || { echo "---- api.log"; tail -80 /tmp/dev/api.log; exit 1; }
'
