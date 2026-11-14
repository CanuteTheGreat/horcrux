"""Init-system abstraction for the installer and the live tools (owner rule 2026-11-13: Horcrux and Patronus support OpenRC *and* systemd).

The installed system runs the SAME init as the ISO it was installed from: the target is a copy of the live root, so its init is fixed by the
download the user picked (`<product>-<edition>-openrc-...iso` or `...-systemd-...iso`). `target_init(root)` reads it from the copied tree.

Service names in profiles, services.list files and the installer code are the OpenRC names (the historical vocabulary); `unit_names()` maps
each one to the systemd unit(s) with the same job. A name that maps to () has no systemd counterpart because systemd itself does that job
(swclock/hwclock: the kernel RTC + chrony rtcsync; elogind: systemd-logind; local: rc-local; swap: fstab swap units).

Pure helpers are unit-tested in tests/test_initsys.py: no root, no VM.
"""
import os
import subprocess

INITS = ("openrc", "systemd")

# OpenRC name -> systemd unit(s). Names not listed map to "<name>.service".
UNIT_MAP = {
    # ZFS: the pool import + mounts + ZED (OpenRC zfs-import / zfs-mount / zfs-zed). The targets must be enabled too, else the
    # services are never pulled in at boot (sys-fs/zfs ships them disabled).
    "zfs-import": ("zfs-import-cache.service", "zfs-import.target"),
    "zfs-mount": ("zfs-mount.service", "zfs.target"),
    "zfs-zed": ("zfs-zed.service",),
    "zfs-share": ("zfs-share.service",),
    "sshd": ("sshd.service",),
    "cronie": ("cronie.service",),
    "chronyd": ("chronyd.service",),
    "dhcpcd": ("dhcpcd.service",),
    "iwd": ("iwd.service",),
    "NetworkManager": ("NetworkManager.service",),
    "docker": ("docker.service",),
    "libvirtd": ("libvirtd.service",),
    "dbus": (),                       # socket-activated by systemd itself
    "udev": (),
    "elogind": (),                    # systemd-logind
    "swclock": (),
    "hwclock": (),
    "local": (),
    "swap": (),                       # fstab swap entries become swap units
    "sysklogd": (),                   # journald
    "zram-init": ("zram-swap.service",),
    "cupsd": ("cups.service",),
    # display-manager / xdm: OpenRC's generic DM script; on systemd the DM's own unit (enabling it creates the display-manager.service
    # alias), resolved per target by enable() (DISPLAY_MANAGERS)
}
DISPLAY_MANAGERS = ("gdm.service", "lightdm.service", "sddm.service")


def _check(init):
    if init not in INITS:
        raise ValueError(f"unknown init {init!r} (expected one of {INITS})")


def target_init(root="/"):
    """The init of the system rooted at `root`: systemd when /sbin/init (or /usr/sbin/init) resolves to systemd, else OpenRC."""
    for p in ("sbin/init", "usr/sbin/init"):
        f = os.path.join(root, p)
        if os.path.islink(f):
            dest = os.readlink(f)
            if "systemd" in dest:
                return "systemd"
            if "openrc" in dest:
                return "openrc"
    if os.path.exists(os.path.join(root, "usr/lib/systemd/systemd")) and not os.path.exists(os.path.join(root, "sbin/openrc")) \
            and not os.path.exists(os.path.join(root, "usr/sbin/openrc")) and not os.path.exists(os.path.join(root, "sbin/openrc-run")):
        return "systemd"
    return "openrc"


def running_init():
    """The init of the running system (same test as the Rust crate::init::detect): systemd iff /run/systemd/system is a directory."""
    forced = os.environ.get("LIVEINST_INIT", "")        # tests (and a chroot with no /run) can pin it
    if forced in INITS:
        return forced
    return "systemd" if os.path.isdir("/run/systemd/system") else "openrc"


def unit_names(svc):
    """systemd unit(s) doing the job of OpenRC service `svc` (a name that already ends in .service/.target/... is returned as is)."""
    if "." in svc and svc.rsplit(".", 1)[1] in ("service", "target", "timer", "socket", "mount", "path"):
        return (svc,)
    if svc in UNIT_MAP:
        return UNIT_MAP[svc]
    return (f"{svc}.service",)


def unit_paths(root, unit):
    return [os.path.join(root, d, unit) for d in ("etc/systemd/system", "usr/lib/systemd/system", "lib/systemd/system")]


