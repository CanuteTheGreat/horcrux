"""Portable swap: encrypted, hibernate-capable swap for laptops/tablets/desktops (never for servers).

ONE stage shared by every disk layout. The layout module creates the swap block device and tells the installer where it is
(`inst.swap_dev`); everything else lives here:

    plan(cfg, ram_mb, hw_profile)      -> {mode: none|hibernate, mb, why}   pure; detection is injectable
    resolve(inst)                      -> sets inst.swap_mode / inst.swap_mb / inst.swap_dev, logs why (call in go(), before partitioning)
    prepare(inst)                      -> dracut resume module, elogind/UPower power policy, rc services   (BEFORE the initramfs is built;
                                          the layout writes the swap fstab line: fstab_line(dev))
    rebuild_initramfs(inst)            -> for layouts that do not rebuild it themselves (ext4/xfs)
    kernel_args(inst)                  -> "resume=UUID=<swap uuid>" for the boot loader
    verify_initramfs(listing)          -> [] when the resume hook is in the image

Layouts: xfs-luks = LUKS2 -> LVM (root + swap LV, luksroot.py; ONE passphrase); ext4/xfs = plain swap partition (unencrypted image);
encrypted ZFS = a LUKS2 swap partition (zfsroot.py, resume BEFORE the pool import) - they set inst.swap_dev and call prepare().

Policy (see docs/SURFACE.md): zram stays the high-priority swap (pri 100), the disk swap is pri 10 and exists to hold the hibernation image
(and as overflow). Hibernate = `loginctl hibernate` (elogind, HibernateMode=shutdown). Lid close = suspend; critical battery
(UPower) = hibernate. Nothing here runs on a server: `auto` resolves to `none` unless a battery / laptop chassis / desktop install type /
a hardware profile says portable.
"""
import math
import os
import re
import subprocess

MODES = ("auto", "none", "hibernate")
# SMBIOS chassis types of things you carry: 8 Portable, 9 Laptop, 10 Notebook, 11 Handheld, 14 Sub Notebook, 30 Tablet,
# 31 Convertible, 32 Detachable (a Surface). 13 All-in-one and 3/4/5/6/7 desktop/tower are NOT portable.
PORTABLE_CHASSIS = {8: "Portable", 9: "Laptop", 10: "Notebook", 11: "Handheld", 14: "Sub Notebook", 30: "Tablet",
                    31: "Convertible", 32: "Detachable"}
DMI_DIR = "/sys/class/dmi/id"
PS_DIR = "/sys/class/power_supply"
VG = "hxvg"
SWAP_PRIO = 10                  # zram-swap is 100: the compressed RAM swap is always used first
MIN_SWAP_MB = 2048
MAX_SWAP_MB = 128 * 1024
DISK_RESERVE_MB = 16 * 1024     # the system needs at least this much on top of the swap (else auto = none, explicit = error)


# ------------------------------------------------------------------------------------------ detection (pure, injectable roots)
def chassis_type(dmi_root=DMI_DIR):
    try:
        return int(open(os.path.join(dmi_root, "chassis_type")).read().strip())
    except (OSError, ValueError):
        return None


def batteries(ps_root=PS_DIR):
    """system batteries only: a wireless mouse/keyboard/phone shows up as type Battery with scope=Device and must not count"""
    out = []
    try:
        names = sorted(os.listdir(ps_root))
    except OSError:
        return out
    for n in names:
        d = os.path.join(ps_root, n)
        try:
            typ = open(os.path.join(d, "type")).read().strip()
        except OSError:
            continue
        if typ != "Battery":
            continue
        try:
            scope = open(os.path.join(d, "scope")).read().strip()
        except OSError:
            scope = "System"
        if scope == "Device":
            continue
        out.append(n)
    return out


def detect_portable(dmi_root=DMI_DIR, ps_root=PS_DIR):
    """(bool, reason): a system battery or a portable SMBIOS chassis type means a machine that sleeps and runs on battery"""
    ct = chassis_type(dmi_root)
    bats = batteries(ps_root)
    if ct in PORTABLE_CHASSIS:
        return True, f"DMI chassis type {ct} ({PORTABLE_CHASSIS[ct]})" + (f", battery {bats[0]}" if bats else "")
    if bats:
        return True, f"battery {bats[0]} present"
    return False, "no battery, chassis type " + (str(ct) if ct is not None else "unknown")


