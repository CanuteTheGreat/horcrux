#!/usr/bin/env python3
"""Unmanaged / foreign VMs are detected and offered for import.

Class of bug (owner, 2026-11-12): libvirt VMs created outside Horcrux were invisible - no importer, no hint on /vms.

Runs against a live API started with HORCRUX_VIRSH=<fake-virsh.sh> (CI / lab container) or against a host with real
libvirt (--real: compares with `virsh list --all --name`).  Asserts:
  1. every libvirt domain is reported by POST /api/vms/import/libvirt/list (nothing silently missing);
  2. a domain that is not managed has imported_as == null (so it is offered) and a usable plan (no `problem`);
  3. the /vms page, in Firefox, shows a visible link/button to the libvirt importer when something is unmanaged.

usage: BASE_URL=https://host:8006 ADMIN_PASSWORD=... check_foreign_vms.py [--real] [--expect a,b] [--no-ui]
fake-virsh.sh (next to this file) serves two domains, `uxqa-foreign-1` and `uxqa-foreign-2`.
"""
import argparse
import json
import os
import re
import ssl
import subprocess
import sys
import urllib.request

BASE = os.environ.get("BASE_URL", "https://localhost:8006").rstrip("/")
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE


def call(path, body=None, token=None):
    h = {"Content-Type": "application/json"}
    if token:
        h["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(BASE + path, data=json.dumps(body if body is not None else {}).encode(), headers=h, method="POST")
    with urllib.request.urlopen(req, context=CTX, timeout=60) as r:
        return json.loads(r.read() or b"{}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--real", action="store_true")
    ap.add_argument("--expect", default="uxqa-foreign-1,uxqa-foreign-2")
    ap.add_argument("--no-ui", action="store_true")
    a = ap.parse_args()
    bad = []
    tok = call("/api/auth/login", {"username": "admin", "password": os.environ.get("ADMIN_PASSWORD", "admin")}).get("ticket")
    lst = call("/api/vms/import/libvirt/list", token=tok)
    if not lst.get("available"):
        sys.exit("FAIL: importer says libvirt/virsh is unavailable (check HORCRUX_VIRSH / virsh install)")
    got = {d["domain"]: d for d in lst.get("domains", [])}
    if a.real:
        want = [n.strip() for n in subprocess.run(["virsh", "-c", "qemu:///system", "list", "--all", "--name"], capture_output=True, text=True).stdout.splitlines() if n.strip()]
    else:
        want = [n for n in a.expect.split(",") if n]
    for n in want:
        if n not in got:
            bad.append(f"libvirt domain '{n}' is not reported by the importer (invisible VM)")
    unmanaged = [d for d in got.values() if not d.get("imported_as")]
    for d in unmanaged:
        if d.get("problem"):
            bad.append(f"unmanaged domain '{d['domain']}' cannot be imported: {d['problem']}")
    print(f"importer: {len(got)} domains, {len(unmanaged)} unmanaged")
    if unmanaged and not a.no_ui:
        from playwright.sync_api import sync_playwright
        with sync_playwright() as p:
            b = p.firefox.launch()
            pg = b.new_context(ignore_https_errors=True, viewport={"width": 1280, "height": 800}).new_page()
            pg.goto(BASE + "/login")
            pg.locator("#login-password").wait_for(timeout=30000)
            pg.fill("#login-username", "admin")
            pg.fill("#login-password", os.environ.get("ADMIN_PASSWORD", "admin"))
            pg.click("#login-submit")
            pg.wait_for_function("() => !location.pathname.startsWith('/login')", timeout=30000)
            pg.goto(BASE + "/vms")
            pg.wait_for_timeout(3500)
            offers = pg.evaluate("""() => [...document.querySelectorAll('a[href],button')].filter(e => e.getBoundingClientRect().width > 0)
                .filter(e => /import/i.test((e.getAttribute('href') || '') + ' ' + e.innerText)).map(e => (e.innerText || e.getAttribute('href')).trim().slice(0, 50))""")
            print("UI offers:", offers)
            if not offers:
                bad.append("/vms shows no import link/button although unmanaged libvirt VMs exist")
            elif not any(re.search(r"libvirt|unmanaged|foreign|existing|found", o, re.I) for o in offers):
                print("note: the import offer on /vms does not mention libvirt/unmanaged VMs (generic 'Import' only)")
            b.close()
    for x in bad:
        print("FAIL", x)
    print(f"check_foreign_vms: {len(bad)} problem(s)")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
