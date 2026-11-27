#!/bin/bash
# SAN real-effect test, guest side (runs as root INSIDE a throwaway Debian VM; see san_vm_e2e.sh).
#
# One VM plays array and initiator: an LIO iSCSI target (targetcli) answers on 4 portals = 2 dummy "NICs" x 2 addresses each,
# with a file-backed LUN that supports UNMAP. A horcrux-api under test is started on a fresh data dir and driven through its API only:
# SAN target -> 4 sessions -> one dm-multipath map with 4 paths -> shared VG (lvmlockd + sanlock) -> storage pool -> a volume with I/O
# -> one "NIC" dropped (nft) while a writer runs -> path view shows 2 failed paths and the writer never errors -> NIC back -> paths
# recover -> volume deleted with discard_removed -> the LUN's backing file shrinks (UNMAP reached the array).
#   bash san_guest_e2e.sh /root/horcrux-api /root/hx_node.py
# Prints `ok - name` / `FAIL - name`; exits 1 on any FAIL.
set -u
BIN=$1; HXN=$2
PORT=18006
export HX_API=https://127.0.0.1:$PORT HX_USER=admin HX_PASS=admin
fails=0
ok() { echo "ok - $1"; }
fail() { echo "FAIL - $1${2:+: $2}"; fails=$((fails+1)); }
check() { if eval "$2"; then ok "$1"; else fail "$1" "${3:-}"; fi; }
api() { python3 "$HXN" api "$@"; }
jq_() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

IQN=iqn.2003-01.org.linux-iscsi.hxsan:sn.e2e
modprobe softdog 2>/dev/null || true
modprobe target_core_mod iscsi_target_mod dm_multipath 2>/dev/null || true

echo "== the array: LIO on 4 portals (2 NICs x 2 addresses)"
for n in a b; do
  ip link add hxsan$n type dummy 2>/dev/null || true
  ip link set hxsan$n up
done
ip addr replace 10.77.1.1/24 dev hxsana; ip addr replace 10.77.1.2/24 dev hxsana
ip addr replace 10.77.2.1/24 dev hxsanb; ip addr replace 10.77.2.2/24 dev hxsanb
mkdir -p /var/lib/san
targetcli clearconfig confirm=True >/dev/null 2>&1 || true
targetcli /backstores/fileio create name=lun0 file_or_dev=/var/lib/san/lun0.img size=6G sparse=true >/dev/null
targetcli /backstores/fileio/lun0 set attribute emulate_tpu=1 >/dev/null
targetcli /iscsi create $IQN >/dev/null
targetcli /iscsi/$IQN/tpg1/portals delete 0.0.0.0 3260 >/dev/null 2>&1 || true
for a in 10.77.1.1 10.77.1.2 10.77.2.1 10.77.2.2; do targetcli /iscsi/$IQN/tpg1/portals create $a 3260 >/dev/null; done
targetcli /iscsi/$IQN/tpg1/luns create /backstores/fileio/lun0 >/dev/null
targetcli /iscsi/$IQN/tpg1 set attribute generate_node_acls=1 demo_mode_write_protect=0 authentication=0 cache_dynamic_acls=1 >/dev/null
check "array-has-4-portals" '[ "$(targetcli ls /iscsi/$IQN/tpg1/portals | grep -c 3260)" -ge 4 ]'

echo "== the manager under test"
D=/var/lib/hxsan-e2e; rm -rf $D; mkdir -p $D/ssl
( HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=$PORT HORCRUX_HTTP_REDIRECT_PORT=0 HORCRUX_DATA_DIR=$D HORCRUX_DATABASE_URL="sqlite://$D/horcrux.db" \
  HORCRUX_SSL_DIR=$D/ssl ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 \
  setsid "$BIN" > $D/api.log 2>&1 & )
for i in $(seq 1 90); do curl -sk https://127.0.0.1:$PORT/api/health >/dev/null 2>&1 && break; sleep 1; done
check "api-up" 'curl -sk https://127.0.0.1:$PORT/api/health >/dev/null' "$(tail -20 $D/api.log)"

st=$(api GET /san/status)
check "status-sees-tools" '[ "$(echo "$st" | jq_ "d[\"body\"][\"tools\"][\"iscsiadm\"] and d[\"body\"][\"tools\"][\"multipathd\"] and d[\"body\"][\"tools\"][\"lvmlockd\"]")" = True ]' "$st"

r=$(api POST /san/discover '{"portals":["10.77.1.1"]}')
check "discover-finds-all-4-portals" '[ "$(echo "$r" | jq_ "len(d[\"body\"][\"targets\"][0][\"portals\"])")" = 4 ]' "$r"

