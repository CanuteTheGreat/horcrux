#!/bin/bash
# s4-test.sh (container): guest hibernate (S4) with a live Venus Vulkan app, then resume on a fresh QEMU.
. /w/lib.sh
MODE=${1:-venus}
$G "pkill vkmark; rm -f /root/vkm.log; (setsid vkmark --winsys headless -s 640x480 --run-forever -b clear > /root/vkm.log 2>&1 &); sleep 8; tail -2 /root/vkm.log; pgrep -a vkmark"
echo "== hibernating (guest S4) at $(date +%T)"
t0=$(date +%s.%N)
$G "systemctl hibernate" || true
for i in $(seq 1 120); do [ -e /proc/$(cat /w/qemu.pid) ] || break; sleep 1; done
t1=$(date +%s.%N)
echo "== qemu exited after $(echo "$t1-$t0" | bc) s; $([ -e /proc/$(cat /w/qemu.pid) ] && echo STILL-RUNNING)"
/w/run-vm.sh $MODE >/dev/null
waitssh 120 && t2=$(date +%s.%N) && echo "== guest back after $(echo "$t2-$t1" | bc) s (incl. qemu start + resume)"
$G "uptime; journalctl -b -0 --no-pager | grep -iE 'hibernat|resume|PM: ' | tail -4; pgrep -a vkmark || echo VKMARK-GONE; sleep 5; tail -4 /root/vkm.log; dmesg | grep -iE 'virtio_gpu|virtgpu|drm' | tail -6"
