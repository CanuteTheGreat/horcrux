#!/usr/bin/env python3
"""print cat/pkg:slot of every installed package that owns /usr/include, LLVM headers/tools or clang files (reads /var/db/pkg)"""
import glob
import os
import re
import portage.versions as v

pat = re.compile(r"^(obj|sym|dir) /usr/(include|lib/llvm/[0-9]+/(include|bin)|lib/clang)")
out = set()
for c in glob.glob("/var/db/pkg/*/*/CONTENTS"):
    with open(c, errors="replace") as f:
        if any(pat.match(l) for l in f):
            d = os.path.dirname(c)[len("/var/db/pkg/"):]
            cp = v.catpkgsplit(d)
            slot = open(os.path.dirname(c) + "/SLOT").read().strip().split("/")[0]
            out.add(f"{cp[0]}/{cp[1]}:{slot}")
print(" ".join(sorted(out)))
