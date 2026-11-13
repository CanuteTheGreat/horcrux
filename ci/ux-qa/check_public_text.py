#!/usr/bin/env python3
"""No private URL / private host in any PUBLIC artifact: UI bundle, docs, packages, tarballs, ISOs' extracted trees, os-release, motd.

Class of bug (owner, 2026-11-12): HOME_URL pointed at the owner's private Forgejo; hostnames leaked into user-visible text.
Generalises gentoo/ci/check-no-forgejo.py (which covers ebuilds) to anything: a file, a directory tree, .tar/.tgz/.tar.zst/.zip.

  fail  = private forge / internal domain / private IP / absolute owner home path          (anywhere)
  warn  = owner machine names (gringotts, hogshead, ...)   -> FAIL under --strict PATH (user-visible: UI bundle, os-release, motd)

usage: check_public_text.py [--strict PATH]... [--allow REGEX]... [--baseline FILE.json] [--report out.json] PATH...
Baseline = {"<relative file>:<rule>": "reason"}; a stale entry is reported so it gets removed.
Binary files are scanned too (strings): a wasm bundle or ELF with an embedded private URL is a leak.
"""
import argparse
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import zipfile

# built from pieces so this file never trips its own scan
_D = "canutethegreat" + "." + "com"
RULES = [
    ("private-forge", "fail", rf"git\.{re.escape(_D)}|forgejo\.[a-z0-9.-]+|{re.escape(_D)}"),
    ("internal-domain", "fail", r"[a-z0-9-]+\.(?:redweek\.internal|internal\.redweek\.com)\b"),
    ("private-ip", "fail", r"\b(?:10\.66\.\d+\.\d+|10\.1\.10\.\d+|172\.30\.\d+\.\d+)\b"),
    ("owner-home-path", "fail", r"/home/(?:canutethegreat|ron)\b|/Users/(?:ron|canute)\w*"),
    ("owner-machine", "warn", r"\b(?:gringotts|hogshead|hogsmeade|madammalkins|crystalcave|ollivanders|azkaban|beauxbatons|rw-man-\d+)\b"),
]
SKIP_DIRS = {".git", "node_modules", "target", "__pycache__", ".venv"}
SELF = {"check_public_text.py", "test_ux_qa.py", "ux-crawl.json", "owner-findings.md"}


def default_rules():
    return [(n, s, re.compile(p, re.I)) for n, s, p in RULES]


def _scan_bytes(name, data, rules, strict, allow, baseline, hits):
    text = data.decode("latin-1")
    if b"\x00" in data[:4096]:  # binary: keep printable runs only
        text = "\n".join(re.findall(r"[\x20-\x7e]{6,}", text))
    for rule, sev, rx in rules:
        m = rx.search(text)
        if not m or any(re.search(a, m.group(0)) or re.search(a, name) for a in allow):
            continue
        s = "fail" if (sev == "warn" and any(name.startswith(x) for x in strict)) else sev
        ln = text.count("\n", 0, m.start()) + 1
        key = f"{name}:{rule}"
        hits.append({"file": name, "rule": rule, "sev": s, "match": m.group(0)[:80], "line": ln, "baselined": key in baseline})


def scan_paths(paths, rules, allow, baseline, strict_prefixes=()):
    hits = []
    strict = tuple(strict_prefixes)

    def walk_file(p, display):
        if os.path.basename(p) in SELF:
            return
        low = p.lower()
        try:
            if low.endswith((".tar.gz", ".tgz", ".tar", ".tar.xz", ".tar.bz2")):
                with tarfile.open(p) as t:
                    for ti in t:
                        if ti.isfile() and ti.size < 64 << 20:
                            _scan_bytes(f"{display}!{ti.name}", t.extractfile(ti).read(), rules, strict, allow, baseline, hits)
            elif low.endswith((".tar.zst", ".tzst")):
                out = subprocess.run(["zstd", "-dc", p], capture_output=True, check=True).stdout
                with tarfile.open(fileobj=io.BytesIO(out)) as t:
                    for ti in t:
                        if ti.isfile() and ti.size < 64 << 20:
                            _scan_bytes(f"{display}!{ti.name}", t.extractfile(ti).read(), rules, strict, allow, baseline, hits)
            elif low.endswith((".zip", ".apk", ".whl")):
                with zipfile.ZipFile(p) as z:
                    for n in z.namelist():
                        _scan_bytes(f"{display}!{n}", z.read(n), rules, strict, allow, baseline, hits)
            else:
                if os.path.getsize(p) < 256 << 20:
                    _scan_bytes(display, open(p, "rb").read(), rules, strict, allow, baseline, hits)
        except (OSError, tarfile.TarError, zipfile.BadZipFile, subprocess.CalledProcessError) as e:
            hits.append({"file": display, "rule": "unreadable", "sev": "warn", "match": str(e)[:80], "line": 0, "baselined": False})

    for root in paths:
        if os.path.isdir(root):
            for d, ds, fs in os.walk(root):
                ds[:] = [x for x in ds if x not in SKIP_DIRS]
                for f in fs:
                    p = os.path.join(d, f)
                    if os.path.islink(p):
                        continue
                    walk_file(p, p)
        else:
            walk_file(root, root)
    return hits


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--strict", action="append", default=[])
    ap.add_argument("--allow", action="append", default=[])
    ap.add_argument("--baseline")
    ap.add_argument("--report")
    a = ap.parse_args()
    baseline = json.load(open(a.baseline)) if a.baseline and os.path.exists(a.baseline) else {}
    hits = scan_paths(a.paths, default_rules(), a.allow, baseline, a.strict)
    bad = [h for h in hits if h["sev"] == "fail" and not h["baselined"]]
    for h in hits:
        tag = "KNOWN" if h["baselined"] else h["sev"].upper()
        print(f"{tag:5} {h['file']}:{h['line']} [{h['rule']}] {h['match']}")
    stale = sorted(set(baseline) - {f"{h['file']}:{h['rule']}" for h in hits})
    if stale:
        print("note: stale baseline entries (remove): " + ", ".join(stale))
    if a.report:
        json.dump({"fail": len(bad), "hits": hits}, open(a.report, "w"), indent=1)
    print(f"check_public_text: {len(bad)} new fail, {sum(h['sev'] == 'warn' for h in hits)} warn")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
