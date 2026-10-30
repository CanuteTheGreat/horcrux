"""ZFS root for the installer: native encryption, boot-environment dataset layout, zram instead of swap.

Disk layout (erase-disk install, one or two disks):
  p1  ESP     1 GiB FAT32 (unencrypted)      p2  /boot  2 GiB ext4 (unencrypted: GRUB cannot read encrypted ZFS)
  p4  rest    zpool `rpool` (ashift=12, zstd, autotrim, optional aes-256-gcm with a passphrase typed by the user)
Servers: no swap partition (ZFS swap is unsafe), a zram device is configured instead. Encrypted laptops/desktops add
  p3 = LUKS2 hibernation swap (zfsswap.py, the pool passphrase unlocks it too: ONE prompt, resume BEFORE the pool import).

The passphrase only exists in the installer's memory and a 0600 file on tmpfs for the duration of `zpool create`;
it is never logged, never written to the installed system, and the key is prompted for at every boot.
"""
import os
import re

POOL = "rpool"
BE = "horcrux"
KEY_FILE = "/run/horcrux-zfs.key"
ENC_ALGO = "aes-256-gcm"
MIN_PASSPHRASE = 12
PORTABLE_SWAP = True        # encrypted pool + LUKS2 hibernation swap partition (zfsswap.py); read by portableswap.layout_supported


def passphrase_problem(p):
    if len(p) < MIN_PASSPHRASE:
        return f"The disk passphrase must be at least {MIN_PASSPHRASE} characters ({len(p)} so far)."
    if len(set(p)) < 5:
        return "The passphrase uses too few different characters."
    if "\n" in p or "\r" in p:
        return "The passphrase must be a single line."
    return None


RAID_LEVELS = ("single", "mirror", "stripe", "raidz1", "raidz2")
MIN_DISKS = {"single": 1, "mirror": 2, "stripe": 2, "raidz1": 3, "raidz2": 4}


def vdev_args(raid, devs):
    """the vdev part of `zpool create` (pure). `raid` is a level name; a bool (old API) means mirror/single.
    stripe = every disk is its own top-level vdev (NO redundancy: one failed disk loses the pool)."""
    if raid is True:
        raid = "mirror"
    elif not raid:
        raid = "single"
    devs = list(devs)
    if raid == "single":
        return devs[:1]
    if raid == "stripe":
        return devs
    if raid in ("mirror", "raidz1", "raidz2"):
        return [raid] + devs
    raise ValueError(f"unknown ZFS layout {raid!r}")


def raid_problem(raid, ndisks):
    """validation text for a layout/disk-count pair, or None"""
    if raid not in RAID_LEVELS:
        return f"unknown ZFS layout {raid!r} (one of {', '.join(RAID_LEVELS)})"
    if ndisks < MIN_DISKS[raid]:
        w = {2: "two", 3: "three", 4: "four"}[MIN_DISKS[raid]]
        return f"ZFS {raid} needs {w} different disks ({ndisks} selected)"
    if raid == "single" and ndisks > 1:
        return "ZFS single-disk layout takes exactly one disk: choose stripe or mirror for several"
    return None


def pool_args(encrypt, devs, mirror, target):
    """argv of `zpool create` (pure: unit tested). `mirror` = layout name or the old bool."""
    a = ["zpool", "create", "-f", "-o", "ashift=12", "-o", "autotrim=on",
         "-O", "compression=zstd", "-O", "acltype=posixacl", "-O", "xattr=sa", "-O", "relatime=on",
         "-O", "dnodesize=auto", "-O", "normalization=formD", "-O", "mountpoint=none", "-O", "canmount=off"]
    if encrypt:
        a += ["-O", f"encryption={ENC_ALGO}", "-O", "keyformat=passphrase", "-O", f"keylocation=file://{KEY_FILE}",
              "-O", "pbkdf2iters=1000000"]
    a += ["-R", target, POOL] + vdev_args(mirror, devs)
    return a


def datasets(with_docker=False):
    """(name, properties) in creation order; the first BE is rpool/ROOT/horcrux"""
    d = [("ROOT", ["canmount=off", "mountpoint=none"]),
         (f"ROOT/{BE}", ["canmount=noauto", "mountpoint=/", "org.horcrux:be=1"]),
         ("home", ["mountpoint=/home"]),
         ("var", ["canmount=off", "mountpoint=/var"]),
         ("var/log", []),
         ("var/lib", ["canmount=off"]),
         ("var/lib/libvirt", ["recordsize=64K"])]       # qcow2 clusters are 64 KiB
    if with_docker:
        d.append(("var/lib/docker", []))
    return d


