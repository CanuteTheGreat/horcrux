#!/usr/bin/env python3
"""Fail when an ebuild in the overlay fetches a distfile that has no DIST line in its package's Manifest.

Thin manifests (DIST only) are committed; a missing one makes the ebuild uninstallable with
'VERIFY FAILED: Insufficient data for checksum verification'. Skips RESTRICT=fetch ebuilds (their
local tarballs are manifested at image build time) and pure-comment mentions of SRC_URI.
Usage: check-manifests.py [overlay_dir]   (default: the gentoo/ dir next to this script's parent)
"""
import os, re, sys

root = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
bad = []

def src_uri(text):
    """Concatenated bodies of every SRC_URI= / SRC_URI+= assignment (comments stripped)."""
    text = "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("#"))
    return [m.group(2) for m in re.finditer(r'^\s*SRC_URI\s*(\+?)=\s*"(.*?)"', text, re.S | re.M)] + \
           [m.group(1) for m in re.finditer(r"^\s*SRC_URI\s*\+?=\s*'(.*?)'", text, re.S | re.M)]

for d, _, files in sorted(os.walk(root)):
    for f in sorted(files):
        if not f.endswith(".ebuild"):
            continue
        path = os.path.join(d, f)
        text = open(path).read()
        if re.search(r'^\s*RESTRICT\s*\+?=\s*".*\bfetch\b', text, re.M):
            continue
        bodies = src_uri(text)
        # crate-based ebuilds (inherit cargo + CRATES) fetch through the eclass SRC_URI
        crates = bool(re.search(r'^inherit\b.*\bcargo\b', text, re.M)) and re.search(r'^\s*CRATES\s*=\s*"\s*[^"\s]', text, re.M)
        if not any(re.search(r'\S', b) for b in bodies) and not crates:
            continue
        pn = os.path.basename(d)
        pf = f[:-len(".ebuild")]
        pv = pf[len(pn) + 1:]
        pvr = pv
        pv = re.sub(r'-r\d+$', '', pv)
        mf = os.path.join(d, "Manifest")
        dist = {}
        if os.path.exists(mf):
            for l in open(mf):
                p = l.split()
                if len(p) >= 3 and p[0] == "DIST":
                    dist[p[1]] = p[2]
        rel = os.path.relpath(path, root)
        if not dist:
            bad.append(f"{rel}: SRC_URI but no DIST entries (Manifest {'empty' if os.path.exists(mf) else 'missing'}; is it gitignored?)")
            continue
        subs = {"P": f"{pn}-{pv}", "PN": pn, "PV": pv, "PF": pf, "PVR": pvr}
        for body in bodies:
            body = re.sub(r'\w+\?\s*\(|\)|\|\|', ' ', body)   # USE conditionals
            toks = body.split()
            names = []
            for i, t in enumerate(toks):
                if t == "->" and i + 1 < len(toks):
                    names[-1:] = [toks[i + 1]]
                elif "://" in t:
                    names.append(t.rsplit("/", 1)[-1])
            for n in names:
                n = re.sub(r'\$\{(\w+)\}', lambda m: subs.get(m.group(1), m.group(0)), n)
                if "${" in n or "$" in n:
                    continue  # needs ebuild variables we cannot resolve here; the any-DIST check above still applies
                if n not in dist:
                    bad.append(f"{rel}: distfile '{n}' has no DIST line in {os.path.relpath(mf, root)}")
for b in bad:
    print("FAIL", b)
print(f"{len(bad)} problem(s)")
sys.exit(1 if bad else 0)
