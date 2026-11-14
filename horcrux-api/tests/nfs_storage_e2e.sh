#!/bin/bash
# NFS storage pool end to end against a real NFSv4 server (nfs-ganesha in its own
# container, on a private docker network) and the real kernel NFS client in the
# API's container: add the pool by its `server:/export` path, create / list /
# delete a volume, check the file really lands on the server's export.
# The server must outlive the client mount: the trap unmounts first, removes the
# client, then the server (a loopback or orphaned NFS mount wedges the container).
# Usage: horcrux-api/tests/nfs_storage_e2e.sh <horcrux-api binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-ubuntu:24.04}
TAG="${HX_RUN_TAG:-$$}"
NET="horcrux-nfs-e2e-$TAG"; SRV="horcrux-nfs-srv-$TAG"; CLI="horcrux-nfs-cli-$TAG"

cleanup() {
  docker exec "$CLI" sh -c 'pkill horcrux-api; sleep 1; for m in $(awk "\$3 ~ /^nfs/ {print \$2}" /proc/mounts); do umount -f -l "$m"; done' >/dev/null 2>&1 || true
  docker rm -fv "$CLI" >/dev/null 2>&1 || true
  docker rm -fv "$SRV" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker network create "$NET" >/dev/null
# /srv/export must be a real (non-overlay) filesystem: ganesha needs file handles.
docker run -d --name "$SRV" --network "$NET" --privileged -v /srv/export --memory 1g "$IMAGE" sleep 1800 >/dev/null
docker run -d --name "$CLI" --network "$NET" --privileged --memory 1g "$IMAGE" sleep 1800 >/dev/null
docker cp "$BIN" "$CLI":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/nfs_storage_e2e.py" "$CLI":/tmp/nfs_storage_e2e.py

docker exec "$SRV" bash -c '
set -euo pipefail
apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nfs-ganesha nfs-ganesha-vfs dbus qemu-utils >/dev/null
mkdir -p /run/dbus /var/run/ganesha /srv/export; dbus-daemon --system --fork
cat > /etc/ganesha/ganesha.conf <<C
NFS_CORE_PARAM { Protocols = 4; NFS_Port = 2049; Enable_NLM = false; Enable_RQUOTA = false; }
NFSv4 { Graceless = true; }
EXPORT { Export_Id = 1; Path = /srv/export; Pseudo = /export; Access_Type = RW; Squash = No_Root_Squash; SecType = sys; FSAL { Name = VFS; } }
C
ganesha.nfsd -L /tmp/ganesha.log -f /etc/ganesha/ganesha.conf
for i in $(seq 1 30); do (echo > /dev/tcp/127.0.0.1/2049) 2>/dev/null && exit 0; sleep 1; done
echo "ganesha did not listen"; tail -20 /tmp/ganesha.log; exit 1'
SRV_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$SRV")

docker exec "$CLI" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  nfs-common qemu-utils iproute2 curl python3 libvirt0 >/dev/null'
docker exec -e SRV_IP="$SRV_IP" "$CLI" bash -c '
set -euo pipefail
mkdir -p /tmp/dev && cd /tmp/dev
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/dev \
HORCRUX_DATABASE_URL=sqlite:///tmp/dev/horcrux.db HORCRUX_SSL_DIR=/tmp/dev/ssl \
HORCRUX_VM_STORAGE=/tmp/dev/vms ADMIN_PASSWORD=admin RUST_LOG=info \
  nohup horcrux-api >> /tmp/dev/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "API did not start"; tail -50 /tmp/dev/api.log; exit 1; }
python3 /tmp/nfs_storage_e2e.py create || { echo "---- api.log"; tail -60 /tmp/dev/api.log; exit 1; }'
# the volume must be on the server's export, not only visible through the client
docker exec "$SRV" test -s /srv/export/e2e-vol.qcow2 || { echo "FAIL volume file is not on the NFS server"; docker exec "$SRV" ls -la /srv/export; exit 1; }
echo "ok - e2e-vol.qcow2 is on the server's export"
docker exec -e SRV_IP="$SRV_IP" "$CLI" python3 /tmp/nfs_storage_e2e.py delete
docker exec "$SRV" test ! -e /srv/export/e2e-vol.qcow2 || { echo "FAIL volume file still on the server after delete"; exit 1; }
echo "ok - volume file removed from the server's export"
docker exec "$CLI" bash -c '! grep -q " nfs" /proc/mounts' || { echo "FAIL removing the pool left an NFS mount behind"; exit 1; }
echo "ok - removing the pool unmounted the export"