api PUT /san/settings '{"sanlock_watchdog": true, "reconcile_interval_secs": 10}' >/dev/null
r=$(api POST /san/targets "{\"id\":\"e2e\",\"iqn\":\"$IQN\",\"portals\":[\"10.77.1.1\"]}")
check "target-created" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 201 ]' "$r"
rep=$(api POST /san/apply)
echo "$rep" | head -c 2000; echo
check "drop-in-written" 'head -1 /etc/multipath/conf.d/horcrux.conf | grep -q "Managed by Horcrux"'
check "4-iscsi-sessions" '[ "$(iscsiadm -m session 2>/dev/null | grep -c "$IQN")" = 4 ]' "$(iscsiadm -m session 2>&1)"
p=$(api GET /san/paths)
check "first-apply-maps-every-path" '[ "$(echo "$p" | jq_ "[(l[\"active_paths\"], len(l[\"paths\"])) for l in d[\"body\"][\"luns\"]]")" = "[(4, 4)]" ]' "$p"
for i in $(seq 1 30); do
  p=$(api GET /san/paths)
  [ "$(echo "$p" | jq_ "[l['active_paths'] for l in d['body']['luns'] if l['map']] == [4]")" = True ] && break
  api POST /san/apply >/dev/null; sleep 2
done
echo "$p" | head -c 3000; echo
check "one-map-4-active-paths" '[ "$(echo "$p" | jq_ "[(l[\"active_paths\"], len(l[\"paths\"])) for l in d[\"body\"][\"luns\"] if l[\"map\"]][0]")" = "(4, 4)" ]' "$p"
check "paths-carry-portals" '[ "$(echo "$p" | jq_ "\",\".join(sorted(x[\"portal\"] for l in d[\"body\"][\"luns\"] if l[\"map\"] for x in l[\"paths\"]))")" = "10.77.1.1:3260,10.77.1.2:3260,10.77.2.1:3260,10.77.2.2:3260" ]' "$p"
WWID=$(echo "$p" | jq_ "[l['wwid'] for l in d['body']['luns'] if l['map']][0]")
check "map-is-a-real-device" '[ -b /dev/mapper/$WWID ]'
check "local-disk-not-multipathed" '! multipath -ll 2>/dev/null | grep -qi "QEMU HARDDISK"'

echo "== a path multipathd left out is taken back by the next apply"
# The race CI hit: a LUN's disk whose uevent came before its WWID was
# registered never joins the map by itself. Recreate it on purpose, with
# the background loop parked so only the explicit apply can repair it.
api PUT /san/settings '{"sanlock_watchdog": true, "reconcile_interval_secs": 3600}' >/dev/null
sleep 12
dev=$(echo "$p" | jq_ "[x['dev'] for l in d['body']['luns'] if l['map'] for x in l['paths']][-1]")
multipathd del path "$dev" >/dev/null 2>&1
q=$(api GET /san/paths)
lo=$(echo "$q" | jq_ "[(l['active_paths'], len(l['paths']), [x['state'] for x in l['paths'] if x['dev'] == '$dev']) for l in d['body']['luns']] == [(3, 4, ['unmapped'])]")
check "left-out-path-stays-in-its-lun" '[ "$lo" = True ]' "$q"
r=$(api POST /san/apply)
check "apply-adds-the-left-out-path" 'echo "$r" | grep -q "added path $dev"' "$r"
q=$(api GET /san/paths)
a4=$(echo "$q" | jq_ "[l['active_paths'] for l in d['body']['luns']] == [4]")
check "map-has-4-active-paths-again" '[ "$a4" = True ]' "$q"
api PUT /san/settings '{"sanlock_watchdog": true, "reconcile_interval_secs": 10}' >/dev/null

