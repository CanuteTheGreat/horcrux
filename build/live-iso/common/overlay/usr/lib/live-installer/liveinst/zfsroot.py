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

from . import tpmunlock

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
# menu labels of the layouts (installer TUI/GUI); MIN_DISKS below is the disk count each one needs
RAID_LABELS = {"single": "single disk", "mirror": "mirror (RAID1, 2+ disks, survives all but one failing)",
               "stripe": "stripe (2+ disks, NO redundancy: one failed disk loses the pool)",
               "raidz1": "RAID-Z1 (3+ disks, survives one failing)", "raidz2": "RAID-Z2 (4+ disks, survives two failing)"}
POOL_NAME_MAX = 64
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


def pool_name(c):
    """the root pool's name: answer-file key disk.zfs_pool / the installer's Advanced field, default POOL"""
    return (c.get("zfs_pool") or POOL).strip()


def pool_name_problem(name):
    """zpool(8) naming rules (pure): a letter first, then letters, digits, _ - . : ; not a vdev keyword"""
    if not name:
        return "the ZFS pool name is empty"
    if len(name) > POOL_NAME_MAX:
        return f"the ZFS pool name is longer than {POOL_NAME_MAX} characters"
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_.:-]*", name):
        return f"ZFS pool name {name!r}: start with a letter, then only letters, digits, _ - . :"
    if name in ("mirror", "raidz", "draid", "spare", "log", "cache", "special", "dedup") or re.fullmatch(r"(raidz|draid)\d*|c\d.*", name):
        return f"ZFS pool name {name!r} is reserved by zpool (a vdev keyword)"
    return None


def pool_args(encrypt, devs, mirror, target, keyformat="passphrase", pool=POOL):
    """argv of `zpool create` (pure: unit tested). `mirror` = layout name or the old bool."""
    a = ["zpool", "create", "-f", "-o", "ashift=12", "-o", "autotrim=on",
         "-O", "compression=zstd", "-O", "acltype=posixacl", "-O", "xattr=sa", "-O", "relatime=on",
         "-O", "dnodesize=auto", "-O", "normalization=formD", "-O", "mountpoint=none", "-O", "canmount=off"]
    if encrypt:
        a += ["-O", f"encryption={ENC_ALGO}", "-O", f"keyformat={keyformat}", "-O", f"keylocation=file://{KEY_FILE}"]
        if keyformat == "passphrase":
            a += ["-O", "pbkdf2iters=1000000"]
    a += ["-R", target, pool] + vdev_args(mirror, devs)
    return a


def datasets(with_docker=False, product="horcrux"):
    """(name, properties) in creation order; the first BE is rpool/ROOT/horcrux"""
    d = [("ROOT", ["canmount=off", "mountpoint=none"]),
         (f"ROOT/{BE}", ["canmount=noauto", "mountpoint=/", "org.horcrux:be=1"]),
         ("home", ["mountpoint=/home"]),
         ("var", ["canmount=off", "mountpoint=/var"]),
         ("var/log", []),
         ("var/lib", ["canmount=off"]),
         ("var/lib/libvirt", ["recordsize=64K"])]       # qcow2 clusters are 64 KiB
    if product == "horcrux":
        # Horcrux's own VM disks (config paths.vm_storage = /var/lib/horcrux/vms): a boot-environment rollback must never touch them.
        # Only Horcrux has them: a Patronus install (same installer, profile name "patronus") must not get Horcrux datasets.
        d += [("var/lib/horcrux", ["canmount=off"]),
              ("var/lib/horcrux/vms", ["recordsize=64K"])]
    if with_docker:
        d.append(("var/lib/docker", []))
    return d


HOSTID_DRACUT_CONF = ('# written by live-installer: the initramfs imports the root pool with the host id the pool was created with\n'
                      '# (/etc/hostid of the installed system); without it zpool status says "Mismatch between pool hostid and system hostid"\n'
                      'install_items+=" /etc/hostid "\n')


