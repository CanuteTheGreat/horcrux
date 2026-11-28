#!/usr/bin/env python3
"""Proxmox "adopt in place" end to end on a REAL ZFS pool, in the privileged
LXC sibling of lxc_e2e.sh (the runner's kernel must have the zfs module):

    LXC_E2E_PY=proxmox_adopt_zfs_e2e.py bash horcrux-api/tests/lxc_e2e.sh test <horcrux-api>

Builds, on a file-backed pool, what a node that ran Proxmox VE looks like when
it boots Horcrux from a new root dataset in the same pool: a read-only
Proxmox root file system whose guest configs live only in the cluster
database (var/lib/pve-cluster/config.db, /etc/pve empty), storage.cfg
pointing at <pool>/data, and the guest volumes there:
  VM 401  UEFI guest: zvol disk (GPT + GRUB printing a marker on serial),
          efidisk zvol, cloud-init zvol, 2 NICs with fixed MACs, disk limits,
          onboot order 1
  CT 402  unprivileged container: subvol rootfs with owners shifted by
          100000, a storage mount point subvol, a bind mount, onboot order 2
  VM 403  shares VM 401's MAC and is onboot too: must NOT start at boot
  VM 404  UEFI guest like 401 but not started at boot: its carried UEFI
          variables are compared byte for byte (a running OVMF rewrites them)
Then snapshots <pool>/data, plans (read only, offline), applies with
start_onboot and checks: disks are clones of the snapshot used as raw zvols,
the VM boots with the carried UEFI variables and seed (sidecar dir), the CT
runs from its cloned subvol with the owners as before (nothing chowned),
VM 403 is not started, the Proxmox datasets are unchanged (written@snap = 0,
same guid, same bytes), and deleting the guests destroys the clones only
(= the rollback). Apply time per guest is printed (TIMING lines).

This sibling has no udev: a stand-in loop creates /dev/zdN nodes and the
/dev/zvol/<dataset> links (what the zfs udev rules do on a real host).
"""
import hashlib
import json
import os
import socket
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lxc_e2e import LXC_PATH, TOKEN, check, fail, log, req, sh, step, wait_for  # noqa: E402

# pool names are global in the runner kernel: the sibling's hostname (its
# container id) keeps parallel runs apart
POOL = f"hx-e2e-adopt-{socket.gethostname()}"
VDEV = "/srv/hx-e2e-pool.img"
ROOT = "/srv/pve-root"
DATA = "/tmp/lxc-e2e-data"
SNAP = "hx-cutover"
PARENT = f"{POOL}/horcrux"
VM_MACS = ["bc:24:11:bb:00:01", "bc:24:11:bb:00:02"]
CT_MAC = "bc:24:11:bb:00:03"
UUID = "4f3c7a52-3e0b-4c43-9d55-0b0d2e7c8a41"
GENID = "5a5f2d0e-81c4-4b0b-a1f6-52c1e4d3b241"
UDEV = []
LOOP = []


def sha_dev(path, length=None):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        h.update(f.read() if length is None else f.read(length))
    return h.hexdigest()


def zget(ds, prop):
    return sh(f"zfs get -Hp -o value {prop} {ds}").strip()


def udev_standin():
    """/dev/zdN + /dev/zvol/<name> for every zvol (no udev in the sibling)."""
    script = r"""
while :; do
  for s in /sys/block/zd*; do
    [ -e "$s/dev" ] || continue
    n=${s##*/}; mm=$(cat $s/dev)
    [ -b /dev/$n ] || mknod /dev/$n b ${mm%:*} ${mm#*:}
    name=$(/usr/lib/udev/zvol_id /dev/$n 2>/dev/null) || continue
    [ -n "$name" ] || continue
    mkdir -p "/dev/zvol/$(dirname "$name")"
    [ "$(readlink /dev/zvol/$name)" = "/dev/$n" ] || ln -sfn /dev/$n "/dev/zvol/$name"
  done
  for l in $(find /dev/zvol -type l 2>/dev/null); do [ -b "$l" ] || rm -f "$l"; done
  sleep 0.2
done
"""
    UDEV.append(subprocess.Popen(["bash", "-c", script]))


