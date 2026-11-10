#!/usr/bin/python3
"""print the installed packages that zfs/zfs-kmod need headers/libs from at build time (DEPEND + BDEPEND, flags ignored).

The gnome-light layer strips /usr/include (and static libs) to fit the ISO under 2 GiB; Portage keeps the records, the files are gone, so
`emerge --nodeps` of exactly these packages puts their development files back (binary packages, no source build) before zfs is compiled
against the T2 kernel. The same list is what a later `emerge zfs` on an installed system needs, which is why the installer's target keeps them."""
import re
import subprocess
import sys

atoms = set()
for pkg in sys.argv[1:] or ["sys-fs/zfs-2.3.9", "sys-fs/zfs-kmod-2.3.9"]:
    for var in ("DEPEND", "BDEPEND", "RDEPEND"):
        out = subprocess.run(["portageq", "metadata", "/", "ebuild", pkg, var], capture_output=True, text=True).stdout
        for m in re.finditer(r"(?<![\w/+-])((?:[a-z0-9]+-[a-z0-9]+|virtual)/[A-Za-z0-9_+][A-Za-z0-9_+.-]*?)(?=[\s:\[\])]|-[0-9]|$)", out):
            atoms.add(m.group(1))
# libzfs_crypto.c includes curl/curl.h whatever the ebuild says (key URLs); pam/util-linux/udev are linked unconditionally
atoms |= {"net-misc/curl", "sys-libs/pam", "sys-apps/util-linux"}
inst = subprocess.run(["qlist", "-IC"], capture_output=True, text=True).stdout.split()
atoms |= {"sys-apps/systemd-utils"} if {"virtual/udev", "virtual/libudev"} & atoms else set()     # libudev.h
print(" ".join(sorted(a for a in atoms if a in inst and not a.startswith("virtual/") and not a.startswith(("sys-kernel/", "sys-fs/zfs")))))