def _initramfs_hostid(inst, img):
    """bytes of /etc/hostid inside the initramfs image `img` (path inside the target), b'' when absent"""
    import subprocess
    r = subprocess.run(["chroot", inst.target, "lsinitrd", "-f", "etc/hostid", img], capture_output=True)
    return r.stdout if r.returncode == 0 else b""


def ensure_initramfs_hostid(inst):
    """After every step that builds an initramfs (T2, TPM seal, hibernate swap): the newest kernel's initramfs holds the same
    /etc/hostid as the installed system, i.e. the one the pool was created with. When it does not (the plain case: the image's
    initramfs is copied as built), rebuild it once; fail the install when it still differs, a pool the initramfs cannot import
    cleanly would report a hostid mismatch on every boot."""
    from . import zfsswap
    from .system import InstallError
    want = open(f"{inst.target}/etc/hostid", "rb").read() if os.path.exists(f"{inst.target}/etc/hostid") else b""
    if len(want) != 4 or want == b"\0\0\0\0":
        raise InstallError("internal: the installed system has no usable /etc/hostid (the pool was created with the live system's)")
    kver = zfsswap._newest_kver(inst)
    img = f"/boot/initramfs-{kver}.img"
    if _initramfs_hostid(inst, img) != want:
        inst.log(f"initramfs {kver} lacks the pool's host id: rebuilding it")
        gcc = f"{inst.target}/usr/lib/gcc"
        chost = sorted(os.listdir(gcc))[0] if os.path.isdir(gcc) and os.listdir(gcc) else "x86_64-pc-linux-gnu"
        inst.run(["dracut", "--force", "--no-hostonly", "--kver", kver, img, kver], chroot=True, env={"CHOST": chost})
        listing = []
        inst.run(["lsinitrd", img], chroot=True, line_cb=lambda l: listing.append(l))
        if not re.search(r"zfs\.ko", "".join(listing)):
            raise InstallError(f"the rebuilt initramfs {img} has no zfs.ko: the root pool could not be imported")
        if _initramfs_hostid(inst, img) != want:
            raise InstallError(f"the initramfs {img} does not carry the installed system's /etc/hostid: the pool would report a hostid mismatch")
    inst.log("initramfs host id = system host id = pool host id")


def force_arg(inst, kver):
    """The ' zfsforce=1' kernel argument or "" for the static (non-BE) grub entry of kernel `kver`, by the zfs_force_import setting: yes/no as said, auto = forced
    only when the initramfs host id is not the pool's (unknown = forced, the old behaviour)"""
    from . import be
    mode = str(inst.c.get("zfs_force_import", "auto")).lower()
    if mode in ("yes", "no"):
        return " zfsforce=1" if mode == "yes" else ""
    have = be.hostid_of(_initramfs_hostid(inst, f"/boot/initramfs-{kver}.img"))
    sysid = be.hostid_of(open(f"{inst.target}/etc/hostid", "rb").read() if os.path.exists(f"{inst.target}/etc/hostid") else b"")
    want = be.pool_hostid(pool_name(inst.c)) or sysid
    return "" if (have and have == want) else " zfsforce=1"


