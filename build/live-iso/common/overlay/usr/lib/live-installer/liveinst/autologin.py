"""Display-manager / console autologin policy (owner rule 2026-11-03): NO autologin by default, in any image or profile.

A machine whose disk is unlocked by a passphrase must still ask WHO is logging in: the greeter is shown after the unlock and no user
session exists before a greeter login. Autologin exists only as an explicit, opt-in installer choice (answer file `autologin = true`
or the checkbox) and always logs a warning. Everything else is scrubbed from the target so a stale image file cannot enable it.

Pure helpers (unit tested, no root needed): `scrub(root)` removes every autologin setting of LightDM, GDM, SDDM and the agetty
inittab/conf.d entries below `root`; `audit(root)` lists what is still configured (the QEMU gate runs the same scan on the installed disk).
"""
import glob
import os
import re

WARNING = ("WARNING: automatic login was chosen. The disk passphrase is asked at boot, but then anybody holding the machine gets "
           "this user's session without a login password. Turn it off again by deleting the autologin file named in the log.")

LIGHTDM_KEYS = re.compile(r"^\s*(autologin-user|autologin-guest|autologin-session|autologin-user-timeout|autologin-in-background)\s*=\s*(\S.*)$", re.I)
GDM_KEYS = re.compile(r"^\s*(AutomaticLoginEnable|AutomaticLogin|TimedLoginEnable|TimedLogin|TimedLoginDelay)\s*=\s*(\S.*)$", re.I)
SDDM_KEYS = re.compile(r"^\s*(User|Session|Relogin)\s*=\s*(\S.*)$", re.I)       # only inside [Autologin]
OPT_IN_FILE_LIGHTDM = "/etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf"


def _read(p):
    try:
        return open(p, errors="replace").read()
    except OSError:
        return ""


def _files(root):
    out = []
    for pat in ("/etc/lightdm/lightdm.conf", "/etc/lightdm/lightdm.conf.d/*", "/etc/gdm/custom.conf", "/etc/gdm/daemon.conf", "/etc/gdm/custom.conf.d/*",
                "/etc/gdm3/custom.conf", "/etc/sddm.conf", "/etc/sddm.conf.d/*", "/usr/lib/sddm/sddm.conf.d/*", "/etc/inittab", "/etc/conf.d/agetty*"):
        out += [p for p in glob.glob(root + pat) if os.path.isfile(p)]
    return sorted(set(out))


def _active_lines(path, text):
    """(lineno, line) of every ACTIVE autologin setting of one file (comments and disabled values ignored)"""
    hits = []
    section = ""
    base = os.path.basename(path)
    for i, line in enumerate(text.splitlines(), 1):
        s = line.strip()
        if not s or s.startswith(("#", ";")):
            continue
        m = re.match(r"^\[(.+)\]$", s)
        if m:
            section = m.group(1).lower()
            continue
        if "lightdm" in path:
            m = LIGHTDM_KEYS.match(line)
            if m and m.group(1).lower() == "autologin-guest":
                if m.group(2).strip().lower() in ("true", "1", "yes"):
                    hits.append((i, line))
            elif m:
                hits.append((i, line))
        elif "gdm" in path:
            m = GDM_KEYS.match(line)
            if m:
                if m.group(1).lower().endswith("enable") and m.group(2).strip().lower() not in ("true", "1", "yes"):
                    continue
                if m.group(1).lower() in ("automaticlogin", "timedlogin", "timedlogindelay"):
                    continue      # only the *Enable switches matter; the user name alone does nothing and is removed with them
                hits.append((i, line))
        elif "sddm" in path:
            if section == "autologin":
                m = SDDM_KEYS.match(line)
                if m and m.group(1).lower() == "user" and m.group(2).strip():
                    hits.append((i, line))
        elif base == "inittab" or base.startswith("agetty"):
            if re.search(r"--autologin\b", line):
                hits.append((i, line))
    return hits


def audit(root="/"):
    """[(file, lineno, line)] of every active autologin setting below `root`; empty = clean"""
    root = root.rstrip("/")
    out = []
    for p in _files(root):
        for i, line in _active_lines(p, _read(p)):
            out.append((p[len(root):] or p, i, line.strip()))
    return out


def scrub(root="/"):
    """Remove every autologin setting below `root` (opt-in files are deleted, other files lose only the autologin keys). Returns the list of changed files."""
    root = root.rstrip("/")
    changed = []
    for p in _files(root):
        text = _read(p)
        hits = _active_lines(p, text)
        if not hits:
            continue
        rel = p[len(root):]
        if rel.startswith("/etc/lightdm/lightdm.conf.d/") and os.path.basename(p).startswith(("60-horcrux-autologin", "zz-autologin")):
            os.unlink(p)
            changed.append(rel)
            continue
        bad = {i for i, _ in hits}
        lines = text.splitlines(keepends=True)
        new = []
        for i, line in enumerate(lines, 1):
            if i in bad:
                if "gdm" in p:
                    new.append(re.sub(r"=\s*\S.*$", "=false", line.rstrip("\n")) + "\n")        # AutomaticLoginEnable=false
                elif os.path.basename(p) == "inittab" or os.path.basename(p).startswith("agetty"):
                    new.append(re.sub(r"\s+(--autologin|-a)\s+\S+", "", line))
                else:
                    new.append("# " + line.lstrip() if not line.lstrip().startswith("#") else line)
            else:
                new.append(line)
        open(p, "w").write("".join(new))
        changed.append(rel)
    return changed


def enable_lightdm(inst, user, session="xfce"):
    """OPT-IN ONLY. Writes the one well-known file (so `audit` and the docs can point at it) and warns."""
    inst.write(OPT_IN_FILE_LIGHTDM, f"[Seat:*]\nautologin-user={user}\nautologin-user-timeout=0\nautologin-session={session}\n")
    inst.run(["groupadd", "-f", "autologin"], chroot=True, check=False)
    inst.run(["gpasswd", "-a", user, "autologin"], chroot=True, check=False)
    inst.log(WARNING + f" ({OPT_IN_FILE_LIGHTDM})")


def enable_gdm(inst, user):
    inst.write("/etc/gdm/custom.conf", f"[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin={user}\nWaylandEnable=true\n")
    inst.log(WARNING + " (/etc/gdm/custom.conf)")


def enable_console(inst, user):
    ip = f"{inst.target}/etc/inittab"
    if os.path.exists(ip):
        t = _read(ip)
        t = re.sub(r"(?m)^c1:.*$", f"c1:12345:respawn:/sbin/agetty --autologin {user} --noclear 38400 tty1 linux", t)
        open(ip, "w").write(t)
    inst.log(WARNING + " (tty1 in /etc/inittab)")