def make_pool():
    step(f"a real ZFS pool on a file vdev ({POOL})")
    check(os.path.exists("/dev/zfs"), "/dev/zfs present (zfs module on the runner)")
    sh("apt-get install -y -qq zfsutils-linux qemu-system-x86 qemu-utils ovmf grub-efi-amd64-bin mtools "
       "dosfstools genisoimage fdisk socat >/dev/null")
    log(sh("zfs version | tr '\\n' ' '"))
    udev_standin()
    sh(f"truncate -s 3G {VDEV}")
    # a loop device: ZFS refuses a file vdev on the container's overlayfs
    LOOP.append(sh(f"losetup --find --show {VDEV}").strip())
    sh(f"zpool create -o cachefile=none -O mountpoint=none -O compression=lz4 {POOL} {LOOP[0]}")
    sh(f"zfs create -o mountpoint=/{POOL}/data {POOL}/data")


def zvol(name, size, src_file):
    ds = f"{POOL}/data/{name}"
    sh(f"zfs create -s -V {size} -o volblocksize=16k {ds}")
    dev = f"/dev/zvol/{ds}"
    wait_for(f"{dev}", lambda: os.path.exists(dev), timeout=30)
    sh(f"dd if={src_file} of={dev} bs=1M conv=fsync status=none")
    return dev


def build_volumes():
    step("Proxmox guest volumes on the pool: zvols (VM) and subvols (CT)")
    vars_tpl = next((p for p in ["/usr/share/OVMF/OVMF_VARS_4M.fd", "/usr/share/OVMF/OVMF_VARS.fd"]
                     if os.path.exists(p)), None)
    check(vars_tpl is not None, f"OVMF variable template {vars_tpl}")
    with open("/tmp/grub-e2e.cfg", "w") as f:
        f.write("serial --unit=0 --speed=115200\nterminal_output serial console\n"
                "echo HX-ADOPT-BOOT-OK\nsleep 600\n")
    sh("grub-mkstandalone -O x86_64-efi -o /tmp/BOOTX64.EFI --modules='part_gpt fat serial echo sleep' "
       "'boot/grub/grub.cfg=/tmp/grub-e2e.cfg'")
    sh("truncate -s 64M /tmp/disk0.raw")
    sh("printf 'label: gpt\\nstart=2048, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B\\n' | sfdisk -q /tmp/disk0.raw")
    sh("rm -f /tmp/esp.img && truncate -s 62M /tmp/esp.img && mkfs.vfat -n ESP /tmp/esp.img >/dev/null")
    sh("mmd -i /tmp/esp.img ::/EFI ::/EFI/BOOT && mcopy -i /tmp/esp.img /tmp/BOOTX64.EFI ::/EFI/BOOT/BOOTX64.EFI")
    sh("dd if=/tmp/esp.img of=/tmp/disk0.raw bs=1M seek=1 conv=notrunc status=none")
    zvol("vm-401-disk-0", "64M", "/tmp/disk0.raw")
    sh(f"cp {vars_tpl} /tmp/efi.raw && truncate -s 1M /tmp/efi.raw")
    zvol("vm-401-disk-1", "1M", "/tmp/efi.raw")
    sh("mkdir -p /tmp/ci && printf 'instance-id: pve-401\\nlocal-hostname: adopt-vm\\n' > /tmp/ci/meta-data "
       "&& printf '#cloud-config\\nusers: []\\n' > /tmp/ci/user-data")
    sh("genisoimage -quiet -o /tmp/ci.iso -V cidata -J -r /tmp/ci/user-data /tmp/ci/meta-data && truncate -s 4M /tmp/ci.iso")
    zvol("vm-401-cloudinit", "4M", "/tmp/ci.iso")
    zvol("vm-403-disk-0", "64M", "/tmp/disk0.raw")
    zvol("vm-404-disk-0", "64M", "/tmp/disk0.raw")
    zvol("vm-404-disk-1", "1M", "/tmp/efi.raw")

    # container: rootfs subvol with Proxmox's local properties and shifted owners
    _, cat = req("GET", "/lxc/templates/catalog?arch=amd64")
    alpine = [e for e in cat if e["distro"] == "alpine" and e["variant"] == "default"
              and all(x.isdigit() for x in e["release"].split("."))]
    rel = max(alpine, key=lambda e: tuple(int(x) for x in e["release"].split(".")))["release"]
    req("POST", "/lxc/templates", {"distro": "alpine", "release": rel})
    tarball = sh(f"ls {DATA}/lxc-templates/alpine/*/amd64/default/rootfs.tar.xz").splitlines()[0]
    for f in ("/etc/subuid", "/etc/subgid"):
        sh(f"grep -q '^root:100000:65536' {f} 2>/dev/null || echo root:100000:65536 >> {f}")
    root = f"/{POOL}/data/subvol-402-disk-0"
    vol = f"/{POOL}/data/subvol-402-disk-1"
    sh(f"zfs create -o acltype=posixacl -o xattr=sa -o refquota=1G {POOL}/data/subvol-402-disk-0")
    sh(f"zfs create -o acltype=posixacl -o xattr=sa -o refquota=512M {POOL}/data/subvol-402-disk-1")
    sh(f"chown 100000:100000 {root} {vol}")
    sh(f"cd {root} && lxc-usernsexec -m b:0:100000:65536 -- tar --numeric-owner -xpJf {tarball} --exclude='./dev/*'")
    sh(f"mkdir -p {root}/home/u && echo owned > {root}/home/u/file && chown -R 101000:101000 {root}/home/u")
    sh(f"echo appdata > {vol}/data && chown 101000:101000 {vol}/data")
    sh("mkdir -p /srv/adopt-bind && echo bound > /srv/adopt-bind/hello")