# ------------------------------------------------------------------------------------------ plan
def swap_mb(ram_mb):
    """hibernation image + slack: RAM rounded UP to whole GiB (MemTotal is ~100-300 MB below the installed size) x 1.125, in MiB.
    4 GB tablet -> 4608 MiB (4.5 GiB), 8 GB -> 9 GiB, 16 GB -> 18 GiB"""
    if ram_mb <= 0:
        return MIN_SWAP_MB
    gib = math.ceil(ram_mb / 1024)
    return max(MIN_SWAP_MB, min(MAX_SWAP_MB, int(gib * 1024 * 9 // 8)))


def fmt_gib(mb):
    g = mb / 1024
    return f"{g:.1f} GiB" if g != int(g) else f"{int(g)} GiB"


def plan(cfg, ram_mb, hw_profile=None, detect=None):
    """decide the swap for this install. cfg["swap"] = auto | none | hibernate (anything else = auto).
    auto: a hardware profile's `[defaults] swap`, else the desktop install type, else DMI/battery detection, else none (server)."""
    want = str(cfg.get("swap", "auto") or "auto").lower()
    if want not in MODES:
        want = "auto"
    if want == "none":
        return {"mode": "none", "mb": 0, "why": "swap = none requested", "requested": want}
    if want == "hibernate":
        why = "swap = hibernate requested"
    else:
        prof = ((hw_profile or {}).get("defaults") or {}).get("swap")
        desk = cfg.get("install_type") == "desktop" or (cfg.get("desktop") or {}).get("enable")
        if prof in ("hibernate", "none"):
            why = f"hardware profile {hw_profile.get('id')} says swap = {prof}"
            if prof == "none":
                return {"mode": "none", "mb": 0, "why": why, "requested": want}
        elif desk:
            why = "desktop install type"
        else:
            port, reason = (detect or detect_portable)()
            if not port:
                return {"mode": "none", "mb": 0, "why": "server: " + reason, "requested": want}
            why = "portable: " + reason
    if not layout_supported(cfg):
        return {"mode": "none", "mb": 0, "why": f"the {cfg.get('fs')} layout has no portable-swap support ({why})", "requested": want}
    return {"mode": "hibernate", "mb": swap_mb(ram_mb), "why": why, "requested": want}


def describe(p):
    if p["mode"] == "none":
        return "none (" + p["why"] + ")"
    return f"{fmt_gib(p['mb'])} hibernate swap, encrypted with the disk where the layout encrypts ({p['why']}); zram stays first"


def disk_mb(disk):
    try:
        r = subprocess.run(["lsblk", "-bdn", "-o", "SIZE", disk], capture_output=True, text=True, timeout=20)
        return int(r.stdout.split()[0]) // (1024 * 1024)
    except (OSError, ValueError, IndexError, subprocess.SubprocessError):
        return 0


def fit_problem(mb, disk_size_mb):
    """None when swap + the system fit on the disk"""
    if disk_size_mb and mb + DISK_RESERVE_MB > disk_size_mb:
        return f"the {fmt_gib(mb)} hibernate swap leaves less than {DISK_RESERVE_MB // 1024} GiB for the system on a {fmt_gib(disk_size_mb)} disk"
    return None


def validate(cfg):
    errs = []
    v = str(cfg.get("swap", "auto") or "auto").lower()
    if v not in MODES:
        errs.append(f"swap = {v!r}: use auto, none or hibernate")
    return errs


# ------------------------------------------------------------------------------------------ pure config text
def fstab_line(dev):
    return f"{dev}\tnone\tswap\tsw,pri={SWAP_PRIO}\t0 0\n"


def dracut_conf(extra_modules=""):
    return ("# written by live-installer (portable swap): resume from the swap BEFORE any file system is mounted read-write\n"
            f'add_dracutmodules+=" resume {extra_modules} "\n')


def resume_arg(uuid):
    return f"resume=UUID={uuid}"


LOGIND_CONF = """# written by live-installer (portable swap): tablet / laptop power policy
[Login]
HandleLidSwitch=suspend
HandleLidSwitchExternalPower=suspend
HandleLidSwitchDocked=ignore
HandlePowerKey=suspend
HandlePowerKeyLongPress=poweroff
HandleSuspendKey=suspend
HandleHibernateKey=hibernate
IdleAction=ignore
"""

SLEEP_CONF = """# written by live-installer (portable swap)
[Sleep]
AllowSuspend=yes
AllowHibernation=yes
# power off after writing the image: platform (ACPI S4) is flaky on old tablets and unsupported by QEMU
HibernateMode=shutdown
"""

UPOWER_KEYS = {"UsePercentageForPolicy": "true", "PercentageLow": "15", "PercentageCritical": "7", "PercentageAction": "5",
               "CriticalPowerAction": "Hibernate"}


def upower_conf(text):
    """UPower.conf with the critical-battery action = Hibernate (a missing key is added to [UPower]; the rest is kept)"""
    lines = text.splitlines()
    done = set()
    out = []
    for ln in lines:
        m = re.match(r"\s*#?\s*(\w+)\s*=", ln)
        if m and m.group(1) in UPOWER_KEYS:
            k = m.group(1)
            if k not in done:
                out.append(f"{k}={UPOWER_KEYS[k]}")
                done.add(k)
            continue        # drop duplicates/commented defaults of the same key
        out.append(ln)
    missing = [k for k in UPOWER_KEYS if k not in done]
    if missing:
        if not any(l.strip() == "[UPower]" for l in out):
            out.insert(0, "[UPower]")
        i = next(n for n, l in enumerate(out) if l.strip() == "[UPower]") + 1
        out[i:i] = [f"{k}={UPOWER_KEYS[k]}" for k in missing]
    return "\n".join(out) + "\n"


def verify_initramfs(listing):
    """the dracut resume hook must be in the image (lsinitrd output); returns the missing items"""
    return [] if re.search(r"(parse-resume|resume)\.sh", listing) else ["resume hook"]


# ------------------------------------------------------------------------------------------ impure installer helpers
def resolve(inst, ram_mb=None):
    """decide + log. Sets inst.swap_mode (none|hibernate), inst.swap_mb, inst.swap_why, inst.swap_dev=""."""
    from . import hwprofile
    from . import system as _s      # late import: system imports this module
    ram = ram_mb if ram_mb is not None else _s.mem_mb()
    hp = None
    if inst.c.get("hw", "auto") != "none":
        hp, _m = hwprofile.detect()
        if inst.c.get("hw", "auto") not in ("auto", "none"):
            hp = next((x for x in hwprofile.load_profiles() if x.get("id") == inst.c["hw"]), hp)
    p = plan(inst.c, ram, hp)
    if p["mode"] == "hibernate" and p["requested"] == "auto" and inst.p.get("portable_swap") is False:
        p = dict(p, mode="none", mb=0, why="this product profile sets portable_swap = false (appliance)")
    if p["mode"] == "hibernate":
        miss = missing_tools(inst.c)
        if miss:
            msg = "this ISO lacks " + ", ".join(miss) + " (sys-fs/lvm2 without USE=lvm): no LVM swap volume possible"
            if p["requested"] == "hibernate":
                raise RuntimeError(msg + "; use a newer ISO or swap = none")
            p = dict(p, mode="none", mb=0, why=msg)
    inst.swap_mode, inst.swap_mb, inst.swap_why, inst.swap_dev = p["mode"], p["mb"], p["why"], ""
    if p["mode"] == "hibernate":
        prob = fit_problem(p["mb"], disk_mb(inst.c["disks"][0]))
        if prob:
            if p["requested"] == "hibernate":
                raise RuntimeError(prob + " (use swap = none or a bigger disk)")
            inst.swap_mode, inst.swap_mb, inst.swap_why = "none", 0, prob + "; no swap"
    inst.log("swap: " + describe({"mode": inst.swap_mode, "mb": inst.swap_mb, "why": inst.swap_why}))
    return p


def layout_supported(cfg):
    """xfs-luks (LVM), ext4/xfs (swap partition); ZFS only when zfsroot declares PORTABLE_SWAP (encrypted LUKS2 swap partition)"""
    fs = cfg.get("fs", "xfs-luks")
    if fs in ("xfs-luks", "ext4", "xfs"):
        return True
    if fs == "zfs":
        try:
            from . import zfsroot
            return bool(getattr(zfsroot, "PORTABLE_SWAP", False)) and bool(cfg.get("zfs_encrypt"))
        except ImportError:
            return False
    return False


LVM_TOOLS = ("pvcreate", "vgcreate", "lvcreate", "vgchange")


def missing_tools(cfg, path=None):
    """tools the chosen layout needs on the live image: xfs-luks portable = LVM (sys-fs/lvm2 built with USE=lvm; without it only dmsetup exists)"""
    if cfg.get("fs", "xfs-luks") != "xfs-luks":
        return []
    import shutil
    path = path or "/usr/sbin:/sbin:/usr/bin:/bin"
    return [t for t in LVM_TOOLS if not shutil.which(t, path=path)]


def uuid_of(dev):
    r = subprocess.run(["blkid", "-s", "UUID", "-o", "value", dev], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ""


def swap_uuid(inst):
    u = uuid_of(inst.swap_dev)
    if not u:
        raise RuntimeError("cannot read the UUID of the swap " + inst.swap_dev)
    inst.swap_uuid = u
    return u


def kernel_args(inst):
    """extra GRUB_CMDLINE_LINUX words"""
    if getattr(inst, "swap_mode", "none") != "hibernate" or not getattr(inst, "swap_dev", ""):
        return ""
    return resume_arg(getattr(inst, "swap_uuid", "") or swap_uuid(inst))


def prepare(inst, extra_dracut_modules=""):
    """write everything the installed system needs; call it BEFORE the initramfs is built. Idempotent."""
    if getattr(inst, "swap_mode", "none") != "hibernate":
        return
    swap_uuid(inst)
    inst.write("/etc/dracut.conf.d/95-portable-swap.conf", dracut_conf(extra_dracut_modules))
    inst.write("/etc/elogind/logind.conf.d/10-horcrux-portable.conf", LOGIND_CONF)
    inst.write("/etc/elogind/sleep.conf.d/10-horcrux-portable.conf", SLEEP_CONF)
    up = f"{inst.target}/etc/UPower/UPower.conf"
    if os.path.exists(up):
        inst.write("/etc/UPower/UPower.conf", upower_conf(open(up).read()))
        inst.log("UPower: critical battery = Hibernate")
    else:
        inst.log("UPower is not installed on this image: no automatic hibernate on critical battery (lid/power key still work)")
    inst.run(["rc-update", "add", "swap", "boot"], chroot=True, check=False)
    inst.run(["rc-update", "add", "elogind", "boot"], chroot=True, check=False)
    inst.log(f"portable swap: {inst.swap_dev} (UUID {inst.swap_uuid}), resume={inst.swap_uuid}")


def rebuild_initramfs(inst):
    """dracut inside the target with the resume module (layouts without their own dracut step: ext4/xfs)"""
    if getattr(inst, "swap_mode", "none") != "hibernate":
        return
    T = inst.target
    kver = sorted(os.listdir(f"{T}/lib/modules"))[-1]
    gcc = f"{T}/usr/lib/gcc"
    chost = sorted(os.listdir(gcc))[0] if os.path.isdir(gcc) and os.listdir(gcc) else "x86_64-pc-linux-gnu"
    inst.run(["dracut", "--force", "--no-hostonly", "--kver", kver, f"/boot/initramfs-{kver}.img", kver], chroot=True, env={"CHOST": chost})
    r = subprocess.run(["chroot", T, "lsinitrd", f"/boot/initramfs-{kver}.img"], capture_output=True, text=True)
    miss = verify_initramfs(r.stdout)
    inst.log("initramfs check (resume hook): " + ("present" if not miss else "MISSING " + ", ".join(miss)))
    if miss:
        raise RuntimeError("the new initramfs has no resume hook: hibernate would not come back")


def plain_partition_stage(inst, L):
    """ext4/xfs: the swap partition (number 3 in the generic layout) becomes the hibernate swap"""
    inst.swap_dev = L[3]