def mkpool(inst):
    c = inst.c
    devs = [inst.layout(d)[4] for d in c["disks"]]
    enc = bool(c.get("zfs_encrypt"))
    inst.run("modprobe zfs", check=False)
    inst.run("test -e /etc/hostid || zgenhostid", check=False)
    if enc:
        fd = os.open(KEY_FILE, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            if c.get("zfs_keymode") == "tpm":
                inst.tpm_key = tpmunlock.new_key()      # 64 hex digits: sealed to the TPM later (configure), also the printed recovery key
                f.write(inst.tpm_key)
            else:
                f.write(c["zfs_passphrase"])       # no trailing newline: the key is exactly what the user typed
    try:
        inst.run(pool_args(enc, devs, c.get("zfs_raid", "single"), inst.target, "hex" if c.get("zfs_keymode") == "tpm" else "passphrase",
                           pool_name(c)))
        if enc:
            # from now on the key is asked for (initramfs prompt); the file URL must not outlive the install
            inst.run(["zfs", "set", "keylocation=prompt", pool_name(c)])
    finally:
        if os.path.exists(KEY_FILE):
            try:
                n = os.path.getsize(KEY_FILE)
                with open(KEY_FILE, "r+b") as f:
                    f.write(b"\0" * n)
            finally:
                os.unlink(KEY_FILE)
    docker = "docker" in inst.p.get("services", [])
    pool = pool_name(inst.c)
    for name, props in datasets(docker, inst.p.get("name", "horcrux")):
        argv = ["zfs", "create"] + [x for p in props for x in ("-o", p)] + [f"{pool}/{name}"]
        inst.run(argv)
    inst.run(["zpool", "set", f"bootfs={pool}/ROOT/{BE}", pool])


def mount(inst):
    # datasets created with canmount=on were auto-mounted under the target at creation, BEFORE the root boot environment existed as a mount:
    # the BE mounted on top of them hid them, and the desktop user's home was created in the BE's own /home (hidden by the empty home dataset
    # at every boot: GNOME could not start). Unmount them first, mount the BE, then mount_rest() mounts the shared datasets above it.
    inst.run(["zfs", "umount", "-a"], check=False)
    inst.run(["zfs", "mount", f"{pool_name(inst.c)}/ROOT/{BE}"])


def mount_rest(inst):
    """after /boot is mounted: the shared datasets (home, var/log, ...)"""
    inst.run(["zfs", "mount", "-a"], check=False)
    home = f"{inst.target}/home"
    if os.system(f"findmnt -n -o FSTYPE {home} | grep -q zfs") != 0:
        from .system import InstallError
        raise InstallError(f"{pool_name(inst.c)}/home is not mounted at {home}: the user's home would be created in the boot environment and hidden at boot")


ZRAM_CONF = """# written by the Horcrux installer: compressed RAM swap instead of swap on ZFS (unsafe) or a disk partition
# read by the image's /etc/init.d/zram-swap (and zram-swap.service); same sizing the old zram-init file had
zram_percent=50
zram_max_mb={max_mb}
"""

BE_CONF = """# horcrux-be (boot environments)
POOL="{pool}"
KEEP="5"
AUTO_SNAPSHOT="yes"
TIMEOUT="5"
CMDLINE="{cmdline}"
# root pool import: auto = zfsforce=1 only on boot entries whose initramfs host id differs from the pool's; yes = always; no = never
ZFS_FORCE_IMPORT="{force}"
KERNEL_GLOB="{kglob}"
# kernels kept bootable per boot environment (one boot-menu entry each); a new kernel boots once as a trial and becomes the default only
# after horcrux-be-good marked it known-good (KERNEL_TRIAL=no: the newest kernel is the default at once); GOOD_DELAY = seconds into a boot
KERNELS_KEEP="2"
KERNEL_TRIAL="yes"
GOOD_DELAY="60"
"""

PORTAGE_BASHRC = """# Horcrux: snapshot the boot environment before the first package operation of an emerge run
if [ "${EBUILD_PHASE}" = "setup" ] && command -v horcrux-be >/dev/null 2>&1; then
    horcrux-be auto-snapshot >/dev/null 2>&1 || true
fi
"""


def configure_zram(inst):
    """One zram swap: the image's zram-swap script (retries mkswap/swapon/zramctl; the zram-init script gave up silently on a slow
    first boot, aarch64 TCG daily run 21506: zram-init 'started', /proc/swaps empty). zram-init is kept out of the runlevels."""
    from .system import mem_mb
    ram = mem_mb() or 8192
    inst.write("/etc/conf.d/zram-swap", ZRAM_CONF.format(max_mb=max(2048, min(ram // 2, 16384))))
    from . import initsys
    if initsys.target_init(inst.target) == "openrc":
        inst.run(["rc-update", "del", "zram-init", "boot"], chroot=True, check=False)
        inst.run(["rc-update", "add", "zram-swap", "default"], chroot=True, check=False)


def configure(inst, cmdline_extra="", kernel_glob="*"):
    """after copy_system, inside the target (chroot bound)"""
    from . import initsys
    configure_zram(inst)
    # systemd: the image's zram-swap.service (the same script as OpenRC's zram-swap, reads /etc/conf.d/zram-swap) is the one zram swap,
    # enabled by cfg_finish_config
    inst.write("/etc/horcrux/be.conf", BE_CONF.format(pool=pool_name(inst.c), cmdline=cmdline_extra.strip(), kglob=kernel_glob, force=inst.c.get("zfs_force_import", "auto")))
    inst.write("/etc/portage/bashrc", PORTAGE_BASHRC)
    inst.write("/etc/kernel/postinst.d/90-horcrux-be", "#!/bin/sh\nexec horcrux-be sync\n", 0o755)
    # the boot-time import of the other pools/datasets: zfs-import/zfs-mount (OpenRC) or zfs-import-cache/zfs-mount + zfs.target (systemd);
    # dracut already mounted /
    for svc in ("zfs-import", "zfs-mount", "zfs-zed"):
        initsys.enable(inst, svc, "boot" if svc != "zfs-zed" else "default")
    # the end-of-boot step that makes a trial kernel the known-good default (OpenRC script / systemd unit of sys-apps/horcrux-be)
    initsys.enable(inst, "horcrux-be-good", "default")
    if inst.c.get("zfs_encrypt") and inst.c.get("zfs_keymode") == "tpm":
        tpm_seal(inst)
    # /boot is its own ext4 partition: fstab (the installer wrote none)
    L = inst.layout(inst.c["disks"][0])
    from .system import fsuuid
    boot_uuid, esp_uuid = fsuuid(L[2]), fsuuid(L[1])
    # multi-disk pool: any drive's ESP + /boot copy can boot the machine, so a dead/missing first drive must not stop the boot at fsck
    # (OpenRC aborts `localmount` -> runlevel 3 without network/sshd): nofail + no fsck pass on those two mounts
    multi = len(inst.c["disks"]) > 1
    opt, pas = ("noatime,nofail", 0) if multi else ("noatime", 2)
    fstab = ["# written by the Horcrux installer", f"UUID={boot_uuid} /boot ext4 {opt} 0 {pas}"]
    if inst.efi and esp_uuid:
        fstab.append(f"UUID={esp_uuid} /boot/efi vfat umask=0077,{opt} 0 {pas}")
    inst.write("/etc/fstab", "\n".join(fstab) + "\n")


def tpm_seal(inst):
    """unattended unlock: seal the pool key to the TPM, rebuild the initramfs with the unlock module and prove it is in there"""
    from .system import InstallError
    key = getattr(inst, "tpm_key", "")
    if not key:
        raise InstallError("internal: TPM key missing")
    inst.recovery_key = inst.c["recovery_key"] = tpmunlock.seal_to_target(inst, key)    # shown once on the done page; never written to disk
    inst.tpm_key = ""
    from . import zfsswap
    zfsswap._rebuild_initramfs_for(inst, tpmunlock.initramfs_problems)
    inst.log("RECOVERY KEY (write it down, it is not stored anywhere): " + inst.recovery_key)
    inst.log("disk key sealed to the TPM 2.0 (PCR 7); the initramfs unseals it at boot, no passphrase is ever asked")


def write_grubcfg(inst, extra_cmdline=""):
    """first BE kernel directory + the static grub.cfg, generated by horcrux-be inside the chroot"""
    L = inst.layout(inst.c["disks"][0])
    from .system import fsuuid
    inst.run(["horcrux-be", "init", "--be", BE, "--boot-uuid", fsuuid(L[2])], chroot=True)
    cfg = open(f"{inst.target}/boot/grub/grub.cfg").read()
    if f"root=ZFS={pool_name(inst.c)}/ROOT/{BE}" not in cfg:
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
    inst.run(["zpool", "export", pool_name(inst.c)], check=False)


def pool_import_key_note():
    return ("The disk passphrase is asked at every boot, before the system starts. It cannot be recovered: "
            "if it is lost the data is gone.")
