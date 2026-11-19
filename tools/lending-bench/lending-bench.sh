#!/bin/bash
# Game-like storage load over NVMe-oF/TCP on ONE host: nvmet (RAM-backed brd target) and the nvme-tcp initiator talk over the
# loopback, and the link is modelled with tc (netem rate + delay) on port 4420 only (other traffic is untouched).
# Needs: root via sudo, docker (image built from Dockerfile in this directory), kernel modules brd nvmet nvmet_tcp nvme_tcp.
# Output: $OUT/<profile>-<workload>.json plus progress lines in $OUT/log. Analyse with analyze.py.
# Settings (environment): OUT, IMG, RDKB (target size KiB), PROFILES ("name:rate_mbit:rtt_us ..." ; rate 'none' = unshaped).
set -u
OUT=${OUT:-/var/tmp/hx-lending-bench/out}
mkdir -p "$OUT"
LOG=$OUT/log
NQN=nqn.2026-11.org.horcrux:lending-bench
CFG=/sys/kernel/config/nvmet
IMG=${IMG:-hx-lending-bench:1}
RDKB=${RDKB:-8388608}
FIO="docker run --rm --privileged --net host -v /dev:/dev -v $OUT:/out $IMG fio"
NVME="docker run --rm --privileged --net host -v /dev:/dev $IMG nvme"
say() { echo "$(date +%T) $*" | tee -a $LOG; }

loaded_brd=0; loaded_nvmet=0; loaded_tcp=0
cleanup() {
  say cleanup
  sudo tc qdisc del dev lo root 2>/dev/null
  $NVME disconnect -n $NQN >/dev/null 2>&1
  if [ -d $CFG/ports/1 ]; then sudo rm -f $CFG/ports/1/subsystems/$NQN; sudo rmdir $CFG/ports/1; fi
  if [ -d $CFG/subsystems/$NQN ]; then echo 0 | sudo tee $CFG/subsystems/$NQN/namespaces/1/enable >/dev/null; sudo rmdir $CFG/subsystems/$NQN/namespaces/1; sudo rmdir $CFG/subsystems/$NQN; fi
  [ $loaded_tcp = 1 ] && sudo modprobe -r nvme_tcp nvme_fabrics 2>/dev/null
  [ $loaded_nvmet = 1 ] && sudo modprobe -r nvmet_tcp nvmet 2>/dev/null
  [ $loaded_brd = 1 ] && sudo modprobe -r brd 2>/dev/null
  say cleanup-done
}
trap cleanup EXIT

if lsmod | grep -q '^brd\|^nvmet\|^nvme_tcp'; then say "brd/nvmet/nvme_tcp already loaded: abort"; trap - EXIT; exit 3; fi
sudo modprobe brd rd_nr=1 rd_size=$RDKB && loaded_brd=1
sudo modprobe nvmet && loaded_nvmet=1
sudo modprobe nvmet_tcp
sudo modprobe nvme_tcp && loaded_tcp=1
mountpoint -q /sys/kernel/config || sudo mount -t configfs none /sys/kernel/config
sudo mkdir $CFG/subsystems/$NQN
echo 1 | sudo tee $CFG/subsystems/$NQN/attr_allow_any_host >/dev/null
sudo mkdir $CFG/subsystems/$NQN/namespaces/1
echo /dev/ram0 | sudo tee $CFG/subsystems/$NQN/namespaces/1/device_path >/dev/null
echo 1 | sudo tee $CFG/subsystems/$NQN/namespaces/1/enable >/dev/null
sudo mkdir $CFG/ports/1
echo tcp | sudo tee $CFG/ports/1/addr_trtype >/dev/null
echo ipv4 | sudo tee $CFG/ports/1/addr_adrfam >/dev/null
echo 127.0.0.1 | sudo tee $CFG/ports/1/addr_traddr >/dev/null
echo 4420 | sudo tee $CFG/ports/1/addr_trsvcid >/dev/null
sudo ln -s $CFG/subsystems/$NQN $CFG/ports/1/subsystems/$NQN
$NVME connect -t tcp -a 127.0.0.1 -s 4420 -n $NQN >>$LOG 2>&1
sleep 2
RDEV=""; for c in /sys/class/nvme-subsystem/nvme-subsys*; do [ "$(cat $c/subsysnqn)" = "$NQN" ] && RDEV=/dev/$(ls $c | grep -m1 "^nvme[0-9]*n[0-9]*$"); done
say "remote namespace device: $RDEV  (kernel $(uname -r), $(nproc) vCPU)"
[ -b "$RDEV" ] || { say "no nvme device"; exit 4; }
# fill the ramdisk once so reads hit real pages
$FIO --name=fill --filename=/dev/ram0 --rw=write --bs=1m --direct=1 --size=100% >/dev/null 2>&1
say filled