def exists(svc, root="/", init=None):
    """True when the service is installed in `root` (OpenRC script or every mapped systemd unit)."""
    init = init or target_init(root)
    _check(init)
    if init == "openrc":
        return os.path.exists(os.path.join(root, "etc/init.d", svc))
    if svc in ("display-manager", "xdm"):
        return display_manager_unit(root) is not None
    units = unit_names(svc)
    if not units:
        return False
    for u in units:
        if "@" in u and not u.endswith("@.service"):
            base = u.split("@", 1)[0] + "@." + u.rsplit(".", 1)[1]
            if not any(os.path.exists(p) for p in unit_paths(root, base)):
                return False
        elif not any(os.path.exists(p) for p in unit_paths(root, u)):
            return False
    return True


def enable_cmds(init, svc, level="default"):
    """argv lists that enable `svc` at boot inside a chroot of the target (level = OpenRC runlevel; systemd has none)."""
    _check(init)
    if init == "openrc":
        return [["rc-update", "add", svc, level]]
    units = unit_names(svc)
    return [["systemctl", "enable", *units]] if units else []


def disable_cmds(init, svc, level="default"):
    _check(init)
    if init == "openrc":
        return [["rc-update", "del", svc, level]]
    units = unit_names(svc)
    return [["systemctl", "disable", *units]] if units else []


def control_cmd(init, action, svc):
    """argv that starts/stops/restarts/queries `svc` on the RUNNING system."""
    _check(init)
    if init == "openrc":
        return ["rc-service", svc, action]
    units = unit_names(svc)
    if action == "status":
        return ["systemctl", "is-active", "--quiet", *(units or (svc,))]
    return ["systemctl", action, *(units or (f"{svc}.service",))]


# ---- installer helpers (inst = system.Installer: .run(argv, chroot=True, check=False), .target) ----

def display_manager_unit(root):
    """the display manager unit installed in `root` (systemd), or None"""
    for u in DISPLAY_MANAGERS:
        if any(os.path.exists(p) for p in unit_paths(root, u)):
            return u
    return None


def enable(inst, svc, level="default"):
    init = target_init(getattr(inst, "target", "/nonexistent"))
    if init == "systemd" and svc in ("display-manager", "xdm"):
        dm = display_manager_unit(inst.target)
        if dm:
            inst.run(["systemctl", "enable", dm], chroot=True, check=False)
            # the image's default target is multi-user (setup-rootfs.sh): display-manager.service is WantedBy=graphical.target,
            # so without this the installed desktop boots to a text login (OpenRC starts it in the default runlevel)
            inst.run(["systemctl", "set-default", "graphical.target"], chroot=True, check=False)
        return
    for cmd in enable_cmds(init, svc, level):
        inst.run(cmd, chroot=True, check=False)


def disable(inst, svc, level="default"):
    init = target_init(getattr(inst, "target", "/nonexistent"))
    for cmd in disable_cmds(init, svc, level):
        inst.run(cmd, chroot=True, check=False)


def installed(inst, svc):
    return exists(svc, getattr(inst, "target", "/nonexistent"))


# ---- live system ----

def control(action, svc, timeout=120):
    """Run a service action on the running system; returns (rc, output)."""
    cmd = control_cmd(running_init(), action, svc)
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout + r.stderr)
    except subprocess.TimeoutExpired:
        return 124, f"{' '.join(cmd)}: timed out after {timeout}s"
    except FileNotFoundError as e:
        return 127, str(e)


def is_running(svc):
    init = running_init()
    rc, out = control("status", svc, timeout=30)
    if init == "openrc":
        return rc == 0 and "started" in out
    return rc == 0


# ---- tty1 / serial console login programs ----

GETTY_DROPIN_DIR = "etc/systemd/system/getty@tty1.service.d"
GETTY_DROPIN = GETTY_DROPIN_DIR + "/10-horcrux-console.conf"


def tty1_dropin_text(agetty_args):
    """getty@tty1 drop-in running agetty with `agetty_args` (everything between `agetty` and the tty name)."""
    return ("# written by live-installer: the tty1 login program (console menu / autologin choice)\n[Service]\nExecStart=\n"
            f"ExecStart=-/sbin/agetty {agetty_args} %I $TERM\n")


def set_tty1(root, agetty_args):
    """Make tty1 run agetty with `agetty_args` on the installed system at `root` (OpenRC: the c1 line of /etc/inittab; systemd: a
    getty@tty1 drop-in). Returns the file written."""
    import re
    init = target_init(root)
    if init == "openrc":
        ip = os.path.join(root, "etc/inittab")
        if os.path.exists(ip):
            t = re.sub(r"(?m)^c1:.*$", f"c1:12345:respawn:/sbin/agetty {agetty_args} 38400 tty1 linux", open(ip).read())
            open(ip, "w").write(t)
        return ip
    d = os.path.join(root, GETTY_DROPIN_DIR)
    os.makedirs(d, exist_ok=True)
    p = os.path.join(root, GETTY_DROPIN)
    with open(p, "w") as f:
        f.write(tty1_dropin_text(agetty_args))
    return p
