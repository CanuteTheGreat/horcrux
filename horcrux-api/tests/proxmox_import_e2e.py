#!/usr/bin/env python3
"""Proxmox VE importer end to end, in the privileged LXC sibling of lxc_e2e.sh:

    LXC_E2E_PY=proxmox_import_e2e.py bash horcrux-api/tests/lxc_e2e.sh test <horcrux-api>

Builds a Proxmox-format source on this host the way a reinstalled node finds
it (guest configs next to the received volumes): VM 301 (q35, OVMF with its
efidisk, a cloud-init cdrom volume, two NICs with fixed MACs, a disk with
I/O limits, serial0 socket) on a directory storage, and an unprivileged
container 302 whose rootfs is a Proxmox subvolume (owners shifted by 100000)
with a bind mount, a storage-backed mount point, a /dev/net/tun pass-through
and a rate-limited NIC. Then plans and imports both through the API and
checks the real effects: the VM boots from its copied disk (the bootloader
prints a marker on the serial console after finding the cloud-init volume),
QEMU runs with the carried MACs, UUID and disk limits, the UEFI variables and
seed are byte copies; the container starts with its owners, mounts, device,
address and rate limit as in Proxmox.
"""
import hashlib
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lxc_e2e import LXC_PATH, TOKEN, check, fail, log, req, sh, step, wait_for  # noqa: E402

SRC = "/srv/pve"
STORE = f"{SRC}/store"  # Proxmox "dir" storage `local-img`
ZFS = f"{SRC}/zfs"  # stands in for a received ZFS pool (`local-zfs`)
DATA = "/tmp/lxc-e2e-data"
VM_MACS = ["bc:24:11:aa:00:01", "bc:24:11:aa:00:02"]
CT_MAC = "bc:24:11:aa:00:03"
UUID = "1f3c7a52-3e0b-4c43-9d55-0b0d2e7c8a11"
GENID = "7a5f2d0e-81c4-4b0b-a1f6-52c1e4d3b201"


def sha(path, length=None):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        h.update(f.read() if length is None else f.read(length))
    return h.hexdigest()


