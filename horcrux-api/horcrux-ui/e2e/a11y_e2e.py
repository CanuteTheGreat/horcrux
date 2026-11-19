#!/usr/bin/env python3
"""Accessibility gate: axe-core (vendored, e2e/a11y/vendor/axe.min.js) on the most used pages.

Config: e2e/a11y/a11y.json (page list, rule tags, impact threshold, widths, schemes); justified
false positives: e2e/a11y/a11y-allowlist.json. Impacts in `fail_impacts` (serious/critical) fail the
run; `list_impacts` (moderate/minor) are printed and written to the report only.

Runs against a live horcrux-api serving the built UI (BASE_URL, default https://localhost:8006,
admin/ADMIN_PASSWORD). Seeds one VM through the API for the VM detail page and removes it again.
Env: A11Y_VERBOSE=1 prints the nodes of axe 'incomplete' results, A11Y_REPORT=path.json, A11Y_WIDTHS=desktop,phone, A11Y_SCHEMES=light,dark, A11Y_PAGES=name,name.
"""
import json
import os
import ssl
import sys
import urllib.error
import urllib.request
from collections import Counter

from playwright.sync_api import sync_playwright

HERE = os.path.dirname(os.path.abspath(__file__))
A11Y = os.path.join(HERE, "a11y")
BASE = os.environ.get("BASE_URL", "https://localhost:8006").rstrip("/")
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
PASSWORD = os.environ.get("ADMIN_PASSWORD", "admin")
VM_ID = "a11y-e2e-vm"


def req(method, path, tok=None, body=None):
    headers = {"Content-Type": "application/json"}
    if tok:
        headers["Authorization"] = f"Bearer {tok}"
    r = urllib.request.Request(f"{BASE}{path}", method=method, headers=headers,
                               data=json.dumps(body).encode() if body is not None else None)
    try:
        with urllib.request.urlopen(r, context=SSL) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        if method == "DELETE":
            return None
        raise AssertionError(f"{method} {path} -> {e.code}: {e.read()[:300]}")


def load_json(name):
    with open(os.path.join(A11Y, name)) as f:
        return json.load(f)


def allowed(allow, page, rule, node_target):
    for e in allow.get("entries", []):
        if e["rule"] == rule and e.get("page", "*") in ("*", page) and e.get("selector", "") in node_target:
            assert e.get("reason"), f"allowlist entry for {rule} needs a reason"
            return True
    return False


def settle(page):
    try:
        page.wait_for_function("() => document.body && document.body.innerText.trim().length > 20", timeout=20000)
    except Exception:  # noqa: BLE001  (a blank page is caught by the crawl; axe still runs)
        pass
    page.wait_for_timeout(2500)


def login(page):
    page.goto(BASE + "/login", wait_until="load")
    page.locator("#login-password").wait_for(timeout=30000)
    page.locator("#login-username").fill("admin")
    page.locator("#login-password").fill(PASSWORD)
    page.locator("#login-submit").click()
    page.wait_for_function("() => !location.pathname.startsWith('/login')", timeout=30000)