VM401 = f"""agent: 1
bios: ovmf
boot: order=scsi0;net0
cores: 1
cpu: qemu64
efidisk0: local-zfs:vm-401-disk-1,efitype=4m,pre-enrolled-keys=0,size=1M
ide2: local-zfs:vm-401-cloudinit,media=cdrom
ipconfig0: ip=10.77.0.71/24,gw=10.77.0.1
machine: q35
memory: 256
name: adopt-vm
net0: virtio={VM_MACS[0].upper()},bridge=vmbr0
net1: e1000={VM_MACS[1].upper()},bridge=vmbr1,rate=5
onboot: 1
scsi0: local-zfs:vm-401-disk-0,discard=on,iothread=1,ssd=1,mbps_rd=50,mbps_wr=20,size=64M
scsihw: virtio-scsi-single
serial0: socket
smbios1: uuid={UUID}
startup: order=1,up=2
vga: std
vmgenid: {GENID}
"""

CT402 = f"""arch: amd64
cores: 1
hostname: adopt-ct
memory: 256
mp0: /srv/adopt-bind,mp=/srv/data,backup=0
mp1: local-zfs:subvol-402-disk-1,mp=/var/lib/app,size=512M
nameserver: 10.77.0.1
net0: name=eth0,bridge=vmbr0,gw=10.77.0.1,hwaddr={CT_MAC.upper()},ip=10.77.0.70/24,rate=10,type=veth
onboot: 1
ostype: alpine
rootfs: local-zfs:subvol-402-disk-0,size=1G
startup: order=2
swap: 0
unprivileged: 1
"""

VM404 = """bios: ovmf
boot: order=scsi0
cores: 1
efidisk0: local-zfs:vm-404-disk-1,efitype=4m,pre-enrolled-keys=0,size=1M
machine: q35
memory: 128
name: adopt-cold
net0: virtio=BC:24:11:BB:00:44,bridge=vmbr0
scsi0: local-zfs:vm-404-disk-0,size=64M
"""

VM403 = f"""bios: seabios
boot: order=scsi0
cores: 1
memory: 128
name: adopt-dup
net0: virtio={VM_MACS[0].upper()},bridge=vmbr0
onboot: 1
scsi0: local-zfs:vm-403-disk-0,size=64M
startup: order=5
"""


def build_root():
    step("a read-only Proxmox root: guest configs only in the cluster database")
    import sqlite3
    sh(f"zfs create -p -o mountpoint={ROOT} {POOL}/ROOT/pve-1")
    sh(f"mkdir -p {ROOT}/etc/pve {ROOT}/var/lib/pve-cluster && echo pve-e2e > {ROOT}/etc/hostname")
    db = sqlite3.connect(f"{ROOT}/var/lib/pve-cluster/config.db")
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("CREATE TABLE tree (inode INTEGER PRIMARY KEY NOT NULL, parent INTEGER NOT NULL, version "
               "INTEGER NOT NULL, writer INTEGER NOT NULL, mtime INTEGER NOT NULL, type INTEGER NOT NULL, "
               "name TEXT NOT NULL, data BLOB)")
    rows = [
        (0, 0, 8, "__version__", None),
        (2, 0, 8, "storage.cfg", f"dir: local\n\tpath /var/lib/vz\n\tcontent iso\n\nzfspool: local-zfs\n\tpool {POOL}/data\n\tcontent images,rootdir\n\tsparse 1\n"),
        (3, 0, 4, "nodes", None),
        (4, 3, 4, "pve-e2e", None),
        (5, 4, 4, "qemu-server", None),
        (6, 4, 4, "lxc", None),
        (7, 5, 8, "401.conf", VM401),
        (8, 6, 8, "402.conf", CT402),
        (9, 5, 8, "403.conf", VM403),
        (10, 5, 8, "404.conf", VM404),
    ]
    for i, p, t, n, d in rows:
        db.execute("INSERT INTO tree VALUES (?, ?, 1, 0, 0, ?, ?, ?)", (i, p, t, n, d.encode() if d else None))
    db.commit()
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    db.close()
    sh(f"zfs set readonly=on {POOL}/ROOT/pve-1")
    check(subprocess.run(["touch", f"{ROOT}/x"]).returncode != 0, "the Proxmox root is read only")
    check(os.listdir(f"{ROOT}/etc/pve") == [], "/etc/pve is empty on disk (pmxcfs is not running)")


