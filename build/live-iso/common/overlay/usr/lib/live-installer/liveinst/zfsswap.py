"""Hibernation swap for ENCRYPTED-ZFS laptops (desktop / T2 / portable profile). Servers stay swap-free.

Why a LUKS2 partition and not a zvol: OpenZFS cannot hibernate to a zvol (the image must be written without the pool
being usable). Layout of the disk when this is on:

    p1 ESP   p2 /boot (ext4)   p3 LUKS2 swap  (>= RAM: RAM rounded up to GiB x 1.125; 16 GB -> 18 GiB)   p4 rpool (ZFS, aes-256-gcm)

ONE passphrase: the LUKS2 swap and the ZFS pool use the SAME passphrase. At boot the initramfs asks for it once
(dracut module `horcrux-unlock`), opens the swap with it, and later feeds the very same passphrase to `zfs load-key`
from a 0600 tmpfs file that is shredded immediately. zram stays the higher-priority swap (zram-init default prio 16383),
the disk swap (prio 10) is only for the hibernation image and for memory pressure beyond zram.

ORDER (the important part, asserted by `verify_initramfs` and by the QEMU test): swap unlocked -> RESUME (udev swsuspend
rule + the stock 74resume pre-mount 10 hook) -> ONLY THEN `zpool import`. Importing the pool first and resuming afterwards
corrupts it (the resumed memory image thinks the pool is exported / in an older txg).
"""
import os
import re

from . import testmachine

MAPPER = "hxswap"
DEV = "/dev/mapper/" + MAPPER
MODULE = "horcrux-unlock"
SWAP_PRIO = 10


def luks_format_cmd(dev, ram_mb):
    from .luksroot import pbkdf_memory_kib
    return ["cryptsetup", "luksFormat", "--type", "luks2", "--batch-mode", "--cipher", "aes-xts-plain64", "--key-size", "512",
            "--hash", "sha256", "--pbkdf", "argon2id", "--pbkdf-memory", str(min(pbkdf_memory_kib(ram_mb), 512 * 1024)),
            "--pbkdf-parallel", "2", "--iter-time", "1500", "--key-file=-", dev]


def dracut_conf():
    return ('# Horcrux: LUKS2 hibernation swap + ZFS with ONE passphrase (dracut module horcrux-unlock)\n'
            'add_dracutmodules+=" crypt dm resume %s zfs "\n'
            'add_drivers+=" dm_crypt "\n' % MODULE)


def cmdline(luks_uuid, swap_uuid):
    """resume=UUID= is the swap INSIDE the LUKS mapping (udev links it once hx-unlock opened the mapping)"""
    return f"resume=UUID={swap_uuid} rd.hx.swap={luks_uuid}"


def fstab_line():
    return f"{DEV}\tnone\tswap\tsw,pri={SWAP_PRIO},discard\t0 0\n"


HX_RESUME_INIT = """#!/sbin/openrc-run
# Horcrux: tell the kernel which swap holds the hibernation image (the initramfs only sets /sys/power/resume
# when it finds an image to resume from); without this `loginctl hibernate` has no resume device.
description="set /sys/power/resume to the hibernation swap"
depend() {
    need localmount
    after swap
    before elogind
}
start() {
    dev=$(readlink -f /dev/mapper/hxswap) || return 0
    [ -b "$dev" ] || return 0
    mm=$(cat /sys/class/block/${dev##*/}/dev 2>/dev/null) || return 0
    echo "$mm" > /sys/power/resume
    return 0
}
"""

HX_RESUME_UNIT = """[Unit]
Description=Horcrux: set /sys/power/resume to the hibernation swap
DefaultDependencies=no
After=local-fs.target dev-mapper-hxswap.device
Before=systemd-logind.service sysinit.target
ConditionPathExists=/dev/mapper/hxswap

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c 'dev=$(readlink -f /dev/mapper/hxswap) && [ -b "$dev" ] && cat /sys/class/block/$${dev##*/}/dev > /sys/power/resume || true'

[Install]
WantedBy=sysinit.target
"""


