#!/bin/bash
# Needs the same prerequisites as lending-bench.sh. Settings: OUT, IMG.
# Stutter mitigation experiment: bulk asset stream (1 MiB, QD4) + 4 KiB QD1 foreground reads.
# Link modelled with ONE htb class shared by both directions; latency-class traffic (second NVMe-oF/TCP port 4421)
# gets strict priority over the bulk port 4420. Baseline puts the foreground on the bulk connection too.
set -u
OUT=${OUT:-/var/tmp/hx-lending-bench/out-qos}
mkdir -p "$OUT"; LOG=$OUT/log
NQ1=nqn.2026-11.org.horcrux:lb-bulk; NQ2=nqn.2026-11.org.horcrux:lb-lat
CFG=/sys/kernel/config/nvmet
IMG=${IMG:-hx-lending-bench:1}
FIO="docker run --rm --privileged --net host -v /dev:/dev -v $OUT:/out $IMG fio"
NVME="docker run --rm --privileged --net host -v /dev:/dev $IMG nvme"
say() { echo "$(date +%T) $*" | tee -a $LOG; }
cleanup() {
  say cleanup
  sudo tc qdisc del dev lo root 2>/dev/null
  for q in $NQ1 $NQ2; do $NVME disconnect -n $q >/dev/null 2>&1; done
  for p in 1 2; do [ -d $CFG/ports/$p ] && { sudo rm -f $CFG/ports/$p/subsystems/*; sudo rmdir $CFG/ports/$p; }; done
  for q in $NQ1 $NQ2; do [ -d $CFG/subsystems/$q ] && { echo 0 | sudo tee $CFG/subsystems/$q/namespaces/1/enable >/dev/null; sudo rmdir $CFG/subsystems/$q/namespaces/1; sudo rmdir $CFG/subsystems/$q; }; done
  sudo modprobe -r nvme_tcp nvme_fabrics 2>/dev/null; sudo modprobe -r nvmet_tcp nvmet 2>/dev/null; sudo modprobe -r brd 2>/dev/null
  say cleanup-done
}
trap cleanup EXIT
if lsmod | grep -q '^brd\|^nvmet\|^nvme_tcp'; then say "modules already loaded: abort"; trap - EXIT; exit 3; fi
sudo modprobe brd rd_nr=1 rd_size=4194304; sudo modprobe nvmet; sudo modprobe nvmet_tcp; sudo modprobe nvme_tcp
mountpoint -q /sys/kernel/config || sudo mount -t configfs none /sys/kernel/config
mk() { # nqn port
  sudo mkdir $CFG/subsystems/$1; echo 1 | sudo tee $CFG/subsystems/$1/attr_allow_any_host >/dev/null
  sudo mkdir $CFG/subsystems/$1/namespaces/1; echo /dev/ram0 | sudo tee $CFG/subsystems/$1/namespaces/1/device_path >/dev/null
  echo 1 | sudo tee $CFG/subsystems/$1/namespaces/1/enable >/dev/null
  sudo mkdir $CFG/ports/$3; echo tcp | sudo tee $CFG/ports/$3/addr_trtype >/dev/null; echo ipv4 | sudo tee $CFG/ports/$3/addr_adrfam >/dev/null
  echo 127.0.0.1 | sudo tee $CFG/ports/$3/addr_traddr >/dev/null; echo $2 | sudo tee $CFG/ports/$3/addr_trsvcid >/dev/null
  sudo ln -s $CFG/subsystems/$1 $CFG/ports/$3/subsystems/$1
}
mk $NQ1 4420 1; mk $NQ2 4421 2
$NVME connect -t tcp -a 127.0.0.1 -s 4420 -n $NQ1 >>$LOG 2>&1
$NVME connect -t tcp -a 127.0.0.1 -s 4421 -n $NQ2 >>$LOG 2>&1
sleep 2
dev() { for c in /sys/class/nvme-subsystem/nvme-subsys*; do [ "$(cat $c/subsysnqn)" = "$1" ] && echo /dev/$(ls $c | grep -m1 "^nvme[0-9]*n[0-9]*$"); done; }
BULK=$(dev $NQ1); LAT=$(dev $NQ2); say "bulk=$BULK lat=$LAT"
[ -b "$BULK" ] && [ -b "$LAT" ] || { say missing; exit 4; }
$FIO --name=fill --filename=/dev/ram0 --rw=write --bs=1m --direct=1 --size=100% >/dev/null 2>&1

shape() { # rate_mbit rtt_us
  sudo tc qdisc del dev lo root 2>/dev/null
  local d=$(( $2 / 2 )) r=$1
  sudo tc qdisc add dev lo root handle 1: htb default 20
  sudo tc class add dev lo parent 1: classid 1:1 htb rate ${r}mbit ceil ${r}mbit burst 256k cburst 256k
  sudo tc class add dev lo parent 1:1 classid 1:10 htb rate $((r/10))mbit ceil ${r}mbit prio 0 burst 256k cburst 256k
  sudo tc class add dev lo parent 1:1 classid 1:20 htb rate $((r*9/10))mbit ceil ${r}mbit prio 1 burst 256k cburst 256k
  sudo tc qdisc add dev lo parent 1:10 handle 10: netem delay ${d}us limit 200000
  sudo tc qdisc add dev lo parent 1:20 handle 20: netem delay ${d}us limit 200000
  for m in "dport 4421" "sport 4421"; do sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip $m 0xffff flowid 1:10; done
  for m in "dport 4420" "sport 4420"; do sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip $m 0xffff flowid 1:20; done
}
stut() { # name fgdev
  $FIO --name=bg --filename=$BULK --direct=1 --ioengine=libaio --rw=randread --bs=1m --iodepth=4 --runtime=20 --time_based \
       --name=fg --filename=$2 --direct=1 --ioengine=libaio --rw=randread --bs=4k --iodepth=1 --runtime=20 --time_based --lat_percentiles=1 --percentile_list=50:90:99:99.9:99.99 --output-format=json --output=/out/$1.json
}
PROFILES=${PROFILES:-"10GbE:10000:100 2.5GbE:2500:150 1GbE:1000:200"}
for prof in $PROFILES; do
  IFS=: read -r n r t <<<"$prof"; set -- "$n" "$r" "$t"; shape $2 $3; say "== $1"
  stut "$1-shared" $BULK; say "done $1-shared"
  stut "$1-prio" $LAT; say "done $1-prio"
done
say ALLDONE