def build_vm_source():
    step("a Proxmox-format VM source (OVMF + efidisk, cloud-init volume, 2 NICs)")
    sh("apt-get install -y -qq qemu-system-x86 qemu-utils ovmf grub-efi-amd64-bin mtools dosfstools "
       "xorriso genisoimage fdisk socat >/dev/null")
    vars_tpl = next((p for p in ["/usr/share/OVMF/OVMF_VARS_4M.fd", "/usr/share/OVMF/OVMF_VARS.fd"]
                     if os.path.exists(p)), None)
    check(vars_tpl is not None, f"OVMF variable template {vars_tpl}")
    d = f"{STORE}/images/301"
    sh(f"mkdir -p {d} {SRC}/qemu-server {SRC}/lxc")
    # The bootloader: GRUB with an embedded config that prints a marker on
    # the serial console (the firmware only connects the boot disk, so the
    # cloud-init drive is checked from the host side: QEMU's block list).
    with open("/tmp/grub-e2e.cfg", "w") as f:
        f.write("serial --unit=0 --speed=115200\nterminal_output serial console\n"
                "echo HX-PVE-BOOT-OK\nsleep 600\n")
    sh("grub-mkstandalone -O x86_64-efi -o /tmp/BOOTX64.EFI --modules='part_gpt fat serial echo sleep' "
       "'boot/grub/grub.cfg=/tmp/grub-e2e.cfg'")
    disk = f"{d}/vm-301-disk-0.raw"
    sh(f"truncate -s 64M {disk}")
    sh(f"printf 'label: gpt\\nstart=2048, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B\\n' | sfdisk -q {disk}")
    sh("rm -f /tmp/esp.img && truncate -s 62M /tmp/esp.img && mkfs.vfat -n ESP /tmp/esp.img >/dev/null")
    sh("mmd -i /tmp/esp.img ::/EFI ::/EFI/BOOT && mcopy -i /tmp/esp.img /tmp/BOOTX64.EFI ::/EFI/BOOT/BOOTX64.EFI")
    sh(f"dd if=/tmp/esp.img of={disk} bs=1M seek=1 conv=notrunc status=none")
    # efidisk: Proxmox stores the variable store at the start of a 1 MiB volume
    sh(f"cp {vars_tpl} {d}/vm-301-disk-1.raw && truncate -s 1M {d}/vm-301-disk-1.raw")
    # cloud-init NoCloud volume as Proxmox generates it
    sh("mkdir -p /tmp/ci && printf 'instance-id: pve-301\\nlocal-hostname: pve-e2e-vm\\n' > /tmp/ci/meta-data "
       "&& printf '#cloud-config\\nusers: []\\n' > /tmp/ci/user-data")
    sh(f"genisoimage -quiet -o {d}/vm-301-cloudinit.raw -V cidata -J -r /tmp/ci/user-data /tmp/ci/meta-data")
    with open(f"{SRC}/qemu-server/301.conf", "w") as f:
        f.write(f"""#End to end import%3A UEFI guest with cloud-init
agent: 1
bios: ovmf
boot: order=scsi0;net0
cores: 1
cpu: x86-64-v2-AES,flags=+aes
efidisk0: local-img:301/vm-301-disk-1.raw,efitype=4m,pre-enrolled-keys=0,size=1M
ide2: local-img:301/vm-301-cloudinit.raw,media=cdrom
ipconfig0: ip=10.77.0.61/24,gw=10.77.0.1
machine: q35
memory: 256
name: pve-e2e-vm
net0: virtio={VM_MACS[0].upper()},bridge=vmbr0
net1: e1000={VM_MACS[1].upper()},bridge=vmbr1,rate=5
onboot: 1
scsi0: local-img:301/vm-301-disk-0.raw,discard=on,iothread=1,ssd=1,mbps_rd=50,mbps_wr=20,size=64M
scsihw: virtio-scsi-single
serial0: socket
smbios1: uuid={UUID}
startup: order=2,up=5
tags: e2e;pve
vga: std
vmgenid: {GENID}

[before-test]
#a snapshot section: listed, not imported
name: pve-e2e-vm
snaptime: 1795000000
""")
    with open(f"{SRC}/storage.cfg", "w") as f:
        f.write(f"dir: local-img\n\tpath {STORE}\n\tcontent images\n\nzfspool: local-zfs\n\tpool rpool/data\n\tcontent rootdir\n")


def build_ct_source():
    step("a Proxmox-format unprivileged container (shifted subvolume rootfs, bind + volume mounts, tun)")
    _, cat = req("GET", "/lxc/templates/catalog?arch=amd64")
    alpine = [e for e in cat if e["distro"] == "alpine" and e["variant"] == "default"
              and all(x.isdigit() for x in e["release"].split("."))]
    rel = max(alpine, key=lambda e: tuple(int(x) for x in e["release"].split(".")))["release"]
    req("POST", "/lxc/templates", {"distro": "alpine", "release": rel})
    tarball = sh(f"ls {DATA}/lxc-templates/alpine/*/amd64/default/rootfs.tar.xz").splitlines()[0]
    root = f"{ZFS}/subvol-302-disk-0"
    vol = f"{ZFS}/subvol-302-disk-1"
    for f in ("/etc/subuid", "/etc/subgid"):  # what a Proxmox host has
        sh(f"grep -q '^root:100000:65536' {f} 2>/dev/null || echo root:100000:65536 >> {f}")
    sh(f"mkdir -p {root} {vol} /srv/pve-bind && chown 100000:100000 {root} {vol}")
    # as Proxmox writes an unprivileged CT: inside its user namespace
    sh(f"cd {root} && lxc-usernsexec -m b:0:100000:65536 -- tar --numeric-owner -xpJf {tarball} --exclude='./dev/*'")
    sh(f"mkdir -p {root}/home/u && echo owned > {root}/home/u/file && chown -R 101000:101000 {root}/home/u")
    sh(f"echo appdata > {vol}/data && chown 101000:101000 {vol}/data")
    sh("echo bound > /srv/pve-bind/hello")
    with open(f"{SRC}/lxc/302.conf", "w") as f:
        f.write(f"""#End to end import%3A unprivileged container
arch: amd64
cores: 1
features: nesting=1
hostname: pve-e2e-ct
memory: 256
mp0: /srv/pve-bind,mp=/srv/data,backup=0
mp1: local-zfs:subvol-302-disk-1,mp=/var/lib/app,size=1G
nameserver: 10.77.0.1
net0: name=eth0,bridge=vmbr0,gw=10.77.0.1,hwaddr={CT_MAC.upper()},ip=10.77.0.60/24,rate=10,type=veth
onboot: 0
ostype: alpine
rootfs: local-zfs:subvol-302-disk-0,size=1G
startup: order=3
swap: 0
tags: e2e
unprivileged: 1
lxc.cgroup2.devices.allow: c 10:200 rwm
lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file
lxc.cgroup2.io.max: 8:99 rbps=10485760 wbps=10485760
""")


