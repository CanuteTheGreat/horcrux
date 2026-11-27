# usage: boot.sh MODE(file|anon)
MODE=${1:-file}; cd /work
pkill -x qemu-system-x86 2>/dev/null; sleep 1
[ -f guest.qcow2 ] || { qemu-img create -q -f qcow2 -b base.qcow2 -F qcow2 guest.qcow2 10G; }
mkdir -p seed/ds; cd seed
cat > ds/meta-data <<M
instance-id: hxvmi1
local-hostname: vmitest
M
cat > ds/user-data <<M
#cloud-config
disable_root: false
ssh_pwauth: true
chpasswd: {expire: false, list: ["root:vmitest"]}
packages: [sysbench, python3]
runcmd: [ "sed -i 's/^#*PermitRootLogin.*/PermitRootLogin yes/' /etc/ssh/sshd_config.d/* /etc/ssh/sshd_config", "systemctl restart ssh", "touch /var/lib/hxvmi-ready" ]
M
touch ds/vendor-data
(cd ds && python3 -m http.server 8099 >/dev/null 2>&1 &) ; cd /work
if [ $MODE = file ]; then
 MEM="-object memory-backend-file,id=pc.ram,size=2G,mem-path=/work/guestmem,share=on -machine q35,accel=kvm,memory-backend=pc.ram"
else
 MEM="-machine q35,accel=kvm"
fi
qemu-system-x86_64 $MEM -cpu host -smp 2 -m 2G -display none \
 -drive file=guest.qcow2,if=virtio -netdev user,id=n0,hostfwd=tcp:127.0.0.1:2222-:22 -device virtio-net-pci,netdev=n0 \
 -smbios "type=1,serial=ds=nocloud-net;s=http://10.0.2.2:8099/" \
 -qmp unix:/work/qmp.sock,server=on,wait=off -serial file:/work/serial.log -daemonize
echo started $MODE