SOURCES = ["vm-401-disk-0", "vm-401-disk-1", "vm-401-cloudinit", "vm-403-disk-0", "vm-404-disk-0", "vm-404-disk-1",
           "subvol-402-disk-0", "subvol-402-disk-1"]


def fingerprint():
    fp = {}
    for n in SOURCES:
        ds = f"{POOL}/data/{n}"
        fp[n] = {"guid": zget(ds, "guid"), "written": zget(ds, f"written@{SNAP}")}
        if n.startswith("vm-"):
            fp[n]["sha"] = sha_dev(f"/dev/zvol/{ds}")
    return fp


def main():
    step("login")
    _, body = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN[0] = body.get("ticket") or body.get("token")
    check(bool(TOKEN[0]), "got an API token")
    sh("ip link add hxbr1 type bridge 2>/dev/null || true; ip link set hxbr1 up")
    if not os.path.exists("/dev/zfs"):
        print("::warning::the runner kernel has no zfs module: adopt-in-place e2e NOT run")
        log("SKIP: no /dev/zfs")
        return
    try:
        make_pool()
        build_volumes()
        build_root()
        run()
    finally:
        for p in UDEV:
            p.kill()
        subprocess.run(f"for c in $(lxc-ls -P {LXC_PATH} 2>/dev/null); do lxc-stop -P {LXC_PATH} -n $c -k; done; "
                       f"pkill -f '[q]emu-system.*adopt-' ; sleep 1; zpool destroy -f {POOL}; "
                       f"{'losetup -d ' + LOOP[0] + ';' if LOOP else ''} rm -f {VDEV}",
                       shell=True)


