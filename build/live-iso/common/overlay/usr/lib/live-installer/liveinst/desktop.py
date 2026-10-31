"""Desktop profile (GNOME on Wayland) for the installer.

The desktop packages are part of the Desktop image variant (build.sh VARIANT=desktop); the installer only
creates the user and wires the services from /usr/share/horcrux/desktop/services.list, so installing needs no
network. Lines of services.list:  <service> <runlevel>   (# comments).
"""
import os
import re

DIR = "/usr/share/horcrux/desktop"
MARKER = DIR + "/installed"
SERVICES = DIR + "/services.list"
GROUPS = ["wheel", "audio", "video", "usb", "plugdev", "input", "render", "cdrom", "lp", "lpadmin",
          "libvirt", "kvm", "users", "pipewire"]
# services of the server image that a desktop replaces (NetworkManager owns the network, see services.list)
REPLACED = ["dhcpcd", "iwd"]


def available(root="/"):
    return os.path.exists(os.path.join(root, MARKER.lstrip("/")))


def user_problem(name):
    if not re.fullmatch(r"[a-z_][a-z0-9_-]{0,30}", name or ""):
        return "User name: lower-case letters, digits, '-' and '_' (max 31)."
    if name in ("root", "admin", "daemon", "bin", "nobody"):
        return "That user name is reserved."
    return None


def parse_services(text):
    out = []
    for line in text.splitlines():
        line = line.split("#")[0].strip()
        if line:
            parts = line.split()
            out.append((parts[0], parts[1] if len(parts) > 1 else "default"))
    return out


def apply(inst):
    from .system import InstallError
    c, T = inst.c, inst.target
    if not available(T):
        raise InstallError("this image has no desktop environment: use the Horcrux Desktop image")
    name = c.get("user_name", "")
    prob = user_problem(name)
    if prob:
        raise InstallError("desktop profile needs a user account: " + prob)
    if not (c.get("user_hash") or c.get("user_password")):
        raise InstallError("desktop profile needs a password for " + name)
    have = set(l.split(":")[0] for l in open(f"{T}/etc/group"))
    groups = ",".join(g for g in GROUPS if g in have)
    # the home dataset is mounted above the root BE by zfsroot.mount()/mount_rest() (they fail the install
    # otherwise): a home created in the BE's own /home would be hidden by the empty home dataset at boot
    inst.run(["useradd", "-m", "-G", groups, "-s", "/bin/bash", "-c", c.get("user_fullname", name), name], chroot=True)
    if c.get("user_hash"):
        inst.run(["chpasswd", "-e"], chroot=True, input_=f"{name}:{c['user_hash']}\n")
    else:
        inst.run(["chpasswd"], chroot=True, input_=f"{name}:{c['user_password']}\n")
    for s in REPLACED:
        inst.run(["rc-update", "del", s, "default"], chroot=True, check=False)
    if os.path.exists(T + SERVICES):
        for svc, lvl in parse_services(open(T + SERVICES).read()):
            if os.path.exists(f"{T}/etc/init.d/{svc}"):
                inst.run(["rc-update", "add", svc, lvl], chroot=True, check=False)
            else:
                inst.log(f"desktop: service {svc} not in the image, skipped")
    # OpenRC's display-manager service reads this (xdm is only a compat alias); gdm is the GNOME display manager
    inst.write("/etc/conf.d/display-manager", 'DISPLAYMANAGER="gdm"\n')
    inst.write("/etc/conf.d/xdm", 'DISPLAYMANAGER="gdm"\n')
    # wired + Wi-Fi by NetworkManager (iwd backend) from now on
    inst.write("/etc/NetworkManager/conf.d/10-horcrux.conf", "[device]\nwifi.backend=iwd\n")
    # the iwd backend only autoconnects networks iwd knows: turn every Wi-Fi profile (joined on the live system or carried over) into /var/lib/iwd/<ssid>.psk
    from . import system as _system
    for p in _system.carry_wifi_to_iwd(T, sources=("/etc/NetworkManager/system-connections", T + "/etc/NetworkManager/system-connections")):
        inst.log(f"desktop: Wi-Fi network carried over to iwd: {os.path.basename(p)}")
    if c.get("user_autologin"):
        inst.write("/etc/gdm/custom.conf", f"[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin={name}\nWaylandEnable=true\n")
    else:
        inst.write("/etc/gdm/custom.conf", "[daemon]\nWaylandEnable=true\n")
    # the GNOME defaults package (AppIndicator tray icons enabled system-wide) belongs to @world of every GNOME install, so later `emerge -uDN @world` keeps it
    pk = f"{T}/var/db/pkg/gnome-extra"
    if os.path.isdir(pk) and any(d.startswith("horcrux-gnome-defaults-") for d in os.listdir(pk)):
        w = f"{T}/var/lib/portage/world"
        if "gnome-extra/horcrux-gnome-defaults" not in (open(w).read().split() if os.path.exists(w) else []):
            inst.write("/var/lib/portage/world", "gnome-extra/horcrux-gnome-defaults\n", append=True)
    inst.log(f"desktop profile: user {name}, GNOME/GDM on Wayland")
