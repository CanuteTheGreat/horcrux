#!/bin/bash
# Storage plugin end to end with the reference SSHFS plugin against a real SSH
# server (openssh in its own container on a private docker network) and the
# real FUSE sshfs client in the API's container:
#   - the plugin is installed like its package does (root-owned, 0755, in the
#     plugin directory) and is listed with its schema and hints;
#   - a pool config is validated against the schema (out-of-range port: 400);
#   - a pool is created through the API (the plugin mounts the export), a
#     volume is created, grown, snapshotted, rolled back and deleted, and each
#     step is checked on the SSH server's disk;
#   - a VM whose first disk is on the plugin pool boots: its disk holds a
#     boot sector that prints a marker on the serial port;
#   - removing the pool unmounts the export.
# Usage: horcrux-api/tests/storage_plugin_e2e.sh <horcrux-api binary> <sshfs plugin binary> [image] [server image]
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/sibling-apt.sh"
BIN=${1:?usage: $0 <horcrux-api binary> <sshfs plugin binary> [image] [server image]}
PLUGIN=${2:?usage: $0 <horcrux-api binary> <sshfs plugin binary> [image] [server image]}
IMAGE=${3:-catthehacker/ubuntu:act-26.04}
SRV_IMAGE=${4:-debian:trixie}
TAG="${HX_RUN_TAG:-$$}"
NET="horcrux-sp-e2e-$TAG"; SRV="horcrux-sp-srv-$TAG"; CLI="horcrux-sp-cli-$TAG"

cleanup() {
  docker exec "$CLI" sh -c 'pkill -x qemu-system-x86_64; pkill horcrux-api; sleep 1; for m in $(awk "\$3 ~ /^fuse/ {print \$2}" /proc/mounts); do fusermount3 -u "$m" || umount -l "$m"; done' >/dev/null 2>&1 || true
  docker rm -fv "$CLI" >/dev/null 2>&1 || true
  docker rm -fv "$SRV" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker network create "$NET" >/dev/null
docker run -d --name "$SRV" --network "$NET" --memory 512m --ulimit nofile=8192:8192 -v /srv/hx "$SRV_IMAGE" sleep 3600 >/dev/null
hx_sibling_apt_prep "$SRV"
docker run -d --name "$CLI" --network "$NET" --privileged --device /dev/fuse --memory 2g --ulimit nofile=8192:8192 "$IMAGE" sleep 3600 >/dev/null
hx_sibling_apt_prep "$CLI"

docker exec "$SRV" bash -c '
set -euo pipefail
for i in 1 2 3; do apt-get update -qq && apt-cache policy openssh-server | grep -q "Candidate: [0-9]" && break; sleep 5; done
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq openssh-server openssh-sftp-server qemu-utils >/dev/null
mkdir -p /run/sshd /root/.ssh /srv/hx; chmod 700 /root/.ssh
printf "PermitRootLogin prohibit-password\nPasswordAuthentication no\n" > /etc/ssh/sshd_config.d/hx.conf
ssh-keygen -A >/dev/null
/usr/sbin/sshd'
SRV_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$SRV")

docker exec "$CLI" bash -c '
set -euo pipefail
for i in 1 2 3; do apt-get update -qq && apt-cache policy sshfs | grep -q "Candidate: [0-9]" && break; sleep 5; done
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq sshfs fuse3 openssh-client qemu-utils qemu-system-x86 \
  iproute2 curl python3 libvirt0 >/dev/null
mkdir -p /root/.ssh && chmod 700 /root/.ssh
ssh-keygen -q -t ed25519 -N "" -f /root/.ssh/id_ed25519'
docker exec "$CLI" cat /root/.ssh/id_ed25519.pub | docker exec -i "$SRV" sh -c 'cat >> /root/.ssh/authorized_keys; chmod 600 /root/.ssh/authorized_keys'

# installed like the package does it
docker exec "$CLI" mkdir -p /usr/libexec/horcrux/storage-plugins
docker cp "$PLUGIN" "$CLI":/usr/libexec/horcrux/storage-plugins/sshfs
docker exec "$CLI" sh -c 'chown root:root /usr/libexec/horcrux/storage-plugins/sshfs && chmod 0755 /usr/libexec/horcrux/storage-plugins /usr/libexec/horcrux/storage-plugins/sshfs'
docker cp "$BIN" "$CLI":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/storage_plugin_e2e.py" "$CLI":/tmp/storage_plugin_e2e.py

srv_has() { docker exec "$SRV" test "$@"; }

docker exec "$CLI" bash -c '
set -euo pipefail
mkdir -p /tmp/dev && cd /tmp/dev
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_HTTP_REDIRECT_PORT=0 HORCRUX_DATA_DIR=/tmp/dev \
HORCRUX_DATABASE_URL=sqlite:///tmp/dev/horcrux.db HORCRUX_SSL_DIR=/tmp/dev/ssl \
HORCRUX_VM_STORAGE=/tmp/dev/vms HORCRUX_QEMU_RUN_DIR=/tmp/dev/run ADMIN_PASSWORD=admin RUST_LOG=info \
  nohup horcrux-api >> /tmp/dev/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "API did not start"; tail -50 /tmp/dev/api.log; exit 1; }'

step() {
  docker exec -e SRV_IP="$SRV_IP" "$CLI" python3 /tmp/storage_plugin_e2e.py "$@" || {
    echo "---- api.log"; docker exec "$CLI" tail -80 /tmp/dev/api.log; exit 1; }
}

step pool
srv_has -s /srv/hx/e2e-vol.qcow2 || { echo "FAIL volume file is not on the SSH server"; docker exec "$SRV" ls -la /srv/hx; exit 1; }
echo "ok - e2e-vol.qcow2 is on the SSH server's disk"
size=$(docker exec "$SRV" qemu-img info --output=json /srv/hx/e2e-vol.qcow2 | python3 -c 'import json,sys; print(json.load(sys.stdin)["virtual-size"])')
[ "$size" = $((2*1024*1024*1024)) ] || { echo "FAIL server copy is $size bytes, want 2 GiB"; exit 1; }
echo "ok - the grown volume is 2 GiB on the server"
docker exec "$SRV" qemu-img snapshot -l /srv/hx/e2e-vol.qcow2 | grep -q before-grow || { echo "FAIL snapshot is not in the server's qcow2"; exit 1; }
echo "ok - the snapshot is inside the server's qcow2"
step volume-cleanup
srv_has ! -e /srv/hx/e2e-vol.qcow2 || { echo "FAIL volume still on the server after delete"; exit 1; }
echo "ok - deleting the volume removed it from the server"

step vm
docker exec "$SRV" sh -c 'ls /srv/hx/*.qcow2' | grep -q e2e-boot || { echo "FAIL the VM disk is not on the SSH server"; exit 1; }
echo "ok - the VM's disk lives on the SSH server"
step teardown
docker exec "$CLI" sh -c '! grep -q " fuse.sshfs " /proc/mounts' || { echo "FAIL removing the pool left the SSHFS mount"; exit 1; }
echo "ok - removing the pool unmounted the export"
echo "ok - storage plugin e2e passed"