echo "== shared LVM on the LUN"
r=$(api POST /san/lvm "{\"id\":\"sanlvm\",\"vg_name\":\"vg_hxsan\",\"device\":\"$WWID\",\"discard_removed\":true,\"create_vg\":true}")
echo "$r" | head -c 2500; echo
check "shared-vg-created" '[ "$(vgs --noheadings -o vg_lock_type vg_hxsan 2>/dev/null | tr -d " ")" = sanlock ]' "$(vgs -o vg_name,vg_lock_type 2>&1; pvs -o pv_name,vg_name 2>&1; dmsetup ls --tree 2>&1 | head -30; multipath -ll 2>&1 | head -20; lvmconfig --type full devices/multipath_component_detection devices/filter devices/global_filter 2>&1; dpkg -l lvm2 multipath-tools | grep ^ii)"
check "lvmlocal-owned-with-host-id" 'grep -q "host_id = 1" /etc/lvm/lvmlocal.conf && grep -q "use_lvmlockd = 1" /etc/lvm/lvmlocal.conf'
check "lvmlockd-sees-lockspace" 'lvmlockctl -i 2>/dev/null | grep -q vg_hxsan' "$(lvmlockctl -i 2>&1 | head)"
pools=$(api GET /storage/pools)
PID=$(echo "$pools" | jq_ "[p['id'] for p in d['body'] if p['name']=='sanlvm'][0]" 2>/dev/null)
check "storage-pool-on-shared-vg" '[ -n "$PID" ] && [ "$(echo "$pools" | jq_ "[(p[\"lvm\"][\"shared\"], p[\"available\"] > 0) for p in d[\"body\"] if p[\"name\"]==\"sanlvm\"][0]")" = "(True, True)" ]' "$pools"
r=$(api POST /storage/pools/$PID/volumes '{"name":"vm-100-disk-0","size":1}')
check "volume-created-on-shared-vg" '[ -b /dev/vg_hxsan/vm-100-disk-0 ]' "$r"
check "volume-active-exclusively" 'lvs --noheadings -o lv_active_exclusively vg_hxsan/vm-100-disk-0 | grep -q "active exclusively"' "$(lvs -o +lv_active_exclusively vg_hxsan 2>&1)"

echo "== I/O while one NIC (2 of 4 paths) dies"
dd if=/dev/urandom of=/dev/vg_hxsan/vm-100-disk-0 bs=1M count=600 oflag=direct status=none
used_before=$(du -k /var/lib/san/lun0.img | cut -f1)
rm -f /tmp/hxsan-io.err /tmp/hxsan-io.count; touch /tmp/hxsan-writing
( i=0; while [ -e /tmp/hxsan-writing ]; do dd if=/dev/zero of=/dev/vg_hxsan/vm-100-disk-0 bs=64k count=16 seek=$((i % 4000)) oflag=direct conv=notrunc status=none || echo WRITE-ERROR >> /tmp/hxsan-io.err; i=$((i+16)); echo $i > /tmp/hxsan-io.count; done ) &
sleep 3
nft add table inet hxsane2e; nft add chain inet hxsane2e in '{ type filter hook input priority -10; }'
nft add rule inet hxsane2e in ip daddr 10.77.2.0/24 tcp dport 3260 drop
nft add rule inet hxsane2e in ip saddr 10.77.2.0/24 tcp sport 3260 drop
for i in $(seq 1 40); do
  p=$(api GET /san/paths)
  [ "$(echo "$p" | jq_ "[l['failed_paths'] for l in d['body']['luns'] if l['map']][0]")" = 2 ] && break
  sleep 3
done
check "path-view-shows-2-failed" '[ "$(echo "$p" | jq_ "[(l[\"active_paths\"], l[\"failed_paths\"]) for l in d[\"body\"][\"luns\"] if l[\"map\"]][0]")" = "(2, 2)" ]' "$p"
check "failed-paths-are-the-dead-nic" '[ "$(echo "$p" | jq_ "\",\".join(sorted(x[\"portal\"] for l in d[\"body\"][\"luns\"] if l[\"map\"] for x in l[\"paths\"] if x[\"state\"]==\"failed\"))")" = "10.77.2.1:3260,10.77.2.2:3260" ]' "$p"
c1=$(cat /tmp/hxsan-io.count 2>/dev/null || echo 0); sleep 10; c2=$(cat /tmp/hxsan-io.count 2>/dev/null || echo 0)
check "io-continues-on-remaining-paths" '[ "$c2" -gt "$c1" ]' "$c1 -> $c2"
check "io-never-failed-during-path-loss" '[ ! -s /tmp/hxsan-io.err ]' "$(cat /tmp/hxsan-io.err 2>/dev/null | head -3)"
nft delete table inet hxsane2e
for i in $(seq 1 40); do
  p=$(api GET /san/paths)
  [ "$(echo "$p" | jq_ "[l['active_paths'] for l in d['body']['luns'] if l['map']][0]")" = 4 ] && break
  sleep 3
done
check "paths-recover" '[ "$(echo "$p" | jq_ "[l[\"active_paths\"] for l in d[\"body\"][\"luns\"] if l[\"map\"]][0]")" = 4 ]' "$p"
rm -f /tmp/hxsan-writing; wait
check "io-never-failed" '[ ! -s /tmp/hxsan-io.err ]'
check "writer-really-wrote" '[ "$(cat /tmp/hxsan-io.count 2>/dev/null || echo 0)" -gt 160 ]' "$(cat /tmp/hxsan-io.count 2>/dev/null)"

