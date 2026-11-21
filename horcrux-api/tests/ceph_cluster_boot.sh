#!/bin/bash
# Bootstraps a single-node Ceph cluster (1 mon, 1 mgr, 1 bluestore OSD on a 3 GiB
# sparse file, no auth, size-1 pools) inside a container, and creates the RADOS pool
# "hxpool". Used by ceph_storage_e2e.sh; needs no devices, kernel modules or network.
set -euo pipefail
# Name the failing command and keep daemon logs in the CI log (run 18163: bare "cluster did not come up").
trap 'rc=$?; echo "ceph boot FAILED (rc=$rc) line $LINENO: $BASH_COMMAND"; ceph -s --connect-timeout 10 2>&1 | tail -20; for f in /var/log/ceph/*.log; do echo "--- $f"; tail -25 "$f"; done 2>/dev/null' ERR
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq && apt-get install -y -qq ceph-mon ceph-mgr ceph-osd ceph-common uuid-runtime >/dev/null
mkdir -p /var/run/ceph /var/log/ceph; chown ceph:ceph /var/run/ceph /var/log/ceph
FSID=$(uuidgen); HOST=$(hostname); IP=127.0.0.1
# The mon store lives on the CI runner's shared Docker disk. ceph-mon refuses to
# start below mon_data_avail_crit (default 5% free), and busy runners sit at
# 4-7% free (run 21328: "available: 4% 22 GiB"), so a throwaway test cluster
# that needs a few hundred MB must not judge the whole host disk by percent.
cat > /etc/ceph/ceph.conf <<C
[global]
fsid = $FSID
mon_initial_members = $HOST
mon_host = $IP
auth_cluster_required = none
auth_service_required = none
auth_client_required = none
osd_pool_default_size = 1
osd_pool_default_min_size = 1
mon_allow_pool_size_one = true
osd_crush_chooseleaf_type = 0
bluestore_block_size = 3221225472
mon_data_avail_warn = 2
mon_data_avail_crit = 1
[osd]
osd_objectstore = bluestore
# Ceph 19's OSD sends its first mon command (update_crush_device_class) while its
# MonClient still holds a zero-fsid monmap, and the mon answers "wrong fsid" (exit 250,
# CI runs 22551/23079). Send no mon command during OSD init; the script places the OSD.
osd_class_update_on_start = false
osd_crush_update_on_start = false
C
rm -f /tmp/mon.keyring /tmp/monmap; ceph-authtool --create-keyring /tmp/mon.keyring --gen-key -n mon. --cap mon 'allow *' >/dev/null
monmaptool --create --add $HOST $IP --fsid $FSID /tmp/monmap >/dev/null
mkdir -p /var/lib/ceph/mon/ceph-$HOST; chown -R ceph:ceph /var/lib/ceph /tmp/mon.keyring
runuser -u ceph -- ceph-mon --mkfs -i $HOST --monmap /tmp/monmap --keyring /tmp/mon.keyring
runuser -u ceph -- ceph-mon -i $HOST --public-addr $IP
for i in $(seq 30); do ceph -s >/dev/null 2>&1 && break; sleep 1; done
ceph mgr module ls >/dev/null 2>&1 || true
# osd
UUID=$(uuidgen); ID=$(ceph osd new $UUID)
D=/var/lib/ceph/osd/ceph-$ID; rm -rf $D; mkdir -p $D
chown ceph:ceph $D
ceph osd crush add-bucket $HOST host >/dev/null; ceph osd crush move $HOST root=default >/dev/null
ceph osd crush add osd.$ID 0.003 host=$HOST >/dev/null; ceph osd crush set-device-class hdd osd.$ID >/dev/null
echo '{"cephx_secret":"x"}' >/dev/null
ceph-osd -i $ID --mkfs --osd-uuid $UUID --setuser ceph --setgroup ceph 2>&1 | tail -3
chown -R ceph:ceph $D
# The OSD sends no mon command at init (see ceph.conf), so the start is deterministic. If it
# still exits non-zero, print its MonClient trace and let the ERR trap print the daemon logs.
rc=0; ceph-osd -i "$ID" --setuser ceph --setgroup ceph --debug-monc 10 >/dev/null 2>&1 || rc=$?
if [ "$rc" -ne 0 ]; then
  echo "ceph-osd start failed (rc=$rc); OSD MonClient trace:"
  grep -E 'monc|fsid|mon_cmd' "/var/log/ceph/ceph-osd.$ID.log" | tail -30 || true
  false # the ERR trap prints the daemon logs
fi
grep -qx ceph-osd /proc/[0-9]*/comm 2>/dev/null # no OSD daemon -> the ERR trap prints the logs
for i in $(seq 60); do ceph osd stat | grep -q '1 up' && break; sleep 1; done
ceph osd stat
mkdir -p /var/lib/ceph/mgr/ceph-x; chown ceph:ceph /var/lib/ceph/mgr/ceph-x
runuser -u ceph -- ceph-mgr -i x >/dev/null 2>&1
# pool stats (ceph df) come from the mgr
for i in $(seq 60); do ceph mgr stat 2>/dev/null | grep -q '"available": true' && break; sleep 1; done
ceph osd pool create hxpool 8 >/dev/null
# every PG clean, not just one (a partial "1 active+clean" used to end this wait early)
for i in $(seq 90); do ceph pg stat | grep -E 'active\+clean' | grep -vqE 'creating|peering|activating|unknown' && break; sleep 1; done
ceph pg stat
for i in $(seq 60); do ceph df --format json | grep -q max_avail && break; sleep 1; done
rbd pool init hxpool
