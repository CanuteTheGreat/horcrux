#!/usr/bin/env python3
"""os-release sanity for an INSTALLED system (or any artifact that ends up as one).

Class of bug (owner, 2026-11-12): an installed system's /etc/os-release said "live/installer", carried the hostname as
VARIANT, said "GNOME Light" (an internal build name) and pointed HOME_URL at the owner's private Forgejo.

usage: check_os_release.py [--product horcrux|patronus] [--live] [--hostname NAME] FILE_OR_ROOTFS...
  FILE_OR_ROOTFS   an os-release file, or a root directory (etc/os-release / usr/lib/os-release inside it are checked)
  --live           the live medium itself may say live/installer (skips that rule only)
  --hostname       a machine name that must not appear anywhere (repeatable); the owner machine names are always banned
Exit 1 on any violation; one line per violation.  Also importable: check(text, product, live, hostnames) -> [problems].
"""
import argparse
import os
import re
import sys

PUBLIC_URL = {
    "horcrux": r"^https://(horcrux\.errant\.solutions(/|$)|github\.com/CanuteTheGreat/horcrux(/|$))",
    "patronus": r"^https://(patronus\.errant\.solutions(/|$)|github\.com/CanuteTheGreat/patronus(/|$))",
}
EDITIONS = {"server", "gnome", "xfce", "desktop", "router", "firewall", "appliance"}
MACHINES = ("hogsmeade", "hogshead", "gringotts", "madammalkins", "surface", "test-machine", "test machine", "ollivanders", "crystalcave",
            "azkaban", "beauxbatons", "rw-man")


def parse(text):
    kv = {}
    for m in re.finditer(r'^([A-Z_0-9]+)=(?:"([^"]*)"|\'([^\']*)\'|(\S*))?', text, re.M):
        kv[m.group(1)] = m.group(2) or m.group(3) or m.group(4) or ""
    return kv


def check(text, product="horcrux", live=False, hostnames=(), need_version=True):
    kv, bad = parse(text), []
    low = text.lower()
    for k in ("NAME", "ID", "PRETTY_NAME"):
        if not kv.get(k):
            bad.append(f"{k} missing")
    if kv.get("ID") and kv["ID"] != product:
        bad.append(f"ID={kv['ID']} (expected {product})")
    if need_version and not (kv.get("VERSION_ID") or kv.get("BUILD_ID") or kv.get("VERSION")):
        bad.append("no VERSION_ID/BUILD_ID: the installed system cannot say which build it is")
    user_facing = " ".join(kv.get(k, "") for k in ("NAME", "PRETTY_NAME", "VARIANT", "VERSION", "VARIANT_ID"))
    if not live and re.search(r"\b(live|installer|iso)\b", user_facing, re.I):
        bad.append(f"live/installer wording on an installed system: {user_facing.strip()!r}")
    if re.search(r"\blight\b|gnome-light|openrc|x86[_-]64|aarch64|\bdaily\b", user_facing, re.I):
        bad.append(f"internal build name in user-facing fields: {user_facing.strip()!r}")
    names = [m for m in MACHINES + tuple(h.lower() for h in hostnames if h)]
    for m in names:
        if m in low:
            bad.append(f"machine/host name '{m}' in os-release (VARIANT is the edition only)")
    if kv.get("VARIANT_ID") and kv["VARIANT_ID"] not in EDITIONS and kv["VARIANT_ID"] not in ("gnome-light",):
        bad.append(f"VARIANT_ID={kv['VARIANT_ID']} is not an edition ({', '.join(sorted(EDITIONS))})")
    for k in ("HOME_URL", "DOCUMENTATION_URL", "SUPPORT_URL", "BUG_REPORT_URL"):
        v = kv.get(k)
        if v and not re.search(PUBLIC_URL.get(product, PUBLIC_URL["horcrux"]), v):
            bad.append(f"{k}={v} is not a public {product} URL")
        if v and re.search(r"canutethegreat\.com|redweek|\b10\.\d+\.\d+\.\d+|localhost|\.internal|\.home\b", v):
            bad.append(f"{k}={v} points at a private host")
    if re.search(r"git\.canutethegreat\.com|forgejo", low):
        bad.append("private Forgejo URL / name in os-release")
    return bad


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--product", default="horcrux")
    ap.add_argument("--live", action="store_true")
    ap.add_argument("--no-version", action="store_true", help="do not require VERSION_ID/BUILD_ID (products that do not stamp one yet)")
    ap.add_argument("--hostname", action="append", default=[])
    a = ap.parse_args()
    rc = 0
    for p in a.paths:
        files = [p]
        if os.path.isdir(p):
            files = [os.path.join(p, f) for f in ("etc/os-release", "usr/lib/os-release") if os.path.exists(os.path.join(p, f))]
            if not files:
                print(f"FAIL {p}: no etc/os-release or usr/lib/os-release")
                rc = 1
        for f in files:
            probs = check(open(f, errors="replace").read(), a.product, a.live, a.hostname, not a.no_version)
            for pr in probs:
                print(f"FAIL {f}: {pr}")
            rc |= bool(probs)
            if not probs:
                print(f"ok   {f}")
    sys.exit(rc)


if __name__ == "__main__":
    main()
