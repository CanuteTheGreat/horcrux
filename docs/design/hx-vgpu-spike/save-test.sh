#!/bin/bash
# save-test.sh MODE (container): QMP migrate-to-file, kill QEMU, restore with -incoming, verify guest continues.
. /w/lib.sh
MODE=${1:-2d}
Q() { python3 /w/qmp.py /w/qmp.sock "$@"; }
cat > /w/tick.sh <<'EOF'
#!/bin/bash
i=0; while :; do i=$((i+1)); echo $i > /root/counter; sleep 0.1; done
EOF
cat > /w/probe.sh <<'EOF'
#!/bin/bash
a=$(cat /root/counter); sleep 2; b=$(cat /root/counter); echo "counter=$a then=$b guest_uptime=$(cut -d' ' -f1 /proc/uptime)"
EOF
scp -q -i /w/guestkey -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -P 22722 /w/tick.sh /w/probe.sh root@127.0.0.1:/root/
$G 'chmod +x /root/tick.sh /root/probe.sh; pkill -x tick.sh; (setsid /root/tick.sh >/dev/null 2>&1 &); sleep 1; /root/probe.sh' | sed 's/^/before: /'
rm -f /w/save-$MODE.bin
t0=$(date +%s.%N)
Q '{"execute":"stop"}' >/dev/null
Q '{"execute":"migrate","arguments":{"uri":"file:/w/save-'$MODE'.bin"}}'
s=""
for i in $(seq 1 240); do s=$(Q '{"execute":"query-migrate"}'); echo "$s" | grep -qE '"status": "(completed|failed)"' && break; sleep 0.5; done
t1=$(date +%s.%N)
echo "$s" | grep -oE '"status": "[a-z]+"' | head -1
[ -s /w/save-$MODE.bin ] || { echo "SAVE FAILED"; Q '{"execute":"cont"}'; exit 1; }
echo "save: $(echo "$t1-$t0" | bc) s, $(du -h /w/save-$MODE.bin | cut -f1)"
Q '{"execute":"quit"}' >/dev/null 2>&1; sleep 2
/w/run-vm.sh $MODE -incoming defer >/dev/null
sleep 1
t2=$(date +%s.%N)
Q '{"execute":"migrate-incoming","arguments":{"uri":"file:/w/save-'$MODE'.bin"}}' >/dev/null
for i in $(seq 1 240); do s=$(Q '{"execute":"query-status"}'); echo "$s" | grep -q '"status": "paused"' && break; sleep 0.5; done
Q '{"execute":"cont"}' >/dev/null
t3=$(date +%s.%N)
echo "restore: $(echo "$t3-$t2" | bc) s"
waitssh 30 && $G /root/probe.sh | sed 's/^/after: /'
