#!/usr/bin/env python3
"""Fail if the built site (site/dist) has a broken relative link/asset or a host-root absolute link
(other than the 404 page). Keeps the site movable to another domain with one redirect."""
import os, re, sys
# Personal hosts must never appear on the public site (owner rule): guard over EVERY built file and the nginx config.
PERSONAL = re.compile(r"git\.canutethegreat\.com|(?:[a-z0-9-]+\.)*canutethegreat\.(?:com|one)|forgejo\.canutethegreat", re.I)
HERE = os.path.dirname(os.path.abspath(__file__))
# /download/... links have no file in dist: they must match a redirect location in nginx.conf
DL_RE = [re.compile(x.replace("(?<", "(?P<")) for x in
         re.findall(r"location ~ (\^/download/\S+) \{", open(os.path.join(HERE, "nginx.conf")).read())]
D = os.path.join(os.path.dirname(os.path.abspath(__file__)), "dist")
bad = []; pages = 0
for dp, _, fs in os.walk(D):
    for f in fs:
        p = os.path.join(dp, f)
        try: t = open(p, encoding="utf-8").read()
        except UnicodeDecodeError: continue   # binary assets
        for m in PERSONAL.finditer(t):
            bad.append((p, m.group(0), "personal host"))
for f in ("nginx.conf", "Dockerfile", "site.json"):
    for m in PERSONAL.finditer(open(os.path.join(HERE, f), encoding="utf-8").read()):
        bad.append((f, m.group(0), "personal host"))
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
            rp = os.path.relpath(os.path.normpath(os.path.join(dp, path)), D) if path else ""
            if rp.startswith("download" + os.sep) or rp == "download":
                if not any(r.match("/" + rp.replace(os.sep, "/")) for r in DL_RE):
                    bad.append((p, u, "download link without nginx redirect"))
                continue
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