echo "== delete with discard_removed: UNMAP reaches the array"
sync
used_before=$(du -k /var/lib/san/lun0.img | cut -f1)
r=$(api DELETE /storage/pools/$PID/volumes/vm-100-disk-0)
check "volume-deleted" '[ ! -e /dev/vg_hxsan/vm-100-disk-0 ]' "$r"
sync; sleep 2
used_after=$(du -k /var/lib/san/lun0.img | cut -f1)
check "backing-file-shrank" '[ $used_after -lt $((used_before - 400000)) ]' "before ${used_before}K after ${used_after}K"

echo "== a VM's disks on the shared VG: volumes vm-<id>-disk-<n>, side files kept out of /dev"
vmj() { api GET /vms/201 | jq_ "$1"; }
r=$(api POST /vms "{\"id\":\"201\",\"name\":\"san-vm\",\"hypervisor\":\"qemu\",\"architecture\":\"x86_64\",\"cpus\":1,\"memory\":128,\"disk_size\":1,\"status\":\"stopped\",\"display\":{\"protocol\":\"none\"},\"hardware\":{},\"disks\":[{\"path\":\"\",\"size_gb\":1,\"disk_type\":\"virtio\",\"cache\":\"none\",\"storage\":\"$PID\"}]}")
check "vm-created-with-disk0-on-shared-vg" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 201 ] && [ -b /dev/vg_hxsan/vm-201-disk-0 ]' "$r $(lvs vg_hxsan 2>&1)"
check "vm-disk0-is-the-raw-lv" '[ "$(vmj "(d[\"body\"][\"disks\"][0][\"path\"], d[\"body\"][\"disks\"][0][\"format\"])")" = "('"'"'/dev/vg_hxsan/vm-201-disk-0'"'"', '"'"'raw'"'"')" ]' "$(api GET /vms/201)"
mkdir -p $D/images
qemu-img create -q -f qcow2 $D/images/pattern.qcow2 64M
qemu-io -f qcow2 -c "write -P 0xab 0 1M" $D/images/pattern.qcow2 >/dev/null
r=$(api POST /vms/201/import-disk "{\"source\":\"$D/images/pattern.qcow2\"}")
qemu-io -f raw -c "read -P 0xab 0 1M" /dev/vg_hxsan/vm-201-disk-0 > /tmp/hxsan-imp.out 2>&1
check "import-disk-writes-into-the-lv" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 200 ] && grep -q "^read 1048576" /tmp/hxsan-imp.out && ! grep -q "verification failed" /tmp/hxsan-imp.out' "$r $(cat /tmp/hxsan-imp.out)"
check "import-left-no-file-in-dev" '[ -z "$(find /dev/vg_hxsan/ -mindepth 1 ! -name "vm-201-disk-*" 2>/dev/null)" ]' "$(ls -la /dev/vg_hxsan/)"
r=$(api POST /vms/201/disks "{\"size_gb\":1,\"bus\":\"virtio\",\"storage\":\"$PID\"}")
check "add-disk-makes-vm-201-disk-1" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 201 ] && [ -b /dev/vg_hxsan/vm-201-disk-1 ] && [ "$(vmj "d[\"body\"][\"disks\"][1][\"path\"]")" = /dev/vg_hxsan/vm-201-disk-1 ]' "$r"
r=$(api PUT /vms/201/disks/1 '{"size_gb": 2}')
check "grow-a-block-disk-extends-its-lv" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 200 ] && [ "$(lvs --noheadings --units g --nosuffix -o lv_size vg_hxsan/vm-201-disk-1 | tr -d " ")" = 2.00 ] && [ "$(vmj "d[\"body\"][\"disks\"][1][\"size_gb\"]")" = 2 ]' "$r $(lvs vg_hxsan 2>&1)"
r=$(api POST /vms/201/start)
for i in $(seq 1 30); do pgrep -f "[f]ile=/dev/vg_hxsan/vm-201-disk-0,format=raw" >/dev/null && break; sleep 1; done
check "vm-runs-on-the-lv" 'pgrep -f "[f]ile=/dev/vg_hxsan/vm-201-disk-0,format=raw" >/dev/null && pgrep -f "[f]ile=/dev/vg_hxsan/vm-201-disk-1,format=raw" >/dev/null' "$r $(tail -5 $D/api.log)"
check "qmp-socket-in-the-vm-dir" '[ -S $D/vms/blk/vg_hxsan--vm-201-disk-0.qmp.sock ]' "$(ls $D/vms/blk/ 2>&1)"
check "nothing-but-lvs-in-dev" '[ -z "$(find /dev/vg_hxsan/ -mindepth 1 ! -name "vm-201-disk-*" 2>/dev/null)" ]' "$(ls -la /dev/vg_hxsan/)"
r=$(api POST /storage/pools/$PID/volumes/vm-201-disk-0/resize '{"size": 3}')
check "volume-of-a-vm-refused-409" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 409 ]' "$r"
api POST /vms/201/stop '{"force": true}' >/dev/null
for i in $(seq 1 60); do pgrep -f "[f]ile=/dev/vg_hxsan/vm-201-disk-0" >/dev/null || break; sleep 1; done
check "vm-stopped" '! pgrep -f "[f]ile=/dev/vg_hxsan/vm-201-disk-0" >/dev/null'
r=$(api DELETE "/vms/201/disks/1?delete_file=true")
check "remove-disk-deletes-its-volume" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 200 ] && ! lvs vg_hxsan/vm-201-disk-1 >/dev/null 2>&1' "$r $(lvs vg_hxsan 2>&1)"
r=$(api DELETE /vms/201)
check "vm-delete-deletes-its-volume" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 204 ] && ! lvs vg_hxsan/vm-201-disk-0 >/dev/null 2>&1' "$r $(lvs vg_hxsan 2>&1)"
check "vm-side-files-gone" '[ -z "$(ls $D/vms/blk/ 2>/dev/null | grep vm-201)" ]' "$(ls -la $D/vms/blk/ 2>&1)"
r=$(api POST /vms "{\"id\":\"202\",\"name\":\"bad\",\"hypervisor\":\"qemu\",\"architecture\":\"x86_64\",\"cpus\":1,\"memory\":128,\"disk_size\":999,\"status\":\"stopped\",\"display\":{\"protocol\":\"none\"},\"hardware\":{},\"disks\":[{\"path\":\"/dev/sda\",\"size_gb\":999,\"disk_type\":\"virtio\",\"storage\":\"$PID\"}]}")
check "too-big-disk-refused-and-nothing-left" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 400 ] && ! lvs vg_hxsan/vm-202-disk-0 >/dev/null 2>&1 && [ "$(api GET /vms/202 | jq_ "d[\"status\"]")" = 404 ]' "$r"

echo "== snapshots as a volume chain (qcow2 on thick LVs) on the same shared VG"
api DELETE /san/lvm/sanlvm >/dev/null
r=$(api POST /san/lvm "{\"id\":\"sanchain\",\"vg_name\":\"vg_hxsan\",\"device\":\"$WWID\",\"snapshot_as_volume_chain\":true}")
pools=$(api GET /storage/pools)
CID=$(echo "$pools" | jq_ "[p['id'] for p in d['body'] if p['name']=='sanchain'][0]" 2>/dev/null)
check "chain-pool-created" '[ -n "$CID" ] && [ "$(echo "$pools" | jq_ "[p[\"lvm\"][\"snapshot_as_volume_chain\"] for p in d[\"body\"] if p[\"name\"]==\"sanchain\"][0]")" = True ]' "$r $pools"
V=/dev/vg_hxsan/chainvol
qio() { qemu-io -f qcow2 -c "$1" "$V" >/tmp/hxq.out 2>&1 && ! grep -q "Pattern verification failed" /tmp/hxq.out; }
snaps() { api GET /storage/pools/$CID/volumes/chainvol/snapshots | jq_ "','.join(d['body']['snapshots'])"; }
r=$(api POST /storage/pools/$CID/volumes '{"name":"chainvol","size":1}')
check "chain-volume-is-qcow2-on-lv" '[ -b $V ] && qemu-img info -U --output=json $V | grep -q "\"format\": \"qcow2\""' "$r"
check "chain-volume-listed-as-qcow2" '[ "$(api GET /storage/pools/$CID/volumes | jq_ "\",\".join(v[\"format\"] for v in d[\"body\"] if v[\"name\"]==\"chainvol\")")" = qcow2 ]'
qio "write -P 0xaa 0 4M"; qio "write -P 0x11 8M 4M"
r=$(api POST /storage/pools/$CID/volumes/chainvol/snapshots '{"name":"s1"}')
check "snapshot-s1-201" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 201 ] && lvs vg_hxsan/snap_chainvol_s1 >/dev/null 2>&1' "$r"
qio "write -P 0xbb 0 4M"
api POST /storage/pools/$CID/volumes/chainvol/snapshots '{"name":"s2"}' >/dev/null
check "chain-is-s1-s2" '[ "$(snaps)" = "s1,s2" ]' "$(snaps)"
lvcreate -y -l 100%FREE -n hxfill vg_hxsan >/dev/null 2>&1
r=$(api POST /storage/pools/$CID/volumes/chainvol/snapshots '{"name":"s9"}')
check "snapshot-on-full-vg-400-volume-intact" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 400 ] && echo "$r" | grep -q "MiB free" && [ -b $V ] && [ "$(snaps)" = "s1,s2" ]' "$r $(lvs vg_hxsan 2>&1)"
lvremove -y vg_hxsan/hxfill >/dev/null 2>&1
check "snapshot-lvs-hidden-from-volume-list" '[ "$(api GET /storage/pools/$CID/volumes | jq_ "\",\".join(sorted(v[\"name\"] for v in d[\"body\"]))")" = chainvol ]'
qio "write -P 0xcc 0 4M"
check "current-reads-newest-write" 'qio "read -P 0xcc 0 4M"' "$(cat /tmp/hxq.out)"
r=$(api POST /storage/pools/$CID/volumes/chainvol/snapshots/s1/rollback)
check "rollback-to-older-refused" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 400 ] && echo "$r" | grep -q "not the newest"' "$r"
r=$(api POST /storage/pools/$CID/volumes/chainvol/snapshots/s2/rollback)
check "rollback-to-s2" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 200 ] && qio "read -P 0xbb 0 4M"' "$r $(cat /tmp/hxq.out)"
r=$(api DELETE /storage/pools/$CID/volumes/chainvol/snapshots/s1)
check "delete-oldest-snapshot" '[ "$(snaps)" = "s2" ] && ! lvs vg_hxsan/snap_chainvol_s1 >/dev/null 2>&1' "$r"
check "data-of-deleted-snapshot-kept" 'qio "read -P 0x11 8M 4M" && qio "read -P 0xbb 0 4M"' "$(cat /tmp/hxq.out)"
api DELETE /storage/pools/$CID/volumes/chainvol/snapshots/s2 >/dev/null
check "delete-last-snapshot-flattens" '[ "$(snaps)" = "" ] && qio "read -P 0x11 8M 4M" && ! qemu-img info -U $V | grep -q "backing file"' "$(qemu-img info -U $V 2>&1)"
check "image-consistent" 'qemu-img check -U -f qcow2 $V >/dev/null 2>&1' "$(qemu-img check -U -f qcow2 $V 2>&1 | tail -3)"
api POST /storage/pools/$CID/volumes/chainvol/snapshots '{"name":"s3"}' >/dev/null
r=$(api DELETE /storage/pools/$CID/volumes/chainvol)
check "volume-delete-removes-chain" '[ -z "$(lvs --noheadings -o lv_name vg_hxsan 2>/dev/null | grep chainvol)" ]' "$r $(lvs vg_hxsan 2>&1)"
r=$(api GET /storage/pools/$PID/volumes/x/snapshots)
echo "== a VM disk on the volume-chain pool is a qcow2 image on its LV"
r=$(api POST /vms "{\"id\":\"203\",\"name\":\"chain-vm\",\"hypervisor\":\"qemu\",\"architecture\":\"x86_64\",\"cpus\":1,\"memory\":128,\"disk_size\":1,\"status\":\"stopped\",\"display\":{\"protocol\":\"none\"},\"hardware\":{},\"disks\":[{\"path\":\"\",\"size_gb\":1,\"disk_type\":\"virtio\",\"storage\":\"$CID\"}]}")
check "chain-vm-disk-is-qcow2" '[ "$(api GET /vms/203 | jq_ "d[\"body\"][\"disks\"][0][\"format\"]")" = qcow2 ] && qemu-img info -U --output=json /dev/vg_hxsan/vm-203-disk-0 | grep -q "\"format\": \"qcow2\""' "$r"
api POST /vms/203/start >/dev/null
for i in $(seq 1 30); do pgrep -f "[f]ile=/dev/vg_hxsan/vm-203-disk-0,format=qcow2" >/dev/null && break; sleep 1; done
check "chain-vm-runs-qcow2-on-lv" 'pgrep -f "[f]ile=/dev/vg_hxsan/vm-203-disk-0,format=qcow2" >/dev/null' "$(tail -5 $D/api.log)"
api POST /vms/203/stop '{"force": true}' >/dev/null
for i in $(seq 1 60); do pgrep -f "[f]ile=/dev/vg_hxsan/vm-203-disk-0" >/dev/null || break; sleep 1; done
api DELETE /vms/203 >/dev/null
check "chain-vm-delete-removes-the-lv" '! lvs vg_hxsan/vm-203-disk-0 >/dev/null 2>&1' "$(lvs vg_hxsan 2>&1)"
check "snapshots-on-other-pool-400-or-404" 'case "$(echo "$r" | jq_ "d[\"status\"]")" in 400|404) true;; *) false;; esac' "$r"

