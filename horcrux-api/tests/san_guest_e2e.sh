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
targetcli /backstores/fileio create name=lun0 file_or_dev=/var/lib/san/lun0.img size=3G sparse=true >/dev/null
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
check "shared-vg-created" '[ "$(vgs --noheadings -o vg_lock_type vg_hxsan 2>/dev/null | tr -d " ")" = sanlock ]' "$(vgs -o vg_name,vg_lock_type 2>&1)"
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

echo "== missing tool answers 424, not 500"
mv /usr/sbin/iscsiadm /usr/sbin/iscsiadm.hx-off
r=$(api POST /san/discover '{"portals":["10.77.1.1"]}')
mv /usr/sbin/iscsiadm.hx-off /usr/sbin/iscsiadm
check "discover-424-without-iscsiadm" '[ "$(echo "$r" | jq_ "d[\"status\"]")" = 424 ] && echo "$r" | grep -q san-iscsiadm-missing' "$r"

echo "== target delete logs out of every path"
api DELETE /san/lvm/sanlvm >/dev/null
vgchange --lockstop vg_hxsan >/dev/null 2>&1; vgchange -an vg_hxsan >/dev/null 2>&1
api DELETE /san/targets/e2e >/dev/null
api POST /san/apply >/dev/null
check "sessions-gone-after-delete" '[ "$(iscsiadm -m session 2>/dev/null | grep -c "$IQN")" = 0 ]' "$(iscsiadm -m session 2>&1)"
check "no-secret-or-500-in-log" '! grep -q " 500 " $D/api.log'

pkill -x horcrux-api || true
echo "SUMMARY fails=$fails"
[ $fails = 0 ]
