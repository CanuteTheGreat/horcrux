#!/usr/bin/env python3
"""Build-time: write /usr/share/live-installer/portage-meta.json.xz from the Portage tree + the installed-package DB.

Runs inside the rootfs build BEFORE the tree is deleted.  Contents (see liveinst/useflags.py):
  desc  - every global USE flag of the tree (profiles/use.desc)
  local - local flag descriptions (profiles/use.local.desc) of the INSTALLED packages only
  pkgs  - per installed package: IUSE, enabled USE, REQUIRED_USE, size, and the USE-dependent
          dependency atoms with [use deps] or blockers (conditions flattened, any-of groups skipped)
"""
import json
import lzma
import os
import re
import subprocess
import sys
import time

REPO = os.environ.get("PORTAGE_REPO", "/var/db/repos/gentoo")
VDB = "/var/db/pkg"
OUT = sys.argv[1] if len(sys.argv) > 1 else "/usr/share/live-installer/portage-meta.json.xz"

EXPAND_PREFIXES = ("python_targets_", "python_single_target_", "ruby_targets_", "video_cards_", "cpu_flags_", "abi_x86_",
                   "abi_", "llvm_slot_", "llvm_targets_", "input_devices_", "l10n_", "linguas_", "lua_targets_",
                   "lua_single_target_", "php_targets_", "qemu_softmmu_targets_", "qemu_user_targets_",
                   "kernel_", "elibc_", "userland_", "ruby_", "perl_features_", "grub_platforms_", "ada_target_",
                   "guile_targets_", "guile_single_target_", "rust_cpu_targets_", "nginx_modules_", "apache2_modules_",
                   "sane_backends_", "gpsd_protocols_", "cameras_", "alsa_cards_", "boost_", "curl_ssl_", "plugins_")


def read(p):
    try:
        with open(p, errors="replace") as f:
            return f.read().strip()
    except OSError:
        return ""


def parse_desc(path):
    out = {}
    for line in read(path).splitlines():
        if line.startswith("#") or " - " not in line:
            continue
        k, _, v = line.partition(" - ")
        out[k.strip()] = v.strip()
    return out


def atom_pkg(atom):
    a = re.sub(r"\[.*\]$", "", atom).lstrip("!~<>=")
    a = re.sub(r":.*$", "", a)
    a = re.sub(r"-\d.*$", "", a)
    return a.rstrip("*")


def constraints(dep, known):
    """[(cond list, atom)] for atoms that carry [use deps] or are blockers; skips || ^^ ?? groups."""
    toks = re.findall(r"\(|\)|[^\s()]+", dep)
    out = []

    def walk(i, cond, skip):
        while i < len(toks):
            t = toks[i]
            i += 1
            if t == ")":
                return i
            if t == "(":
                i = walk(i, cond, skip)
            elif t in ("||", "^^", "??"):
                if i < len(toks) and toks[i] == "(":
                    i += 1
                i = walk(i, cond, True)
            elif t.endswith("?"):
                if i < len(toks) and toks[i] == "(":
                    i += 1
                i = walk(i, cond + [t[:-1]], skip)
            elif not skip and (t.startswith("!") or "[" in t):
                p = atom_pkg(t)
                if p in known:
                    out.append((cond, t))
        return i
    walk(0, [], False)
    return out


def main():
    gdesc = parse_desc(f"{REPO}/profiles/use.desc")
    ldesc_all = {}
    for line in read(f"{REPO}/profiles/use.local.desc").splitlines():
        if line.startswith("#") or " - " not in line:
            continue
        k, _, v = line.partition(" - ")
        pk, _, fl = k.partition(":")
        ldesc_all.setdefault(pk, {})[fl.strip()] = v.strip()
    pkgs = {}
    live_only = {x.lstrip("=") for x in read("/etc/live-installer/live-only-packages").split()}     # graphical installer stack: removed on a Server
    for cat in sorted(os.listdir(VDB)):
        for pf in sorted(os.listdir(f"{VDB}/{cat}")):
            d = f"{VDB}/{cat}/{pf}"
            if not os.path.isdir(d) or f"{cat}/{pf}" in live_only:
                continue
            name = re.sub(r"-\d.*$", "", pf)
            iuse = [x.lstrip("+-") for x in read(d + "/IUSE").split()]
            iuse = [x for x in iuse if not x.startswith(EXPAND_PREFIXES)]
            use = set(read(d + "/USE").split())
            pkgs[f"{cat}/{name}"] = {"v": pf[len(name) + 1:], "iuse": sorted(set(iuse)), "use": sorted(use & set(iuse)),
                                      "ru": read(d + "/REQUIRED_USE"), "size": int(read(d + "/SIZE") or 0),
                                      "_dep": " ".join(read(d + f"/{x}") for x in ("DEPEND", "RDEPEND", "PDEPEND", "IDEPEND"))}
    known = set(pkgs)
    for pk, info in pkgs.items():
        dep = info.pop("_dep")
        cons = constraints(dep, known)
        # keep only constraints whose use-deps mention a flag at all, or blockers
        info["cons"] = [[c, a] for c, a in cons]
        if not info["ru"]:
            del info["ru"]
        if not info["cons"]:
            del info["cons"]
    local = {pk: {f: d for f, d in ldesc_all.get(pk, {}).items() if f in info["iuse"]} for pk, info in pkgs.items()}
    local = {k: v for k, v in local.items() if v}
    gdesc = {k: v for k, v in gdesc.items() if not k.startswith(EXPAND_PREFIXES)}
    meta = {"version": 1, "arch": os.uname().machine, "generated": time.strftime("%FT%TZ", time.gmtime()), "desc": gdesc, "local": local, "pkgs": pkgs}
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    raw = json.dumps(meta, separators=(",", ":"), sort_keys=True).encode()
    with lzma.open(OUT, "wb", preset=9 | lzma.PRESET_EXTREME) as f:
        f.write(raw)
    print(f"portage-meta: {len(gdesc)} global flags, {len(pkgs)} packages, {len(raw) // 1024} KiB raw, {os.path.getsize(OUT) // 1024} KiB xz -> {OUT}")


if __name__ == "__main__":
    main()
