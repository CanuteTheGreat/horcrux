"""Kernel line (Horcrux `system.kernel_line`): `default` = the distribution's stable kernel, `newest` = opt-in, the newest kernel the
repositories carry, held at the newest line the installed OpenZFS module builds against (zfs-kmod's MODULES_KERNEL_MAX).

The installer records the choice for the installed system's manager (profile key `kernel_line_state`, a JSON file the manager reads) and
writes the two Portage pins the manager keeps in step afterwards (`package.accept_keywords/<pin>`, `package.mask/<pin>`; profile key
`kernel_line_pin`). The text is the same as horcrux-api's `api/system_admin/kernel_line.rs` writes (both test suites pin it), so the manager
sees its own files. The newer kernel itself arrives with the first update of the installed system (horcrux-autoupdate: boot-environment
snapshot, @world update, the previous kernel stays in the boot menu); nothing here changes the boot path.

Offered only when the profile sets `kernel_line_choice = true`."""
import glob
import json
import os
import platform
import re

LINES = [("default", "Default - the distribution's stable kernel (recommended)"),
         ("newest", "Newest - opt-in: the newest kernel the repositories carry (held at what OpenZFS supports)")]
KERNEL_PACKAGES = ["sys-kernel/gentoo-kernel-bin", "sys-kernel/gentoo-kernel", "virtual/dist-kernel"]
ZFS_MODULE = "sys-fs/zfs-kmod"
DEFAULT_PIN = "horcrux-kernel-line"
DEFAULT_RECONCILE_MINUTES = 60


def offered(profile):
    return bool(profile.get("kernel_line_choice"))


def problem(line):
    return None if line in [x[0] for x in LINES] else f"kernel_line {line!r} unknown (default, newest)"


def portage_arch(machine=None):
    m = machine or platform.machine()
    return {"x86_64": "amd64", "aarch64": "arm64", "riscv64": "riscv", "ppc64le": "ppc64"}.get(m, m)


def _vkey(v):
    return [int(x) if x.isdigit() else x for x in re.split(r"[._\-]", v)]


def zfs_cap(root="/"):
    """(max 'major.minor', 'sys-fs/zfs-kmod-<ver>') of the newest installed zfs-kmod, or None"""
    hits = []
    for d in glob.glob(os.path.join(root, "var/db/pkg/sys-fs/zfs-kmod-[0-9]*")):
        ver = os.path.basename(d)[len("zfs-kmod-"):]
        hits.append((_vkey(ver), ver, d))
    if not hits:
        return None
    _k, ver, d = max(hits)
    try:
        with open(os.path.join(d, f"zfs-kmod-{ver}.ebuild")) as f:
            text = f.read()
    except OSError:
        return None
    m = re.search(r"^\s*MODULES_KERNEL_MAX=[\"']?([0-9][0-9.]*)", text, re.M)
    return (m.group(1), f"{ZFS_MODULE}-{ver}") if m else None


def pins(line, arch, cap):
    """(accept_keywords text | None, mask text | None) exactly as the manager writes them"""
    if line != "newest":
        return None, None
    head = ("# Written by Horcrux: system.kernel_line = newest (System > Packages > Kernel line).\n"
            "# Edit the setting, not this file: Horcrux rewrites it.\n")
    kw = head + "".join(f"{p} ~{arch}\n" for p in KERNEL_PACKAGES)
    mask = None
    if cap:
        mx, by = cap
        parts = [int(x) for x in re.findall(r"\d+", mx)[:2]] + [0, 0]
        if parts[0] > 0:
            mask = (head + f"# {by} builds against kernels up to {mx}: newer kernel lines wait for it.\n"
                    + "".join(f">={p}-{parts[0]}.{parts[1] + 1}\n" for p in KERNEL_PACKAGES))
    return kw, mask


def apply(inst, line, profile, arch=None):
    """write the setting + pins into the target (Installer.write keeps /etc/portage modes right)"""
    pin = profile.get("kernel_line_pin", DEFAULT_PIN)
    kw, mask = pins(line, arch or portage_arch(), zfs_cap(inst.target))
    for kind, text in (("package.accept_keywords", kw), ("package.mask", mask)):
        rel = f"/etc/portage/{kind}/{pin}"
        p = os.path.join(inst.target, rel.lstrip("/"))
        if text is None:
            if os.path.exists(p):
                os.remove(p)
        elif os.path.isfile(os.path.dirname(p)):
            inst.log(f"warning: {os.path.dirname(rel)} is a file: kernel line pin not written (the manager reports it)")
        else:
            inst.write(rel, text)
    state = profile.get("kernel_line_state")
    if state:
        inst.write(state, json.dumps({"line": line, "reconcile_minutes": DEFAULT_RECONCILE_MINUTES}, indent=2) + "\n", 0o640)
    return kw, mask
