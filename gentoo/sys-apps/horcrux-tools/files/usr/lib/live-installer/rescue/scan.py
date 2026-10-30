"""Pure parsing/decision helpers of the rescue component (no disks touched; unit tested).

Used by rescue.ops (does the work) and rescue.menu (talks to the user).
"""
import json
import re

FS_MOUNTABLE = {"ext2", "ext3", "ext4", "xfs", "btrfs"}   # the supported root file systems are ext4 / xfs (+ ZFS, see below)
SKIP_FS = {"swap", "iso9660", "squashfs", "linux_raid_member", "LVM2_member", "crypto_LUKS", "zfs_member", "vfat", "", None}


def parse_lsblk(text):
    """`lsblk -J -p -b -o NAME,PATH,FSTYPE,LABEL,UUID,SIZE,TYPE,MOUNTPOINT,PKNAME,RM,PARTLABEL,RO` -> flat list of dicts."""
    try:
        data = json.loads(text)
    except ValueError:
        return []
    out = []

    def walk(nodes, parent=None):
        for n in nodes or []:
            d = {k.lower(): n.get(k) for k in ("name", "path", "fstype", "label", "uuid", "size", "type", "mountpoint", "pkname", "rm", "partlabel", "ro")}
            d["parent"] = parent
            d["path"] = d["path"] or d["name"]
            try:
                d["size"] = int(d["size"] or 0)
            except (TypeError, ValueError):
                d["size"] = 0
            out.append(d)
            walk(n.get("children"), d["path"])

    walk(data.get("blockdevices"))
    return out


def live_medium_devices(devs, volid):
    """Paths of the device(s) holding the live medium (label == volid) plus their parent disk: never offered as a system."""
    live = set()
    for d in devs:
        if volid and d.get("label") == volid:
            live.add(d["path"])
            if d.get("parent"):
                live.add(d["parent"])
    # the stick's appended EFI partition and every other partition of that disk
    return live | {d["path"] for d in devs if d.get("parent") in live}


def classify(d):
    """locked | zfs | lvm | raid | fs | efi | swap | skip"""
    ft = d.get("fstype") or ""
    if ft == "crypto_LUKS":
        return "locked"
    if ft == "zfs_member":
        return "zfs"
    if ft == "LVM2_member":
        return "lvm"
    if ft == "linux_raid_member":
        return "raid"
    if ft in FS_MOUNTABLE:
        return "fs"
    if ft == "vfat":
        return "efi"
    if ft == "swap":
        return "swap"
    return "skip"


def parse_zpool_import(text):
    """`zpool import` (list mode) -> [{name,id,state}]"""
    pools, cur = [], None
    for line in text.splitlines():
        m = re.match(r"\s*pool:\s*(\S+)", line)
        if m:
            cur = {"name": m.group(1), "id": "", "state": ""}
            pools.append(cur)
            continue
        if cur is None:
            continue
        m = re.match(r"\s*id:\s*(\d+)", line)
        if m:
            cur["id"] = m.group(1)
        m = re.match(r"\s*state:\s*(\S+)", line)
        if m:
            cur["state"] = m.group(1)
    return pools


def zfs_root_datasets(text, altroot):
    """`zfs list -H -o name,mountpoint,canmount,type -r <pool>` -> datasets that mount at the (alt)root '/'."""
    res = []
    alt = altroot.rstrip("/")
    for line in text.splitlines():
        f = line.split("\t")
        if len(f) < 3:
            continue
        name, mp, canmount = f[0], f[1], f[2]
        if mp in ("/", alt, alt + "/") and canmount in ("on", "noauto"):
            res.append(name)
    return res


def zfs_child_datasets(text, altroot):
    """datasets below the root dataset that mount under the altroot, shortest mount point first"""
    alt = altroot.rstrip("/")
    kids = []
    for line in text.splitlines():
        f = line.split("\t")
        if len(f) < 3:
            continue
        name, mp, canmount = f[0], f[1], f[2]
        if canmount in ("on", "noauto") and mp.startswith(alt + "/"):
            kids.append((mp, name))
    return [n for _, n in sorted(kids, key=lambda t: (len(t[0]), t[0]))]


def os_pretty(text):
    m = re.search(r'^PRETTY_NAME="?([^"\n]*)"?', text or "", re.M)
    if m:
        return m.group(1)
    m = re.search(r'^NAME="?([^"\n]*)"?', text or "", re.M)
    return m.group(1) if m else ""