def run():
    source = {"type": "adopt", "pve_root": ROOT, "snapshot": SNAP, "target_parent_dataset": PARENT}
    bridges = {"vmbr0": "hxbr0", "vmbr1": "hxbr1"}

    step("plan BEFORE the snapshot exists: refused per volume, nothing created")
    _, plan = req("POST", "/import/proxmox/plan", {"source": source, "bridge_map": bridges})
    g = {x["vmid"]: x for x in plan["guests"]}
    check(set(g) == {401, 402, 403, 404}, f"four guests read from config.db: {sorted(g)}")
    check(any("snapshot" in b and "does not exist" in b for b in g[401]["blockers"]),
          f"missing snapshot is a blocker: {g[401]['blockers']}")
    _, e = req("POST", "/import/proxmox/plan", {"source": {**source, "snapshot": None}}, expect=400)
    check("allow_no_snapshot" in str(e), "no snapshot without allow_no_snapshot is refused")

    step("the cutover snapshot (guests stopped)")
    sh(f"zfs snapshot -r {POOL}/data@{SNAP}")
    before = fingerprint()

    step("plan (read only, offline): adopt volumes, onboot guard, QEMU line on the zvols")
    _, plan = req("POST", "/import/proxmox/plan", {"source": source, "bridge_map": bridges})
    g = {x["vmid"]: x for x in plan["guests"]}
    for k in (401, 402, 403, 404):
        check(g[k]["blockers"] == [], f"guest {k} has no blockers: {g[k]['blockers']}")
    a = {v["key"]: v for v in g[401]["adopt"]}
    check(a["scsi0"]["kept"] and a["scsi0"]["dataset"] == f"{PARENT}/adopt-vm-scsi0"
          and a["scsi0"]["from_snapshot"] == f"{POOL}/data/vm-401-disk-0@{SNAP}", f"scsi0 is a clone: {a['scsi0']}")
    check(not a["efidisk0"]["kept"] and not a["ide2"]["kept"], "efidisk/cloud-init are read via temporary clones")
    ca = {v["key"]: v for v in g[402]["adopt"]}
    check(("acltype=posix" in ca["rootfs"]["command"]) and "xattr=sa" in ca["rootfs"]["command"] and f"refquota={1 << 30}" in ca["rootfs"]["command"],
          f"rootfs clone keeps Proxmox's local properties and size: {ca['rootfs']['command']}")
    check(not g[402]["ct"]["lxc"].get("disk_gb"), "no fixed-size rootfs image for an adopted container")
    rows403 = {r["key"]: r for r in g[403]["rows"]}
    check("onboot" in rows403 and "not started at boot" in rows403["onboot"]["horcrux"]
          and "401" in rows403["onboot"]["note"], f"VM 403 shares 401's MAC: not started at boot: {rows403.get('onboot')}")
    check(g[401]["autostart"]["onboot"] and not g[403]["autostart"]["onboot"], "only the first by start order keeps onboot")
    check(f"/dev/zvol/{POOL}/data/vm-401-disk-0" not in (g[401].get("horcrux_cmdline") or ""), "plan line never names a source zvol")
    check(f"file=/dev/zvol/{PARENT}/adopt-vm-scsi0,format=raw" in (g[401].get("horcrux_cmdline") or ""),
          "planned QEMU line runs the clone's zvol raw")
    check(sh(f"zfs list -H -o name -r {POOL} | grep -c horcrux || true").strip() == "0", "the plan created nothing")

    step("apply (start_onboot): clones + guests, router first")
    t0 = time.time()
    _, j = req("POST", "/import/proxmox", {"source": source, "bridge_map": bridges, "guests": [401, 402, 403, 404],
                                           "start_onboot": True}, expect=202)
    st = wait_for("the adopt job", lambda: (lambda s: s if s["state"] != "running" else None)(
        req("POST", "/import/proxmox/status", {"job": j["job"]})[1]), timeout=600)
    wall = time.time() - t0
    if st["state"] != "done":
        fail(f"adopt: {json.dumps(st)[:3000]}")
    res = {x["vmid"]: x for x in st["guests"]}
    for k in (401, 402, 403, 404):
        log(f"TIMING guest {k}: apply {res[k].get('secs', 0):.2f} s, started at +{res[k].get('started_after_secs')}")
    log(f"TIMING job wall {wall:.1f} s (includes 2 s up delay of 401 and the job poll)")
    check([x["vmid"] for x in st["guests"]] == [401, 402, 403, 404], "applied in start order (no order = last)")
    check(res[401].get("started_after_secs") is not None and res[402].get("started_after_secs") is not None,
          "onboot guests started")
    check(res[403].get("started_after_secs") is None, "VM 403 (shared MAC) not started")
    check(res[401]["started_after_secs"] < res[402]["started_after_secs"], "router (order 1) started first")

    step("VM: disks are clones used raw, sidecar files in place")
    _, v = req("GET", "/vms/adopt-vm")
    d0 = v["disks"][0]["path"]
    check(d0 == f"/dev/zvol/{PARENT}/adopt-vm-scsi0", f"disk 0 is the clone's zvol: {d0}")
    check(zget(f"{PARENT}/adopt-vm-scsi0", "origin") == f"{POOL}/data/vm-401-disk-0@{SNAP}", "origin is the snapshot")
    check(zget(f"{PARENT}/adopt-vm-scsi0", "horcrux:guest") == "adopt-vm", "clone is tagged with its guest")
    check(sh(f"zfs list -H -o name -r {PARENT} | grep -c -- '-tmp$' || true").strip() == "0", "temporary clones dropped")
    side = f"{DATA}/vms/sidecars/zvol/{PARENT}/adopt-vm-scsi0"
    cold = f"{DATA}/vms/sidecars/zvol/{PARENT}/adopt-cold-scsi0"
    sh(f"qemu-img convert -f qcow2 -O raw {cold}.efivars.qcow2 /tmp/efivars.raw")
    n = os.path.getsize("/tmp/efivars.raw")
    check(sha_dev("/tmp/efivars.raw") == sha_dev("/tmp/efi.raw", n),
          f"UEFI variables carried into the sidecar dir byte for byte ({n} bytes, VM 404 never started)")
    check(os.path.exists(f"{side}.efivars.qcow2"), "VM 401's variable store is in its sidecar dir")
    check(sha_dev(f"{side}.cidata.iso") == sha_dev("/tmp/ci.iso"), "cloud-init seed carried into the sidecar dir")
    check(v["hardware"]["nets"][0]["mac"] == VM_MACS[0] and v["hardware"]["nets"][1]["mac"] == VM_MACS[1], "MACs kept")
    argv = wait_for("QEMU of adopt-vm", lambda: sh("ps -eo args | grep '[q]emu-system.*adopt-vm' || true"), timeout=60)
    for want in [f"file={d0},format=raw", f"mac={VM_MACS[0]}", f"uuid={UUID}", f"guid={GENID}",
                 "throttling.bps-read=52428800", "if=pflash", f"{side}.efivars.qcow2"]:
        check(want in argv, f"QEMU runs with {want}")
    out = wait_for("the bootloader marker", lambda: os.path.exists(f"{side}.serial.log") and
                   "HX-ADOPT-BOOT-OK" in open(f"{side}.serial.log", errors="replace").read(), timeout=240)
    check(bool(out), "the guest booted from the cloned zvol with its carried UEFI variables")
    _, pol = req("GET", "/vms/adopt-dup")
    check(pol["disks"][0]["path"] == f"/dev/zvol/{PARENT}/adopt-dup-scsi0", "VM 403 adopted, not running")

    step("container: rootfs is the cloned subvol mounted in place, owners untouched")
    from lxc_e2e import exec_ct
    rootds = f"{PARENT}/adopt-ct-rootfs"
    check(zget(rootds, "mountpoint") == f"{LXC_PATH}/adopt-ct/rootfs" and zget(rootds, "mounted") == "yes",
          "rootfs clone mounted at the container's rootfs")
    check(zget(rootds, "acltype") in ("posix", "posixacl") and zget(rootds, "xattr") == "sa",
          "acltype/xattr carried to the clone (a clone inherits from its new parent otherwise)")
    check(zget(rootds, "refquota") == str(1 << 30), "size limit = refquota on the clone")
    wait_for("the container running", lambda: exec_ct("adopt-ct", "echo up") == "up", timeout=60)
    check(exec_ct("adopt-ct", "stat -c %u:%g /home/u/file") == "1000:1000", "file owner as the container saw it")
    check(exec_ct("adopt-ct", "cat /var/lib/app/data; stat -c %u /var/lib/app/data") == "appdata\n1000",
          "mount point clone mounted with owners")
    check(exec_ct("adopt-ct", "cat /srv/data/hello") == "bound", "bind mount kept by path")
    check(CT_MAC in exec_ct("adopt-ct", "cat /sys/class/net/eth0/address"), "container MAC kept")
    check(sh(f"stat -c %u /{POOL}/data/subvol-402-disk-0/home/u/file").strip() == "101000",
          "the Proxmox subvol still has its shifted owners (nothing chowned)")

    step("the Proxmox datasets were never written")
    exec_ct("adopt-ct", "echo new > /home/u/new; sync")
    after = fingerprint()
    for n in SOURCES:
        check(after[n]["guid"] == before[n]["guid"], f"{n}: same guid")
        check(after[n]["written"] == "0", f"{n}: written@{SNAP} = 0")
        if "sha" in before[n]:
            check(after[n]["sha"] == before[n]["sha"], f"{n}: same bytes")

    step("rollback: deleting the guests destroys the clones only")
    req("POST", "/vms/adopt-vm/stop", {}, expect=None)
    wait_for("adopt-vm stopped", lambda: not sh("ps -eo args | grep '[q]emu-system.*adopt-vm' || true"), timeout=60)
    req("POST", "/containers/adopt-ct/stop", {}, expect=None)
    time.sleep(2)
    for path in ["/vms/adopt-vm", "/vms/adopt-dup", "/vms/adopt-cold", "/containers/adopt-ct"]:
        req("DELETE", path, None, expect=None)
    left = sh(f"zfs list -H -o name -r {PARENT} | tail -n +2 || true").strip()
    check(left == "", f"every clone destroyed: {left!r}")
    check(not os.path.exists(f"{side}.efivars.qcow2"), "sidecar files removed with the VM")
    final = fingerprint()
    for n in SOURCES:
        check(final[n]["guid"] == before[n]["guid"] and final[n]["written"] == "0", f"{n}: untouched after rollback")
    log("ALL OK")


if __name__ == "__main__":
    main()
