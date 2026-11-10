#!/usr/bin/env python3
"""orphan-scan: files under the system directories that no installed Portage package owns (merged-/usr aware: qfile compares raw
paths, so /usr/bin/bash vs the recorded /bin/bash look like an orphan). Prints one path per line.
  orphan-scan.py [--roots /etc /usr ...]"""
import glob
import os
import sys

ROOTS = ["/etc", "/usr", "/opt", "/boot", "/var/lib", "/var/db/repos/horcrux-gringotts", "/root", "/home", "/srv"]
if "--roots" in sys.argv:
    ROOTS = sys.argv[sys.argv.index("--roots") + 1:]
owned = set()
for c in glob.glob("/var/db/pkg/*/*/CONTENTS"):
    for line in open(c, errors="replace"):
        parts = line.rstrip("\n").split(" ")
        if len(parts) < 2 or parts[0] not in ("obj", "sym", "dev", "fif"):
            continue
        # obj <path> <md5> <mtime> | sym <path> -> <target> <mtime>
        if parts[0] == "sym":
            path = line[4:].split(" -> ")[0]
        else:
            path = " ".join(parts[1:-2]) if len(parts) > 3 else parts[1]
        d, b = os.path.split(path)
        owned.add(os.path.join(os.path.realpath(d), b))
dev0 = {}
for root in ROOTS:
    if not os.path.isdir(root):
        continue
    for dp, dns, fns in os.walk(root):
        rp = os.path.realpath(dp)
        dns[:] = [d for d in dns if not os.path.ismount(os.path.join(dp, d))]
        for fn in fns:
            p = os.path.join(rp, fn)
            if p not in owned:
                print(p)