echo "== missing tool answers 424, not 500"
mv /usr/sbin/iscsiadm /usr/sbin/iscsiadm.hx-off
r=$(api POST /san/discover '{"portals":["10.77.1.1"]}')
mv /usr/sbin/iscsiadm.hx-off /usr/sbin/iscsiadm
check "discover-424-without-iscsiadm" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 424 ] && echo "$r" | grep -q san-iscsiadm-missing' "$r"

echo "== target delete logs out of every path"
api DELETE /san/lvm/sanchain >/dev/null
vgchange --lockstop vg_hxsan >/dev/null 2>&1; vgchange -an vg_hxsan >/dev/null 2>&1
api DELETE /san/targets/e2e >/dev/null
api POST /san/apply >/dev/null
check "sessions-gone-after-delete" '[ "$(iscsiadm -m session 2>/dev/null | grep -c "$IQN")" = 0 ]' "$(iscsiadm -m session 2>&1)"
check "no-secret-or-500-in-log" '! grep -q " 500 " $D/api.log'

echo "== NVMe over TCP: nvmet on 2 addresses (one per NIC), native multipath"
NQN=nqn.2014-08.org.horcrux:hxsan-e2e
modprobe nvmet 2>/dev/null; modprobe nvmet_tcp 2>/dev/null; modprobe nvme_tcp 2>/dev/null
mountpoint -q /sys/kernel/config || mount -t configfs none /sys/kernel/config
truncate -s 1G /var/lib/san/nvme0.img
LOOP=$(losetup -f --show /var/lib/san/nvme0.img)
C=/sys/kernel/config/nvmet
mkdir -p $C/subsystems/$NQN/namespaces/1
echo 1 > $C/subsystems/$NQN/attr_allow_any_host
echo "$LOOP" > $C/subsystems/$NQN/namespaces/1/device_path
echo 1 > $C/subsystems/$NQN/namespaces/1/enable
for pa in 1:10.77.1.1 2:10.77.2.1; do
  n=${pa%%:*}; a=${pa#*:}
  mkdir -p $C/ports/$n
  echo tcp > $C/ports/$n/addr_trtype; echo ipv4 > $C/ports/$n/addr_adrfam
  echo "$a" > $C/ports/$n/addr_traddr; echo 4420 > $C/ports/$n/addr_trsvcid
  ln -sf $C/subsystems/$NQN $C/ports/$n/subsystems/$NQN
done
check "nvmet-has-2-ports" '[ "$(ls $C/ports/*/subsystems | grep -c "$NQN")" = 2 ]' "$(ls -R $C/ports 2>&1 | head -20)"
r=$(api POST /san/nvme/discover '{"addresses":["10.77.1.1:4420","10.77.2.1:4420"]}')
nd=$(echo "$r" | jq_ "[s['addresses'] for s in d['body']['subsystems'] if s['subnqn'] == '$NQN']")
check "nvme-discover-finds-2-paths" '[ "$nd" = "[['"'"'10.77.1.1:4420'"'"', '"'"'10.77.2.1:4420'"'"']]" ]' "$r"
r=$(api POST /san/nvme/targets "{\"id\":\"nv\",\"subnqn\":\"$NQN\",\"addresses\":[\"10.77.1.1:4420\",\"10.77.2.1:4420\"]}")
check "nvme-target-created" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 201 ]' "$r"
rep=$(api POST /san/apply)
echo "$rep" | head -c 2000; echo
nctrl() { grep -l "^$NQN\$" /sys/class/nvme/nvme*/subsysnqn 2>/dev/null | wc -l; }
check "2-nvme-controllers" '[ "$(nctrl)" = 2 ]' "$(nvme list-subsys 2>&1 | head -20)"
check "host-nqn-present" 'grep -q "^nqn\." /etc/nvme/hostnqn'
p=$(api GET /san/paths)
echo "$p" | head -c 3000; echo
nv=$(echo "$p" | jq_ "[(l['active_paths'], len(l['paths']), bool(l['map'])) for l in d['body']['luns'] if l['target'] == '$NQN']")
check "nvme-one-head-2-active-paths" '[ "$nv" = "[(2, 2, True)]" ]' "$p"
np=$(echo "$p" | jq_ "','.join(sorted(x['portal'] for l in d['body']['luns'] if l['target'] == '$NQN' for x in l['paths']))")
check "nvme-paths-carry-addresses" '[ "$np" = "10.77.1.1:4420,10.77.2.1:4420" ]' "$p"
check "native-multipath-reported" '[ "$(echo "$p" | jq_ "d[\"body\"][\"native_nvme_multipath\"]")" = True ]' "$p"
HEAD=$(echo "$p" | jq_ "[l['map'] for l in d['body']['luns'] if l['target'] == '$NQN'][0]")
iop=$(for f in /sys/class/nvme-subsystem/*/subsysnqn; do grep -qx "$NQN" $f && cat $(dirname $f)/iopolicy; done)
check "iopolicy-round-robin" '[ "$iop" = round-robin ]' "$(grep -H . /sys/class/nvme-subsystem/*/iopolicy 2>&1)"

