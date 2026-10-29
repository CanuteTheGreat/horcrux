"""Hardware profiles: DMI autodetect + the per-profile install steps (generic; Surface is the first user).

A profile is a TOML file in /usr/share/live-installer/hwprofiles/ (shipped by gentoo/<profile>/):

    id = "surface"            title = "Microsoft Surface"
    [match]   sys_vendor = ["Microsoft Corporation"]   product_name = ["Surface Pro", ...]   product_prefix = ["Surface"]
    [defaults] fs = "xfs-luks"   desktop = true
    [[models]] id = "surface-pro-1"  product_name = [...]  kernel_cmdline = [...]  notes = "..."

`detect()` returns the best matching (profile, model) or (None, None); a model match beats a prefix-only match.
Nothing here changes a system that is not detected: Horcrux on a server behaves exactly as before.
"""
import os
import re

PROFILE_DIR = "/usr/share/live-installer/hwprofiles"
DMI_DIR = "/sys/class/dmi/id"
LOW_RAM_MB = 7000          # MemTotal of a "8 GB" machine is ~7.6-7.9 GB (firmware/GPU carve-outs): below this = low-RAM profile


def read_dmi(root=DMI_DIR):
    out = {}
    for k in ("sys_vendor", "product_name", "product_version", "product_family", "product_sku", "board_name"):
        try:
            with open(os.path.join(root, k)) as f:
                out[k] = f.read().strip()
        except OSError:
            out[k] = ""
    return out


def load_profiles(path=PROFILE_DIR):
    import tomllib
    res = []
    if not os.path.isdir(path):
        return res
    for f in sorted(os.listdir(path)):
        if f.endswith(".toml"):
            try:
                with open(os.path.join(path, f), "rb") as fh:
                    res.append(tomllib.load(fh))
            except Exception:       # a broken profile file must never break the installer
                continue
    return res


def _norm(s):
    return re.sub(r"\s+", " ", s.strip()).lower()


def detect(dmi=None, profiles=None):
    """(profile, model) for this machine, or (None, None)"""
    dmi = dmi if dmi is not None else read_dmi()
    profiles = profiles if profiles is not None else load_profiles()
    vendor, prod = _norm(dmi.get("sys_vendor", "")), _norm(dmi.get("product_name", ""))
    best = (None, None, 0)
    for p in profiles:
        m = p.get("match", {})
        if vendor not in [_norm(v) for v in m.get("sys_vendor", [])]:
            continue
        score, model = 0, None
        for mod in p.get("models", []):
            if prod in [_norm(x) for x in mod.get("product_name", [])]:
                score, model = 3, mod
                break
        if not score and prod in [_norm(x) for x in m.get("product_name", [])]:
            score = 2
        if not score and any(prod.startswith(_norm(x)) for x in m.get("product_prefix", [])):
            score = 1
        if score > best[2]:
            best = (p, model, score)
    return best[0], best[1]


def low_ram(mem_mb):
    return 0 < mem_mb < LOW_RAM_MB


def default_fs(filesystems, mem_mb, profile=None):
    """Interactive default: XFS+LUKS on low-RAM machines (and wherever a hardware profile asks for it), else ZFS when the
    image has it (checksums, snapshots), else ext4."""
    want = (profile or {}).get("defaults", {}).get("fs")
    if want in filesystems:
        return want
    if low_ram(mem_mb) and "xfs-luks" in filesystems:
        return "xfs-luks"
    if "zfs" in filesystems:
        return "zfs"
    return "ext4"


def kernel_cmdline(profile, model):
    out = []
    for src in (profile or {}, model or {}):
        for a in src.get("kernel_cmdline", []):
            if a not in out:
                out.append(a)
    return out


# ------------------------------------------------------------------ install-time steps
def apply_profile(inst, profile, model):
    """Run on the installed system after the base configuration. Everything is optional and idempotent."""
    c = inst.c
    d = c.get("desktop") or {}
    inst.log(f"hardware profile: {profile['id']} / {(model or {}).get('id', 'generic')}")
    for s in profile.get("services_add", []):
        name, _, lvl = s.partition(":")
        inst.run(["rc-update", "add", name, lvl or "default"], chroot=True, check=False)
    for s in profile.get("services_del", []):
        name, _, lvl = s.partition(":")
        inst.run(["rc-update", "del", name, lvl or "default"], chroot=True, check=False)
    if d.get("enable") and d.get("user"):
        add_desktop_user(inst, d)


def add_desktop_user(inst, d):
    u = d["user"]
    inst.run(["useradd", "-m", "-G", "wheel,audio,video,input,plugdev,users", "-s", "/bin/bash", u], chroot=True, check=False)
    if d.get("password_hash"):
        inst.run(["chpasswd", "-e"], chroot=True, input_=f"{u}:{d['password_hash']}\n")
    elif d.get("password"):
        inst.run(["chpasswd"], chroot=True, input_=f"{u}:{d['password']}\n")
    else:
        inst.run(["passwd", "-l", u], chroot=True, check=False)
    if d.get("autologin"):
        # opt-in: tty1 logs this user in and starts the session. With LUKS the disk is already unlocked by a passphrase.
        ip = f"{inst.target}/etc/inittab"
        if os.path.exists(ip):
            t = open(ip).read()
            t = re.sub(r"(?m)^c1:.*$", f"c1:12345:respawn:/sbin/agetty --autologin {u} --noclear 38400 tty1 linux", t)
            open(ip, "w").write(t)
        inst.log(f"autologin enabled for {u} on tty1 (opt-in)")
