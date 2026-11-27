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
# --- compliance pages: every page's footer links the privacy page, the support policy and the licenses ---
for need in ("docs/privacy/index.html", "docs/update-policy/index.html", "licenses/THIRD-PARTY-LICENSES.txt"):
    if not os.path.exists(os.path.join(D, need)):
        bad.append((need, "", "compliance page missing"))
for dp, _, fs in os.walk(D):
    for f in fs:
        if not f.endswith(".html"):
            continue
        p = os.path.join(dp, f); t = open(p, encoding="utf-8").read()
        if f == "404.html":
            continue
        for tail in ("privacy/", "update-policy/", "licenses/THIRD-PARTY-LICENSES.txt"):   # links are relative, so match the tail
            if tail in p.replace(os.sep, "/"):
                continue   # the page itself: its footer link is "./"
            if not re.search(r'<footer>.*href="[^"]*' + re.escape(tail) + '"', t, re.S):
                bad.append((p, tail, "footer link missing"))
_pv = open(os.path.join(D, "docs/privacy/index.html"), encoding="utf-8").read() if os.path.exists(os.path.join(D, "docs/privacy/index.html")) else ""
if "<title>Privacy" not in _pv or "<script" in _pv:
    bad.append(("docs/privacy/index.html", "", "privacy page needs its own title and no script"))
_lic = os.path.join(D, "licenses/THIRD-PARTY-LICENSES.txt")
if os.path.exists(_lic) and os.path.getsize(_lic) < 10000:
    bad.append((_lic, "", "license inventory suspiciously small"))
# --- shared donation component: give.py + give.css must match give.lock (the same hash in horcrux and patronus) ---
import hashlib
_h = hashlib.sha256()
for _f in ("give.py", "give.css"):
    _h.update(open(os.path.join(HERE, _f), "rb").read())
_lock = open(os.path.join(HERE, "give.lock")).read().split()[0]
if _h.hexdigest() != _lock:
    bad.append(("give.py/give.css", _h.hexdigest(), "shared donation component changed: copy it to BOTH product repos and update give.lock in both"))
_ix = open(os.path.join(D, "index.html"), encoding="utf-8").read() if os.path.exists(os.path.join(D, "index.html")) else ""
if _ix.count('class="wrap givebar"') != 1 or _ix.count('<div class="card">') < 4 or 'id="support"' not in _ix or 'class="give-foot"' not in _ix:
    bad.append(("index.html", "", "donation strip, #support cards or footer line missing"))
for b in bad:
    print("BROKEN", *b, file=sys.stderr)
print(f"checked {pages} pages, {len(bad)} problems")

# --- gating unit checks (fake asset lists; no network) ---
sys.path.insert(0, HERE)
import gating
_o = "horcrux-server-openrc-daily-x86_64.iso"; _s = "horcrux-server-systemd-daily-x86_64.iso"
_cases = [
    (set(), False), (None, False),
    ({_o, _o + ".sha256"}, False),                 # only OpenRC published
    ({_s}, False), ({_s + ".sha256"}, False),      # incomplete systemd pair: stay hidden
    ({_s, _s + ".sha256"}, True),                  # tested + published
    ({_s.replace("x86_64", "aarch64"), _s.replace("x86_64", "aarch64") + ".sha256"}, False),  # other arch only
]
_fail = [(a, w) for a, w in _cases if gating.systemd_listed(_o, a) != w]
assert gating.systemd_name(_o) == _s
if _fail:
    print("GATING CHECK FAILED", _fail, file=sys.stderr); sys.exit(1)
print(f"gating: {len(_cases)} cases ok")
sys.exit(1 if bad else 0)