def main():
    cfg = load_json("a11y.json")
    allow = load_json(cfg["allowlist"])
    axe_src = open(os.path.join(A11Y, cfg["axe_script"])).read()
    widths = [w for w in os.environ.get("A11Y_WIDTHS", ",".join(cfg["viewports"])).split(",") if w]
    schemes = [s for s in os.environ.get("A11Y_SCHEMES", ",".join(cfg["schemes"])).split(",") if s]
    only = [p for p in os.environ.get("A11Y_PAGES", "").split(",") if p]
    pages = [p for p in cfg["pages"] if not only or p["name"] in only]
    opts = {"runOnly": {"type": "tag", "values": cfg["tags"]}, "resultTypes": ["violations", "incomplete"]}

    tok = req("POST", "/api/auth/login", body={"username": "admin", "password": PASSWORD})["ticket"]
    req("DELETE", f"/api/vms/{VM_ID}", tok)
    req("POST", "/api/vms", tok, {"id": VM_ID, "name": VM_ID, "hypervisor": "qemu", "architecture": "x86_64",
                                  "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped"})
    needs_review = Counter()  # axe "incomplete": could not be decided automatically (listed, never fails)
    rows = []  # one per (page, vp, scheme, rule, impact)
    try:
        with sync_playwright() as p:
            browser = getattr(p, os.environ.get("BROWSER", "firefox")).launch()
            for scheme in schemes:
                for vp in widths:
                    w, h = cfg["viewports"][vp]
                    ctx = browser.new_context(ignore_https_errors=True, viewport={"width": w, "height": h}, color_scheme=scheme)
                    page = ctx.new_page()
                    if any(pg.get("auth", True) for pg in pages):
                        login(page)
                    for pg in pages:
                        if not pg.get("auth", True):
                            ctx2 = browser.new_context(ignore_https_errors=True, viewport={"width": w, "height": h}, color_scheme=scheme)
                            tgt = ctx2.new_page()
                        else:
                            ctx2, tgt = None, page
                        tgt.goto(BASE + pg["path"].format(vm_id=VM_ID), wait_until="load")
                        settle(tgt)
                        tgt.evaluate(axe_src)
                        res = tgt.evaluate("(o) => axe.run(document, o)", opts)
                        for v in res.get("incomplete", []):
                            needs_review[(pg["name"], v["id"])] += 1
                            if os.environ.get("A11Y_VERBOSE"):
                                for n in v["nodes"][:6]:
                                    info = (n.get("any") or [{}])[0]
                                    print(f"  incomplete {pg['name']} {vp}/{scheme} {v['id']} {' '.join(map(str, n['target']))[:80]} :: {info.get('message', '')[:110]} {info.get('data') if isinstance(info.get('data'), dict) and 'fgColor' in info['data'] else ''}")
                        for v in res["violations"]:
                            nodes = [n for n in v["nodes"] if not allowed(allow, pg["name"], v["id"], " ".join(map(str, n["target"])))]
                            if nodes:
                                rows.append({"page": pg["name"], "viewport": vp, "scheme": scheme, "rule": v["id"],
                                             "impact": v["impact"], "help": v["help"], "count": len(nodes),
                                             "targets": [" ".join(map(str, n["target"]))[:120] for n in nodes[:5]]})
                        if ctx2:
                            ctx2.close()
                    ctx.close()
            browser.close()
    finally:
        req("DELETE", f"/api/vms/{VM_ID}", tok)

    failing = [r for r in rows if r["impact"] in cfg["fail_impacts"]]
    listed = [r for r in rows if r["impact"] not in cfg["fail_impacts"]]
    if os.environ.get("A11Y_REPORT"):
        json.dump(rows, open(os.environ["A11Y_REPORT"], "w"), indent=1)
    for label, group in (("FAIL", failing), ("LIST", listed)):
        merged = {}
        for r in group:
            m = merged.setdefault((r["page"], r["rule"], r["impact"]), {"n": 0, "combos": [], "r": r})
            m["n"] = max(m["n"], r["count"])
            m["combos"].append(f"{r['viewport']}/{r['scheme']}")
        for (page_name, rule, impact), m in sorted(merged.items()):
            print(f"{label} {impact:8} {page_name:10} {rule} x{m['n']} [{', '.join(m['combos'])}]: {m['r']['help']} | {'; '.join(m['r']['targets'][:2])}")
    if needs_review:
        print("needs manual review (axe incomplete):", {f"{p}:{r}": n for (p, r), n in sorted(needs_review.items())})
    by_rule = Counter()
    for r in rows:
        by_rule[(r["impact"], r["rule"])] += r["count"]
    print("by rule:", dict(by_rule))
    print(f"a11y: {len(pages)} pages x {len(widths)} widths x {len(schemes)} schemes; {len(failing)} failing, {len(listed)} listed")
    sys.exit(1 if failing else 0)


if __name__ == "__main__":
    main()
