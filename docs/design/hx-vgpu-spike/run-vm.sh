#!/bin/bash
# Start the spike guest inside the container. Usage: run-vm.sh MODE [extra qemu args]
# MODE: venus (virtio-vga-gl + venus + blob), 2d (plain virtio-vga), venus-lvp (venus on host lavapipe)
set -e
MODE=${1:-venus}; shift || true
W=/w
MEM=${MEM:-4G}
cd $W
gpu=()
case $MODE in
  venus|venus-lvp)
    gpu=(-object memory-backend-memfd,id=mem1,size=$MEM,share=on -machine q35,accel=kvm,memory-backend=mem1
         -device virtio-vga-gl,hostmem=${HOSTMEM:-2G},blob=true,venus=true
         -display egl-headless,rendernode=/dev/dri/renderD128) ;;
  2d)
    gpu=(-machine q35,accel=kvm -device virtio-vga -display none) ;;
esac
if [ "$MODE" = venus-lvp ]; then export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json; fi
if [ "$MODE" = venus ]; then export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json; fi
setsid qemu-system-x86_64 -name hxvgpu -cpu host -smp ${SMP:-4} -m $MEM "${gpu[@]}" \
  -drive file=$W/guest.qcow2,if=virtio,cache=unsafe \
  -drive file=$W/seed.iso,media=cdrom \
  -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:22722-:22 \
  -qmp unix:$W/qmp.sock,server=on,wait=off \
  -serial file:$W/serial-$MODE.log -monitor none -pidfile $W/qemu.pid "$@" </dev/null >$W/qemu-$MODE.log 2>&1 &
sleep 2; cat $W/qemu-$MODE.log