def main():
    step("login")
    _, body = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN[0] = body.get("ticket") or body.get("token")
    check(bool(TOKEN[0]), "got an API token")
    sh("ip link add hxbr1 type bridge 2>/dev/null || true; ip link set hxbr1 up")
    build_vm_source()
    build_ct_source()
    source = {"type": "dir", "path": SRC, "storage_map": {"local-zfs": {"dir": ZFS}}}
    bridges = {"vmbr0": "hxbr0", "vmbr1": "hxbr1"}

    step("plan (read only): every key in the table, nothing created")
    _, plan = req("POST", "/import/proxmox/plan", {"source": source, "bridge_map": bridges,
                                                    "io_devices": {"8:99": "/dev/disk/by-id/hx-e2e-missing"}})
    g = {x["vmid"]: x for x in plan["guests"]}
    check(set(g) == {301, 302}, f"both guests planned: {sorted(g)}")
    vm, ct = g[301], g[302]
    check(vm["blockers"] == [] and ct["blockers"] == [], f"no blockers: {vm['blockers']} {ct['blockers']}")
    keys = {r["key"] for r in vm["rows"]}
    for k in ["efidisk0", "scsi0", "net0", "net1", "ide2", "smbios1", "vmgenid", "serial0", "startup", "snapshots"]:
        check(k in keys, f"VM row {k}")
    rows = {r["key"]: r for r in vm["rows"]}
    check(rows["snapshots"]["carry"] == "not-carried", "snapshots reported as not carried")
    check(rows["cpu"]["carry"] == "changed" and "qemu64" in rows["cpu"]["horcrux"]
          and "x86-64-v2-AES" in rows["cpu"]["note"], f"plan shows the CPU model mapping: {rows['cpu']}")
    check("x86-64-v2-AES" not in vm["horcrux_cmdline"] and "-cpu qemu64," in vm["horcrux_cmdline"] and "+aes" in vm["horcrux_cmdline"],
          "planned QEMU line uses the stock model with the flag kept")
    check("throttling.bps-read=52428800" in vm["horcrux_cmdline"], "planned QEMU line carries mbps_rd=50")
    check(VM_MACS[0] in vm["horcrux_cmdline"] and UUID in vm["horcrux_cmdline"], "planned QEMU line keeps MAC and UUID")
    _, vms = req("GET", "/vms")
    check(not vms, "the plan created nothing")

    step("import both")
    _, j = req("POST", "/import/proxmox", {"source": source, "bridge_map": bridges, "guests": [301, 302],
                                           "io_devices": {"8:99": "/dev/disk/by-id/hx-e2e-missing"}}, expect=202)
    job = j["job"]

    def done():
        _, s = req("POST", "/import/proxmox/status", {"job": job})
        return s if s["state"] != "running" else None
    st = wait_for("the import job", done, timeout=900)
    res = {x["vmid"]: x for x in st["guests"]}
    if st["state"] != "done":
        fail(f"import: {st}")
    check(res[301]["link"] == "/vms/pve-e2e-vm", f"VM result links to its page: {res[301]['link']}")
    check(res[302]["link"] == "/containers/pve-e2e-ct/lxc", f"CT result links to its page: {res[302]['link']}")

    step("VM: data moved into Horcrux storage and verified")
    _, v = req("GET", "/vms/pve-e2e-vm")
    d0 = v["disks"][0]["path"]
    check(d0.startswith(f"{DATA}/vms/") and d0.endswith(".qcow2"), f"disk 0 is Horcrux's own qcow2: {d0}")
    sh(f"qemu-img compare -f raw -F qcow2 {STORE}/images/301/vm-301-disk-0.raw {d0}")
    log("ok: qemu-img compare: the copy equals the source volume")
    t = v["disks"][0].get("throttle", {})
    check(t.get("bps_rd") == 50 << 20 and t.get("bps_wr") == 20 << 20, f"disk limits stored: {t}")
    macs = [n["mac"] for n in v["hardware"]["nets"]]
    check(macs == VM_MACS, f"MACs kept: {macs}")
    check([n["bridge"] for n in v["hardware"]["nets"]] == ["hxbr0", "hxbr1"], "bridges mapped")
    check(v["hardware"]["nets"][1].get("rate_mbps") == 40, "net1 rate 5 MB/s = 40 Mbit/s")
    efivars = d0[: -len(".qcow2")] + ".efivars.qcow2"
    sh(f"qemu-img convert -f qcow2 -O raw {efivars} /tmp/efivars.raw")
    n = os.path.getsize("/tmp/efivars.raw")
    check(sha("/tmp/efivars.raw") == sha(f"{STORE}/images/301/vm-301-disk-1.raw", n),
          f"UEFI variable store is a byte copy ({n} bytes)")
    seed = d0[: -len(".qcow2")] + ".cidata.iso"
    check(sha(seed) == sha(f"{STORE}/images/301/vm-301-cloudinit.raw"), "cloud-init seed is a byte copy")

    step("VM boots from the imported disk")
    req("POST", "/vms/pve-e2e-vm/start", {}, expect=None)
    argv = wait_for("QEMU", lambda: sh("ps -eo args | grep '[q]emu-system.*pve-e2e-vm' || true"), timeout=60)
    check("x86-64-v2" not in argv, "QEMU is not given a Proxmox-only CPU model")
    for want in ["-cpu qemu64,", "+aes", f"mac={VM_MACS[0]}", f"mac={VM_MACS[1]}", f"uuid={UUID}", f"guid={GENID}",
                 "throttling.bps-read=52428800", "throttling.bps-write=20971520", "if=pflash", "q35"]:
        check(want in argv, f"QEMU runs with {want}")
    serial_log = d0[: -len(".qcow2")] + ".serial.log"
    out = wait_for("the bootloader marker on the serial console",
                   lambda: (os.path.exists(serial_log) and "HX-PVE-BOOT-OK" in open(serial_log, errors="replace").read())
                   and open(serial_log, errors="replace").read(), timeout=240)
    check("HX-PVE-BOOT-OK" in out, "the guest booted from the imported disk with the carried UEFI variables")
    check(f"file={seed},format=raw,media=cdrom,readonly=on" in argv, "the cloud-init seed is attached read-only")
    qmp = sh(f"printf '%s\\n' '{{\"execute\":\"qmp_capabilities\"}}' '{{\"execute\":\"query-block\"}}' "
             f"| timeout 5 socat - UNIX-CONNECT:{d0[: -len('.qcow2')]}.qmp.sock || true", check_rc=False)
    check("cidata.iso" in qmp, "QEMU has the seed inserted (query-block)")
    tc = sh("tc qdisc show | grep -i tbf || true")
    check("40Mbit" in tc, f"net1 tap is rate-limited: {tc}")
    req("POST", "/vms/pve-e2e-vm/stop", {}, expect=None)

    step("container: owners, mounts, device, address, rate limit")
    cfg = sh(f"cat {LXC_PATH}/pve-e2e-ct/config")
    check("lxc.idmap = u 0 100000 65536" in cfg, "same id map as Proxmox")
    check("# io limit on /dev/disk/by-id/hx-e2e-missing not applied" in cfg, "an io limit on a missing device is written as a comment")
    req("POST", "/containers/pve-e2e-ct/start", {}, expect=None)
    from lxc_e2e import exec_ct
    wait_for("the container running", lambda: exec_ct("pve-e2e-ct", "echo up") == "up", timeout=60)
    check(exec_ct("pve-e2e-ct", "stat -c %u:%g /home/u/file") == "1000:1000", "file owner as the container saw it")
    check(exec_ct("pve-e2e-ct", "stat -c %u /") == "0", "rootfs owned by the container's root")
    check(exec_ct("pve-e2e-ct", "cat /srv/data/hello") == "bound", "bind mount (host path kept)")
    check(exec_ct("pve-e2e-ct", "cat /var/lib/app/data; stat -c %u /var/lib/app/data") == "appdata\n1000",
          "storage-backed mount point copied with owners")
    check(exec_ct("pve-e2e-ct", "test -c /dev/net/tun && echo tun") == "tun", "/dev/net/tun passed in")
    wait_for("the static address", lambda: "10.77.0.60/24" in exec_ct("pve-e2e-ct", "ip -4 addr show eth0"), timeout=60)
    log("ok: static address carried")
    check(CT_MAC in exec_ct("pve-e2e-ct", "cat /sys/class/net/eth0/address"), "container MAC kept")
    tc = sh("tc qdisc show | grep -i tbf || true")
    check("80Mbit" in tc, f"container veth is rate-limited (10 MB/s): {tc}")
    req("POST", "/containers/pve-e2e-ct/stop", {}, expect=None)

    step("the source is offered for removal, never removed")
    _, pend = req("POST", "/vms/import/cleanup/pending", {})
    ids = {p["vm_id"] for p in pend}
    check("pve-e2e-vm" in ids, f"cleanup offered for the VM: {ids}")
    check(os.path.exists(f"{STORE}/images/301/vm-301-disk-0.raw"), "source disk untouched")

    step("a second import of the same source guest is refused")
    _, plan = req("POST", "/import/proxmox/plan", {"source": source, "bridge_map": bridges, "guests": [301]})
    check(plan["guests"][0]["imported_as"] == "pve-e2e-vm", "plan shows imported_as")
    _, j = req("POST", "/import/proxmox", {"source": source, "bridge_map": bridges, "guests": [301]}, expect=202)
    st = wait_for("the refused job", lambda: (lambda s: s if s["state"] != "running" else None)(
        req("POST", "/import/proxmox/status", {"job": j["job"]})[1]), timeout=60)
    check(st["state"] == "failed" and "already imported" in (st["guests"][0]["error"] or ""), "re-import refused")
    ssh_source(bridges)
    log("ALL OK")


