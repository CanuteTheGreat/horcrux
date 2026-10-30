#!/usr/bin/env python3
"""portage-orphans.py [ROOT]: files under /usr /etc /opt /boot /bin /sbin /lib* that NO installed Portage package owns.

`qfile -o` compares the literal path with the CONTENTS records, so on a merged-/usr system (/lib -> usr/lib, /lib64 -> usr/lib64,
/sbin -> usr/bin) every file recorded as /lib64/x is reported as an orphan when asked as /usr/lib64/x. This script resolves the
directory part of every path (CONTENTS entries and files on disk) before comparing.

Survivors are classified:
  GENERATED  files that no ebuild can own by nature: written by the installer/first boot (passwd, fstab, machine-id, initramfs, grub,
             host keys), by Portage/eselect tooling (profile.env, ld.so.cache, make.conf, gnupg keyring), or caches built by
             post-install hooks (mime database, icon-theme.cache, locale-archive, gconv cache)
  ALTLINK    symlinks created by eselect/gcc-config/binutils-config/pinentry/iptables selectors whose target is a Portage-owned file
  ORPHAN     everything else = a file somebody dropped in without an ebuild (must be empty)
Output: `CLASS path` lines; exit status 1 when ORPHAN lines exist (or the summary line with --summary).
"""
import os
import re
import sys

args = [a for a in sys.argv[1:] if not a.startswith("--")]
root = (args[0] if args else "/").rstrip("/") or "/"
db = os.path.join(root, "var/db/pkg")

GENERATED = [re.compile(x) for x in (
    r"^/etc/(passwd|shadow|group|gshadow)-?$", r"^/etc/(subuid|subgid|fstab|crypttab|hostname|machine-id|adjtime|mtab|resolv\.conf(\.bak)?|ld\.so\.cache|ld\.so\.conf|localtime|timezone|profile\.env|csh\.env|\.pwd\.lock|securetty|motd|environment|hostid|machine-info)$",
    r"^/etc/ssh/ssh_host_.*$", r"^/etc/conf\.d/(hostname|keymaps|display-manager|xdm|net|hwclock|consolefont)$", r"^/etc/runlevels/.*$",
    r"^/etc/env\.d/(.*)$", r"^/etc/environment\.d/.*$", r"^/etc/dracut\.conf\.d/.*$", r"^/etc/lightdm/lightdm\.conf\.d/60-horcrux-autologin\.conf$",
    r"^/etc/elogind/logind\.conf\.d/10-horcrux-portable\.conf$", r"^/etc/elogind/sleep\.conf\.d/.*$", r"^/etc/UPower/UPower\.conf$", r"^/etc/sudoers\.d/.*$", r"^/etc/NetworkManager/system-connections/.*$",
    r"^/etc/portage/(make\.conf|make\.conf\.arch|make\.profile|\.gitkeep|binrepos\.conf/.*|gnupg/.*|env/.*|package\.env/.*|package\.accept_keywords/arch|package\.mask/zfs)$",
    r"^/etc/live-iso-(volid|label|variant|cmdline|autoentry|hwprofile)$", r"^/etc/surface-kernelrelease$", r"^/etc/local\.d/50-installer-firstboot\.start$",
    r"^/etc/(ssl/certs/.*|ca-certificates\.conf)$", r"^/etc/\._cfg.*$", r"^/etc/ssh/sshd_config(\.d/.*)?$", r"^/etc/inittab$", r"^/etc/default/grub$", r"^/etc/hosts$",
    r"^/etc/(kernel/.*|modprobe\.d/.*|modules-load\.d/.*|sysctl\.d/.*)$", r"^/etc/conf\.d/(zram-swap|horcrux-api)$", r"^/etc/udev/hwdb\.bin$",
    r"^/boot/(initramfs-.*|grub/.*|EFI/.*|System\.map.*|config-.*|vmlinuz-.*)$", r"^/etc/xml/.*$", r"^/etc/ld\.so\.conf\.d/.*$", r"^/etc/iwd/main\.conf$", r"^/etc/live-installer/live-only-packages$",
    r"^/usr/lib/modules/[^/]+/modules\..*$", r"^/usr/lib/locale/locale-archive$", r"^/usr/lib64/gconv/gconv-modules\.cache$",
    r"^/usr/lib64?/gdk-pixbuf-2\.0/.*/loaders\.cache$", r"^/usr/lib64?/gio/modules/giomodule\.cache$", r"^/usr/lib64?/gtk-[234]\.0/.*/immodules\.cache$",
    r"^/usr/lib/python[0-9.]+/.*__pycache__/.*$", r"^/usr/lib/cpp$",
    r"^/usr/share/mime/.*$", r"^/usr/share/icons/[^/]+/icon-theme\.cache$", r"^/usr/share/applications/mimeinfo\.cache$", r"^/usr/share/glib-2\.0/schemas/gschemas\.compiled$",
    r"^/usr/share/(info/dir|.*/info/dir)$", r"^/usr/share/fonts/.*/(fonts\.dir|fonts\.scale)$", r"^/usr/share/live-installer/portage-meta\.json\.xz$",
    r"^/usr/lib/\.build-id/.*$", r"^/usr/share/man/man1/.*\.1(\.bz2|\.gz|\.xz|\.zst)?$",
)]


def norm(p):
    d, b = os.path.split(p)
    return os.path.join(os.path.realpath(root + d) if root != "/" else os.path.realpath(d), b)


owned = set()
for cat in os.listdir(db):
    for pkg in os.listdir(os.path.join(db, cat)):
        try:
            lines = open(os.path.join(db, cat, pkg, "CONTENTS"), errors="replace").read().splitlines()
        except OSError:
            continue
        for ln in lines:
            t = ln.split(" ")
            if t[0] == "obj" and len(t) > 3:
                owned.add(norm(" ".join(t[1:-2])))
            elif t[0] == "sym" and "->" in t:
                owned.add(norm(" ".join(t[1:t.index("->")])))
counts = {"GENERATED": 0, "ALTLINK": 0, "ORPHAN": 0}
total = 0
for top in ("usr", "etc", "opt", "boot", "bin", "sbin", "lib", "lib64"):
    start = os.path.join(root, top)
    if os.path.islink(start) or not os.path.isdir(start):
        continue
    for dp, dn, fn in os.walk(start):
        for n in fn:
            p = os.path.join(dp, n)
            q = p[len(root):] if root != "/" else p
            total += 1
            if norm(q) in owned:
                continue
            if any(g.match(q) for g in GENERATED):
                cls = "GENERATED"
            elif os.path.islink(p):
                # follow the chain of symlinks: it must end in an owned file
                t, hops = p, 0
                while os.path.islink(t) and hops < 8:
                    nt = os.readlink(t)
                    t = nt if nt.startswith("/") else os.path.join(os.path.dirname(t), nt)
                    t = (root + t) if (nt.startswith("/") and root != "/") else t
                    hops += 1
                tq = t[len(root):] if root != "/" and t.startswith(root) else t
                cls = "ALTLINK" if (norm(os.path.normpath(tq)) in owned) else "ORPHAN"
            else:
                cls = "ORPHAN"
            counts[cls] += 1
            print(cls, q)
print(f"SUMMARY files={total} owned={total - sum(counts.values())} generated={counts['GENERATED']} altlink={counts['ALTLINK']} orphan={counts['ORPHAN']}")
sys.exit(1 if counts["ORPHAN"] else 0)