def initramfs_problems(lsinitrd_text):
    """what is missing in the initramfs to hibernate/resume + unlock on one prompt (pure: unit tested)"""
    t = lsinitrd_text
    need = {"hx-unlock": r"bin/hx-unlock", "hx-zfskey (pre-mount)": r"pre-mount/50-hx-zfskey|pre-mount/\d+-hx-zfskey",
            "resume hook": r"pre-mount/10-resume|lib/dracut/resume\.sh", "cryptsetup": r"(?:^|/)cryptsetup\s*$|bin/cryptsetup",
            "zfs.ko": r"zfs\.ko", "dm-crypt": r"dm-crypt\.ko"}
    return [k for k, rx in need.items() if not re.search(rx, t, re.M)]


def plain_initramfs_problems(lsinitrd_text):
    """TEST MACHINE initramfs (pure): ZFS + resume present, NO unlock module / prompt hooks / cryptsetup"""
    t, out = lsinitrd_text, []
    for k, rx in {"zfs.ko": r"zfs\.ko", "resume hook": r"pre-mount/10-resume|lib/dracut/resume\.sh"}.items():
        if not re.search(rx, t, re.M):
            out.append("missing " + k)
    for k, rx in {"hx-unlock": r"bin/hx-unlock", "hx-zfskey": r"hx-zfskey", "cryptsetup": r"bin/cryptsetup"}.items():
        if re.search(rx, t, re.M):
            out.append("unexpected " + k + " (a passphrase prompt could appear)")
    return out


def hook_order_ok(lsinitrd_text):
    """resume (pre-mount 10) must come before the pool import: hx-zfskey is pre-mount 50, 90zfs imports in the mount hook (98)"""
    m = re.findall(r"pre-mount/(\d+)-(\S+?)\.sh", lsinitrd_text)
    order = {n: int(p) for p, n in m}
    if "resume" not in order and re.search(r"lib/dracut/resume\.sh", lsinitrd_text):
        order["resume"] = 10                  # 74resume installs resume.sh; parse-resume moves it to pre-mount/10 at boot
    ok = ("resume" in order and "hx-zfskey" in order and order["resume"] < order["hx-zfskey"])
    mount = [int(p) for p, n in re.findall(r"hooks/mount/(\d+)-(\S+?)\.sh", lsinitrd_text) if "zfs" in n]
    return ok and all(order.get("hx-zfskey", 99) < p for p in mount)


# ---------------------------------------------------------------- installer stages (thin wrappers, need a disk)
def sgdisk_part(mb, plain=False):
    """partition 3 of the disk: the LUKS2 swap (type 8309); a TEST MACHINE without disk encryption gets a plain swap partition (8200)"""
    if plain:
        return ["-n", f"3:0:+{mb}M", "-t", "3:8200", "-c", "3:swap"]
    return ["-n", f"3:0:+{mb}M", "-t", "3:8309", "-c", "3:cryptswap"]


def mkswap_stage(inst):
    """after partitioning, before the pool: LUKS2-format partition 3 with the POOL passphrase, open it, mkswap"""
    from .system import mem_mb
    L = inst.layout(inst.c["disks"][0])
    if testmachine.requested(inst.c):
        inst.run(["mkswap", "-L", "hxswap", L[3]])          # test machine: plain swap, no LUKS
        inst.swap_dev = L[3]
        return
    pp = inst.c["zfs_passphrase"]
    inst.run(luks_format_cmd(L[3], mem_mb() or 8192), input_=pp)
    inst.run(["cryptsetup", "open", "--type", "luks2", "--key-file=-", "--allow-discards", L[3], MAPPER], input_=pp)
    inst.run(["mkswap", "-L", "hxswap", DEV])
    inst.swap_dev = DEV


def _blkid(inst, dev):
    out = []
    inst.run(["blkid", "-s", "UUID", "-o", "value", dev], line_cb=lambda l: out.append(l.strip()))
    return out[0] if out else ""


def prepare(inst):
    """BEFORE the initramfs is built (T2 rebuilds it in t2.apply): dracut modules (resume + horcrux-unlock), kernel args"""
    from . import portableswap
    L = inst.layout(inst.c["disks"][0])
    if testmachine.requested(inst.c):
        return prepare_plain(inst, L)
    inst.swap_dev = DEV
    portableswap.prepare(inst, extra_dracut_modules=f"crypt dm {MODULE} zfs")
    luks_uuid = _blkid(inst, L[3])
    inst.hx_luks_uuid = luks_uuid
    inst.write("/etc/dracut.conf.d/91-horcrux-unlock.conf", dracut_conf())
    inst.cmdline_extra = (inst.cmdline_extra + " " + cmdline(luks_uuid, inst.swap_uuid)).strip()


