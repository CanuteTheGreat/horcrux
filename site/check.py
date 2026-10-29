#!/usr/bin/env python3
"""Fail if the built site (site/dist) has a broken relative link/asset or a host-root absolute link
(other than the 404 page). Keeps the site movable to another domain with one redirect."""
import os, re, sys
D = os.path.join(os.path.dirname(os.path.abspath(__file__)), "dist")
bad = []; pages = 0
for dp, _, fs in os.walk(D):
    for f in fs:
        if not f.endswith(".html"):
            continue
        pages += 1
        p = os.path.join(dp, f); t = open(p, encoding="utf-8").read()
        for u in re.findall(r'(?:href|src)="([^"]*)"', t):
            if re.match(r"^(?:[a-z][a-z0-9+.-]*:|#|//)", u, re.I):
                continue
            if u.startswith("/") and f != "404.html":
                bad.append((p, u, "host-absolute link")); continue
            path = u.split("#")[0]
            if not path or f == "404.html":
                continue
            tgt = os.path.normpath(os.path.join(dp, path))
            if os.path.isdir(tgt):
                tgt = os.path.join(tgt, "index.html")
            if not os.path.exists(tgt):
                bad.append((p, u, "missing target"))
for b in bad:
    print("BROKEN", *b, file=sys.stderr)
print(f"checked {pages} pages, {len(bad)} problems")
sys.exit(1 if bad else 0)
