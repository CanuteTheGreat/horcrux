#!/usr/bin/env python3
"""Fetch Alpine packages and their dependencies for an offline guest.

Usage: alpine_apks.py MIRROR BRANCH OUTDIR PKG...
  MIRROR  e.g. https://dl-cdn.alpinelinux.org/alpine
  BRANCH  e.g. v3.24 (the branch of the ISO the guest boots)

Reads APKINDEX of main and community for x86_64, resolves the run-time
dependencies (package names, `so:` and `cmd:` provides) the live system
does not already carry, and downloads the .apk files into OUTDIR. Used by
the console e2e to give a network-less Alpine guest `alsa-utils` from a
data CD (the test VM has no NIC). Each file's size is checked against
the index.
"""
import io
import os
import sys
import tarfile
import time
import urllib.request

MIRROR, BRANCH, OUT = sys.argv[1].rstrip("/"), sys.argv[2], sys.argv[3]
WANT = sys.argv[4:]
ARCH = "x86_64"
# already in the live system (busybox/musl base); never fetched
BASE = {"musl", "busybox", "so:libc.musl-x86_64.so.1", "/bin/sh"}


def get(url):
    for i in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "horcrux-e2e/1.0"}),
                                        timeout=60) as r:
                return r.read()
        except Exception:
            if i == 3:
                raise
            time.sleep(3 * (i + 1))


pkgs, provides = {}, {}
for repo in ("main", "community"):
    raw = get(f"{MIRROR}/{BRANCH}/{repo}/{ARCH}/APKINDEX.tar.gz")
    with tarfile.open(fileobj=io.BytesIO(raw)) as t:
        text = t.extractfile("APKINDEX").read().decode()
    for block in text.split("\n\n"):
        f = {}
        for line in block.splitlines():
            if len(line) > 2 and line[1] == ":":
                f[line[0]] = line[2:]
        if "P" not in f or f["P"] in pkgs:
            continue
        f["repo"] = repo
        pkgs[f["P"]] = f
        provides.setdefault(f["P"], f["P"])
        for p in f.get("p", "").split():
            provides.setdefault(p.split("=")[0], f["P"])


def name(dep):
    for op in ("<", ">", "=", "~"):
        dep = dep.split(op)[0]
    return dep


todo, chosen = list(WANT), []
while todo:
    d = name(todo.pop())
    if d.startswith("!") or d in BASE:
        continue
    p = provides.get(d)
    if p is None:
        raise SystemExit(f"alpine_apks: nothing provides {d}")
    if p in chosen or p in BASE:
        continue
    chosen.append(p)
    todo.extend(pkgs[p].get("D", "").split())

os.makedirs(OUT, exist_ok=True)
for p in chosen:
    f = pkgs[p]
    fn = f"{p}-{f['V']}.apk"
    data = get(f"{MIRROR}/{BRANCH}/{f['repo']}/{ARCH}/{fn}")
    # S: is the package file's size (C: covers only its control segment)
    if f.get("S") and int(f["S"]) != len(data):
        raise SystemExit(f"alpine_apks: {fn}: size {len(data)} != {f['S']}")
    with open(os.path.join(OUT, fn), "wb") as fh:
        fh.write(data)
    print(fn)