def mkpool(inst):
    c = inst.c
    devs = [inst.layout(d)[4] for d in c["disks"]]
    enc = bool(c.get("zfs_encrypt"))
    inst.run("modprobe zfs", check=False)
    inst.run("test -e /etc/hostid || zgenhostid", check=False)
    if enc:
        fd = os.open(KEY_FILE, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(c["zfs_passphrase"])           # no trailing newline: the key is exactly what the user typed
    try:
        inst.run(pool_args(enc, devs, c.get("zfs_raid", "single"), inst.target))
        if enc:
            # from now on the key is asked for (initramfs prompt); the file URL must not outlive the install
            inst.run(["zfs", "set", "keylocation=prompt", POOL])
    finally:
        if os.path.exists(KEY_FILE):
            try:
                n = os.path.getsize(KEY_FILE)
                with open(KEY_FILE, "r+b") as f:
                    f.write(b"\0" * n)
            finally:
                os.unlink(KEY_FILE)
    docker = "docker" in inst.p.get("services", [])
    for name, props in datasets(docker):
        argv = ["zfs", "create"] + [x for p in props for x in ("-o", p)] + [f"{POOL}/{name}"]
        inst.run(argv)
    inst.run(["zpool", "set", f"bootfs={POOL}/ROOT/{BE}", POOL])


def mount(inst):
    # `zfs create` mounts every new dataset with an auto mount point (home, var/log, ...) at once, BEFORE the canmount=noauto root dataset is
    # mounted on /mnt/target: the root mount then shadows them, the system image and the new user's home land in the root dataset and the
    # shared datasets stay empty (home empty at first boot, `zfs umount -a` busy at the end). Unmount first, root first, then the rest.
    inst.run(["zfs", "umount", "-a"], check=False)
    inst.run(["zfs", "mount", f"{POOL}/ROOT/{BE}"])


def mount_rest(inst):
    """after /boot is mounted: the shared datasets (home, var/log, ...)"""
    inst.run(["zfs", "mount", "-a"], check=False)


ZRAM_CONF = """# written by the Horcrux installer: compressed RAM swap instead of swap on ZFS (unsafe) or a disk partition
load_on_start=yes
unload_on_stop=yes
num_devices=1
type0=swap
flag0=
size0={mb}
maxs0=1
algo0=zstd
"""

BE_CONF = """# horcrux-be (boot environments)
POOL="rpool"
KEEP="5"
AUTO_SNAPSHOT="yes"
TIMEOUT="5"
CMDLINE="{cmdline}"
KERNEL_GLOB="{kglob}"
"""

PORTAGE_BASHRC = """# Horcrux: snapshot the boot environment before the first package operation of an emerge run
if [ "${EBUILD_PHASE}" = "setup" ] && command -v horcrux-be >/dev/null 2>&1; then
    horcrux-be auto-snapshot >/dev/null 2>&1 || true
fi
"""


def configure(inst, cmdline_extra="", kernel_glob="*"):
    """after copy_system, inside the target (chroot bound)"""
    from .system import mem_mb
    ram = mem_mb() or 8192
    inst.write("/etc/conf.d/zram-init", ZRAM_CONF.format(mb=max(2048, min(ram // 2, 16384))))
    if os.path.exists(f"{inst.target}/etc/init.d/zram-init"):
        inst.run(["rc-update", "add", "zram-init", "boot"], chroot=True, check=False)
    inst.write("/etc/horcrux/be.conf", BE_CONF.format(cmdline=cmdline_extra.strip(), kglob=kernel_glob))
    inst.write("/etc/portage/bashrc", PORTAGE_BASHRC)
    inst.write("/etc/kernel/postinst.d/90-horcrux-be", "#!/bin/sh\nexec horcrux-be sync\n", 0o755)
    # the boot-time import of an encrypted pool: zfs-import/zfs-mount from OpenRC (dracut already mounted /)
    for svc in ("zfs-import", "zfs-mount", "zfs-zed"):
        inst.run(["rc-update", "add", svc, "boot" if svc != "zfs-zed" else "default"], chroot=True, check=False)
    # /boot is its own ext4 partition: fstab (the installer wrote none)
    L = inst.layout(inst.c["disks"][0])
    from .system import fsuuid
    boot_uuid, esp_uuid = fsuuid(L[2]), fsuuid(L[1])
    fstab = ["# written by the Horcrux installer", f"UUID={boot_uuid} /boot ext4 noatime 0 2"]
    if inst.efi and esp_uuid:
        fstab.append(f"UUID={esp_uuid} /boot/efi vfat umask=0077,noatime 0 2")
    inst.write("/etc/fstab", "\n".join(fstab) + "\n")


def write_grubcfg(inst, extra_cmdline=""):
    """first BE kernel directory + the static grub.cfg, generated by horcrux-be inside the chroot"""
    L = inst.layout(inst.c["disks"][0])
    from .system import fsuuid
    inst.run(["horcrux-be", "init", "--be", BE, "--boot-uuid", fsuuid(L[2])], chroot=True)
    cfg = open(f"{inst.target}/boot/grub/grub.cfg").read()
    if f"root=ZFS={POOL}/ROOT/{BE}" not in cfg:
        from .system import InstallError
        raise InstallError("grub.cfg has no ZFS boot environment entry")
    mirror_boot(inst)


def mirror_conf(rows):
    """/etc/horcrux/boot-mirrors.conf (pure): one line per extra drive: <disk> <ESP uuid> <boot uuid>"""
    return "# written by the Horcrux installer: extra drives that carry a copy of /boot + an ESP (horcrux-boot-sync)\n" + \
        "".join(f"{d} {e} {b}\n" for d, e, b in rows)


def mirror_boot(inst):
    """multi-disk pool: every extra drive gets a synced /boot + its own bootable ESP, so any drive's ESP can boot"""
    from .system import fsuuid, InstallError
    disks = inst.c["disks"]
    if len(disks) < 2 or not inst.efi:
        return
    rows = []
    for d in disks[1:]:
        l = inst.layout(d)
        rows.append((d, fsuuid(l[1]), fsuuid(l[2])))
    inst.write("/etc/horcrux/boot-mirrors.conf", mirror_conf(rows))
    inst.write("/etc/kernel/postinst.d/95-horcrux-boot-sync", "#!/bin/sh\nexec horcrux-boot-sync\n", 0o755)
    inst.run(["horcrux-boot-sync", "--init"], chroot=True)


def finish(inst):
    inst.run(["swapoff", "-a"], check=False)
    inst.run(["cryptsetup", "close", "hxswap"], check=False)      # the hibernation swap mapping (zfsswap.py), if any
    inst.run(["zfs", "umount", "-a"], check=False)
    inst.run(["zpool", "export", POOL], check=False)


def pool_import_key_note():
    return ("The disk passphrase is asked at every boot, before the system starts. It cannot be recovered: "
            "if it is lost the data is gone.")