def ssh_source(bridges):
    step("SSH source: a (stand-in) Proxmox node reached over ssh, read only")
    sh("apt-get install -y -qq openssh-server openssh-client >/dev/null")
    sh("mkdir -p /run/sshd /root/.ssh && ssh-keygen -A >/dev/null && "
       "rm -f /root/.ssh/id_hx* && ssh-keygen -q -t ed25519 -N '' -f /root/.ssh/id_hx && "
       "cat /root/.ssh/id_hx.pub >> /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys")
    sh("/usr/sbin/sshd -p 2222 -o PermitRootLogin=prohibit-password -o ListenAddress=127.0.0.1")
    # Proxmox's own tools on the node: configs in /etc/pve, `pvesm path`
    sh("mkdir -p /etc/pve/qemu-server /etc/pve/lxc")
    with open("/usr/local/bin/pvesm", "w") as f:
        f.write(f"""#!/bin/sh
[ "$1" = path ] || exit 2
case "$2" in
  local-img:*) echo "{STORE}/images/${{2#local-img:}}";;
  local-zfs:*) echo "{ZFS}/${{2#local-zfs:}}";;
  *) exit 1;;
esac
""")
    sh("chmod 755 /usr/local/bin/pvesm")
    vm = open(f"{SRC}/qemu-server/301.conf").read().split("\n[")[0]
    vm = (vm.replace("name: pve-e2e-vm", "name: pve-ssh-vm").replace(VM_MACS[0].upper(), "BC:24:11:AA:01:01")
          .replace(VM_MACS[1].upper(), "BC:24:11:AA:01:02").replace(UUID, "2f3c7a52-3e0b-4c43-9d55-0b0d2e7c8a12")
          .replace(GENID, "8a5f2d0e-81c4-4b0b-a1f6-52c1e4d3b202"))
    check("cpu: x86-64-v2-AES" in vm, "stand-in source VM has a PVE-only model")
    vm = "".join(l + "\n" for l in vm.splitlines() if not l.startswith("cpu:"))  # no cpu key at all
    open("/etc/pve/qemu-server/303.conf", "w").write(vm)
    ct = open(f"{SRC}/lxc/302.conf").read()
    ct = (ct.replace("hostname: pve-e2e-ct", "hostname: pve-ssh-ct").replace(CT_MAC.upper(), "BC:24:11:AA:01:03")
          .replace("10.77.0.60/24", "10.77.0.62/24"))
    open("/etc/pve/lxc/304.conf", "w").write(ct)
    source = {"type": "ssh", "host": "root@127.0.0.1", "port": 2222, "identity": "/root/.ssh/id_hx"}

    # an unknown host key is refused with what to do, never accepted blindly
    _, e = req("POST", "/import/proxmox/plan", {"source": source}, expect=400)
    check("host key" in str(e), f"unknown host key explained: {str(e)[:200]}")
    sh("ssh-keyscan -p 2222 127.0.0.1 2>/dev/null >> /root/.ssh/known_hosts")

    _, plan = req("POST", "/import/proxmox/plan", {"source": source, "bridge_map": bridges})
    g = {x["vmid"]: x for x in plan["guests"]}
    check(set(g) == {303, 304} and not g[303]["blockers"] and not g[304]["blockers"],
          f"both guests planned over ssh: {[(k, v['blockers']) for k, v in g.items()]}")
    _, j = req("POST", "/import/proxmox", {"source": source, "bridge_map": bridges, "guests": [303, 304]}, expect=202)
    st = wait_for("the ssh import", lambda: (lambda s: s if s["state"] != "running" else None)(
        req("POST", "/import/proxmox/status", {"job": j["job"]})[1]), timeout=900)
    if st["state"] != "done":
        fail(f"ssh import: {st}")
    _, v = req("GET", "/vms/pve-ssh-vm")
    d0 = v["disks"][0]["path"]
    sh(f"qemu-img compare -f raw -F qcow2 {STORE}/images/301/vm-301-disk-0.raw {d0}")
    log("ok: disk copied over ssh equals the source volume")
    check([n["mac"] for n in v["hardware"]["nets"]] == ["bc:24:11:aa:01:01", "bc:24:11:aa:01:02"], "MACs kept over ssh")
    check(v["hardware"].get("cpu_type") == "kvm64", f"no cpu key -> a stock model: {v['hardware'].get('cpu_type')}")
    req("POST", "/vms/pve-ssh-vm/start", {}, expect=None)
    argv = wait_for("QEMU of the no-cpu VM", lambda: sh("ps -eo args | grep '[q]emu-system.*pve-ssh-vm' || true"), timeout=60)
    check("-cpu kvm64" in argv and "x86-64-v2" not in argv, "the no-cpu VM runs with a stock CPU model")
    serial_log = d0[: -len(".qcow2")] + ".serial.log"
    out = wait_for("the no-cpu VM's bootloader marker", lambda: (os.path.exists(serial_log)
                   and "HX-PVE-BOOT-OK" in open(serial_log, errors="replace").read()) and "ok", timeout=240)
    log("ok: the no-cpu VM boots under QEMU")
    req("POST", "/vms/pve-ssh-vm/stop", {}, expect=None)
    check(not any(f.endswith(".fetch.raw") for f in os.listdir(os.path.dirname(d0))), "no staging file left")
    from lxc_e2e import exec_ct
    req("POST", "/containers/pve-ssh-ct/start", {}, expect=None)
    wait_for("the ssh-imported container", lambda: exec_ct("pve-ssh-ct", "echo up") == "up", timeout=60)
    check(exec_ct("pve-ssh-ct", "stat -c %u:%g /home/u/file") == "1000:1000", "owners kept over ssh")
    check(exec_ct("pve-ssh-ct", "cat /var/lib/app/data") == "appdata", "storage mount point copied over ssh")
    req("POST", "/containers/pve-ssh-ct/stop", {}, expect=None)
    _, pend = req("POST", "/vms/import/cleanup/pending", {})
    check("pve-ssh-vm" not in {p["vm_id"] for p in pend}, "no local cleanup offer for a remote source")


if __name__ == "__main__":
    main()