def prepare_plain(inst, L):
    """TEST MACHINE (no encryption): plain swap partition, dracut resume + zfs only, no unlock module, no passphrase kernel args"""
    from . import portableswap
    inst.swap_dev = L[3]
    portableswap.prepare(inst, extra_dracut_modules="zfs")
    inst.write("/etc/dracut.conf.d/95-portable-swap.conf", testmachine.bootconf(inst.swap_uuid))
    inst.write("/etc/portage/package.use/horcrux-test-machine", testmachine.PACKAGE_USE)
    inst.cmdline_extra = (inst.cmdline_extra + f" resume=UUID={inst.swap_uuid}").strip()
    inst.hx_luks_uuid = ""


def finish_config(inst):
    """after zfsroot.configure (which wrote the fstab): swap line, resume device, initramfs check"""
    if testmachine.requested(inst.c):
        from . import portableswap
        inst.write("/etc/fstab", portableswap.fstab_line("UUID=" + inst.swap_uuid), append=True)
        # no hx-resume service: the kernel is given resume=UUID= on the command line and the stock resume module handles it
        if not getattr(inst, "t2", False):
            _rebuild_initramfs(inst)
        else:
            _check_initramfs(inst)
        return
    inst.write("/etc/fstab", fstab_line(), append=True)
    from . import initsys
    inst.write("/etc/init.d/hx-resume", HX_RESUME_INIT, 0o755)
    inst.write("/etc/systemd/system/hx-resume.service", HX_RESUME_UNIT, 0o644)
    initsys.enable(inst, "hx-resume", "boot")
    if not getattr(inst, "t2", False):
        _rebuild_initramfs(inst)
    else:
        _check_initramfs(inst)


def _newest_kver(inst):
    import glob
    ks = sorted(os.path.basename(k) for k in glob.glob(f"{inst.target}/lib/modules/*"))
    t2 = [k for k in ks if "t2" in k]
    return (t2 or ks or [""])[-1]


def _rebuild_initramfs(inst):
    kver = _newest_kver(inst)
    gcc = f"{inst.target}/usr/lib/gcc"
    chost = sorted(os.listdir(gcc))[0] if os.path.isdir(gcc) and os.listdir(gcc) else "x86_64-pc-linux-gnu"
    inst.run(["dracut", "--force", "--no-hostonly", "--kver", kver, f"/boot/initramfs-{kver}.img", kver], chroot=True, env={"CHOST": chost})
    _check_initramfs(inst, kver)


def _rebuild_initramfs_for(inst, problems_fn):
    """rebuild the newest kernel's initramfs and fail the install when `problems_fn(lsinitrd listing)` names a missing piece"""
    from .system import InstallError
    kver = _newest_kver(inst)
    gcc = f"{inst.target}/usr/lib/gcc"
    chost = sorted(os.listdir(gcc))[0] if os.path.isdir(gcc) and os.listdir(gcc) else "x86_64-pc-linux-gnu"
    inst.run(["dracut", "--force", "--no-hostonly", "--kver", kver, f"/boot/initramfs-{kver}.img", kver], chroot=True, env={"CHOST": chost})
    out = []
    inst.run(["lsinitrd", f"/boot/initramfs-{kver}.img"], chroot=True, line_cb=lambda l: out.append(l))
    miss = problems_fn("".join(out))
    if miss:
        raise InstallError("initramfs is missing: " + ", ".join(miss))


def _check_initramfs(inst, kver=None):
    from .system import InstallError
    kver = kver or _newest_kver(inst)
    out = []
    inst.run(["lsinitrd", f"/boot/initramfs-{kver}.img"], chroot=True, line_cb=lambda l: out.append(l))
    txt = "".join(out)
    if testmachine.requested(inst.c):
        miss = plain_initramfs_problems(txt)
        if miss:
            raise InstallError("test-machine initramfs: " + "; ".join(miss))
        inst.log(f"initramfs {kver}: unencrypted (no unlock module), resume hook present, zfs.ko present")
        return
    miss = initramfs_problems(txt)
    if miss:
        raise InstallError("hibernate initramfs is missing: " + ", ".join(miss))
    if not hook_order_ok(txt):
        raise InstallError("initramfs hook order: the resume hook must run before the ZFS pool import")
    inst.log(f"initramfs {kver}: one-passphrase unlock + resume-before-import verified")
