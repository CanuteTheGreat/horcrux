#!/usr/bin/env python3
"""Fail when a public overlay file mentions the owner's private Forgejo host.

Everything under gentoo/ is published to the public GitHub mirror and shipped to end users (installed repos.conf,
HOMEPAGE, EGIT_REPO_URI, metadata.xml, installed config files). Public packages point at GitHub or
https://horcrux.errant.solutions/. Private packages are listed in PRIVATE below and are exempt.
Usage: check-no-forgejo.py [overlay_dir]
"""
import os, sys

NEEDLE = "git." + "canutethegreat.com"  # split so this file does not match itself
# category/package dirs that are owner-private (not shipped to end users); keep this list short
PRIVATE = {"net-misc/ctg-ssh-ca"}

root = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
bad = []
for d, _, files in os.walk(root):
    rel = os.path.relpath(d, root).replace(os.sep, "/")
    if any(rel == p or rel.startswith(p + "/") for p in PRIVATE):
        continue
    for f in files:
        path = os.path.join(d, f)
        try:
            text = open(path, encoding="utf-8").read()
        except (UnicodeDecodeError, OSError):
            continue
        for n, line in enumerate(text.splitlines(), 1):
            if NEEDLE in line:
                bad.append(f"{os.path.relpath(path, root)}:{n}: {line.strip()[:120]}")
if bad:
    print(f"FAIL: {NEEDLE} found in public overlay files (use GitHub or https://horcrux.errant.solutions/):")
    print("\n".join(bad))
    sys.exit(1)
print(f"ok: no {NEEDLE} in public overlay files")