def parse_fstab(text):
    """-> [(spec, mountpoint, fstype, options)] without comments"""
    rows = []
    for line in (text or "").splitlines():
        line = line.split("#", 1)[0].strip()
        f = line.split()
        if len(f) >= 3:
            rows.append((f[0], f[1], f[2], f[3] if len(f) > 3 else "defaults"))
    return rows


def fstab_mounts(text):
    """The extra file systems worth mounting for a rescue (/boot, /boot/efi, /var, /home ... local ones): never swap,
    never noauto, never network or virtual file systems. Parents before children."""
    out = []
    for spec, mp, ft, opts in parse_fstab(text):
        if mp in ("/", "none", "swap") or ft in ("swap", "tmpfs", "proc", "sysfs", "devpts", "nfs", "nfs4", "cifs", "overlay", "zfs"):
            continue
        if "noauto" in opts.split(","):
            continue
        out.append((spec, mp, ft))
    return sorted(out, key=lambda t: (t[1].count("/"), t[1]))


def blkid_args(spec):
    """fstab spec -> argv that resolves it to a device path (None = already a path)"""
    if spec.startswith("UUID="):
        return ["blkid", "-U", spec[5:].strip('"')]
    if spec.startswith("LABEL="):
        return ["blkid", "-L", spec[6:].strip('"')]
    if spec.startswith("PARTUUID="):
        return ["blkid", "-t", "PARTUUID=" + spec[9:].strip('"'), "-o", "device"]
    if spec.startswith("PARTLABEL="):
        return ["blkid", "-t", "PARTLABEL=" + spec[10:].strip('"'), "-o", "device"]
    return None


def ro_mount_opts(fstype, forensic):
    """mount options: forensic (and every probe) = read-only without journal replay / log recovery"""
    if fstype in ("ext2", "ext3", "ext4"):
        return "ro,noload,noatime" if forensic else "rw"
    if fstype == "xfs":
        return "ro,norecovery,nouuid,noatime" if forensic else "rw"
    if fstype == "vfat":
        return "ro" if forensic else "rw"
    return "ro,noatime" if forensic else "rw"


def probe_mount_opts(fstype):
    o = ro_mount_opts(fstype, True)
    return o


def grub_target(arch, efi):
    """grub-install --target for a machine architecture"""
    if efi:
        return {"x86_64": "x86_64-efi", "aarch64": "arm64-efi", "riscv64": "riscv64-efi", "i686": "i386-efi"}.get(arch)
    return "i386-pc" if arch in ("x86_64", "i686", "i386") else None


def static_zfs_grubcfg(title, kver, boot_uuid, dataset, extra):
    serial = "serial --unit=0 --speed=115200\nterminal_input console serial\nterminal_output console serial\n" if extra else ""
    return f"""# generated by live-rescue (root on ZFS: grub-mkconfig cannot probe it)
set timeout=3
{serial}insmod part_gpt
insmod ext2
search --no-floppy --fs-uuid --set=root {boot_uuid}
menuentry "{title} (ZFS root)" {{
    linux /vmlinuz-{kver} root=ZFS={dataset} zfsforce=1 ro {extra}
    initrd /initramfs-{kver}.img
}}
"""


def twofactor_columns(columns):
    """column names of a users table that hold 2FA state (offered for clearing when the admin is locked out by a lost token)"""
    return [c for c in columns if re.search(r"(totp|mfa|2fa|two_?factor)", c, re.I) and re.search(r"(enabled|secret|active|confirmed)", c, re.I)]


def safe_dataset_name(s):
    return bool(re.fullmatch(r"[A-Za-z0-9_.:/-]+", s or ""))


def pick_boot_parts(devs, root_dev, live=()):
    """(boot_dev, esp_dev) for a system whose fstab does not list /boot and /boot/efi (the live installer writes none):
    the ext4 partition labelled 'boot' and the vfat ESP, preferring the disk that holds the root file system."""
    by = {d["path"]: d for d in devs}

    def top(path):
        d = by.get(path)
        while d and d.get("parent") in by:
            d = by[d["parent"]]
        return d["path"] if d else None

    disk = top(root_dev)

    def best(match):
        c = [d for d in devs if d["path"] not in live and match(d)]
        c.sort(key=lambda d: (top(d["path"]) != disk, d["path"]))
        return c[0]["path"] if c else None

    boot = best(lambda d: (d.get("fstype") or "") in ("ext2", "ext3", "ext4") and "boot" in ((d.get("label") or "").lower(), (d.get("partlabel") or "").lower()))
    esp = best(lambda d: d.get("fstype") == "vfat" and "esp" in ((d.get("label") or "").lower(), (d.get("partlabel") or "").lower()))
    return boot, esp