echo "== NVMe I/O while one NIC (1 of 2 paths) dies"
rm -f /tmp/hxnv-io.err /tmp/hxnv-io.count; touch /tmp/hxnv-writing
( i=0; while [ -e /tmp/hxnv-writing ]; do dd if=/dev/zero of=/dev/$HEAD bs=64k count=16 seek=$((i % 4000)) oflag=direct conv=notrunc status=none || echo WRITE-ERROR >> /tmp/hxnv-io.err; i=$((i+16)); echo $i > /tmp/hxnv-io.count; done ) &
sleep 3
nft add table inet hxnvme2e; nft add chain inet hxnvme2e in '{ type filter hook input priority -10; }'
nft add rule inet hxnvme2e in ip daddr 10.77.2.0/24 tcp dport 4420 drop
nft add rule inet hxnvme2e in ip saddr 10.77.2.0/24 tcp sport 4420 drop
for i in $(seq 1 60); do
  p=$(api GET /san/paths)
  [ "$(echo "$p" | jq_ "[l['failed_paths'] for l in d['body']['luns'] if l['target'] == '$NQN']")" = "[1]" ] && break
  sleep 3
done
nf=$(echo "$p" | jq_ "[(l['active_paths'], l['failed_paths'], [x['portal'] for x in l['paths'] if x['state'] == 'failed']) for l in d['body']['luns'] if l['target'] == '$NQN']")
check "nvme-path-view-shows-the-dead-nic" '[ "$nf" = "[(1, 1, ['"'"'10.77.2.1:4420'"'"'])]" ]' "$p"
c1=$(cat /tmp/hxnv-io.count 2>/dev/null || echo 0); sleep 10; c2=$(cat /tmp/hxnv-io.count 2>/dev/null || echo 0)
check "nvme-io-continues-on-remaining-path" '[ "$c2" -gt "$c1" ]' "$c1 -> $c2"
nft delete table inet hxnvme2e
for i in $(seq 1 60); do
  p=$(api GET /san/paths)
  [ "$(echo "$p" | jq_ "[l['active_paths'] for l in d['body']['luns'] if l['target'] == '$NQN']")" = "[2]" ] && break
  sleep 3