shape() { # name rate_mbit rtt_us
  sudo tc qdisc del dev lo root 2>/dev/null
  [ "$2" = none ] && return
  local d=$(( $3 / 2 ))
  sudo tc qdisc add dev lo root handle 1: prio bands 4 priomap 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
  sudo tc qdisc add dev lo parent 1:3 handle 30: netem delay ${d}us rate ${2}mbit limit 200000
  sudo tc qdisc add dev lo parent 1:4 handle 40: netem delay ${d}us rate ${2}mbit limit 200000
  sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip dport 4420 0xffff flowid 1:3
  sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport 4420 0xffff flowid 1:4
}

run_profile() { # name dev rate rtt
  local n=$1 dev=$2
  if [ "$3" = local ]; then shape x none; else shape $n $3 $4; fi
  say "== profile $n rate=$3 rtt_us=$4 dev=$dev"
  # A: level load, mixed block sizes, 2 jobs x 3 GiB
  $FIO --name=load --filename=$dev --direct=1 --ioengine=libaio --rw=randread --bssplit=4k/20:64k/30:256k/30:1m/20 --iodepth=16 --numjobs=2 --size=3g --offset_increment=3g --group_reporting --output-format=json --output=/out/$n-A.json
  # B: 4k QD1 (a frame-blocking small read)
  $FIO --name=qd1 --filename=$dev --direct=1 --ioengine=libaio --rw=randread --bs=4k --iodepth=1 --runtime=15 --time_based --lat_percentiles=1 --percentile_list=50:90:99:99.9:99.99 --output-format=json --output=/out/$n-B.json
  # C: stutter: QD1 4k foreground while a 1 MiB asset stream runs in the background
  $FIO --name=bg --filename=$dev --direct=1 --ioengine=libaio --rw=randread --bs=1m --iodepth=4 --runtime=20 --time_based \
       --name=fg --filename=$dev --direct=1 --ioengine=libaio --rw=randread --bs=4k --iodepth=1 --runtime=20 --time_based --lat_percentiles=1 --percentile_list=50:90:99:99.9:99.99 --output-format=json --output=/out/$n-C.json
  # D: 4k QD32 x4 (shader cache / many small reads)
  $FIO --name=small --filename=$dev --direct=1 --ioengine=libaio --rw=randread --bs=4k --iodepth=32 --numjobs=4 --runtime=10 --time_based --group_reporting --output-format=json --output=/out/$n-D.json
  say "done $n"
}

PROFILES=${PROFILES:-"nvmetcp-loopback:none:0 25GbE:25000:100 10GbE:10000:100 2.5GbE:2500:150 1GbE:1000:200 10GbE-rtt0.5ms:10000:500 10GbE-rtt2ms:10000:2000 1GbE-rtt2ms:1000:2000"}
run_profile local /dev/ram0 local 0
for p in $PROFILES; do IFS=: read -r n r t <<<"$p"; run_profile "$n" "$RDEV" "$r" "$t"; done
say ALLDONE