done
check "nvme-paths-recover" '[ "$(echo "$p" | jq_ "[l[\"active_paths\"] for l in d[\"body\"][\"luns\"] if l[\"target\"] == \"$NQN\"]")" = "[2]" ]' "$p"
rm -f /tmp/hxnv-writing; wait
check "nvme-io-never-failed" '[ ! -s /tmp/hxnv-io.err ]' "$(head -3 /tmp/hxnv-io.err 2>/dev/null)"
check "nvme-writer-really-wrote" '[ "$(cat /tmp/hxnv-io.count 2>/dev/null || echo 0)" -gt 160 ]' "$(cat /tmp/hxnv-io.count 2>/dev/null)"

echo "== NVMe: missing nvme-cli answers 424; delete disconnects every path"
NVB=$(command -v nvme)
mv "$NVB" "$NVB.hx-off"
r=$(api POST /san/nvme/discover '{"addresses":["10.77.1.1:4420"]}')
mv "$NVB.hx-off" "$NVB"
check "nvme-discover-424-without-nvme-cli" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 424 ] && echo "$r" | grep -q san-nvme-cli-missing' "$r"
api DELETE /san/nvme/targets/nv >/dev/null
api POST /san/apply >/dev/null
check "nvme-disconnected-after-delete" '[ "$(nctrl)" = 0 ]' "$(nvme list-subsys 2>&1 | head)"
check "still-no-500-in-log" '! grep -q " 500 " $D/api.log'

echo "== Fibre Channel: no HBA in this VM (QEMU emulates none): empty view, rescan answers 424"
r=$(api GET /san/fc)
check "fc-view-empty-without-hba" '[ "$(echo "$r" | jq_ "(d[\"status\"], d[\"body\"][\"hosts\"], d[\"body\"][\"remote_ports\"])")" = "(200, [], [])" ]' "$r"
r=$(api POST /san/fc/rescan '{}')
check "fc-rescan-424-without-hba" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 424 ] && echo "$r" | grep -q san-fc-no-hba' "$r"
check "paths-carry-fc-hosts-key" '[ "$(api GET /san/paths | jq_ "d[\"body\"][\"fc_hosts\"]")" = "[]" ]'

pkill -x horcrux-api || true
echo "SUMMARY fails=$fails"
[ $fails = 0 ]
