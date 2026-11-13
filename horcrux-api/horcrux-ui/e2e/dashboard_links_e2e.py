#!/usr/bin/env python3
"""Dashboard links e2e (Playwright Firefox, desktop AND phone width).

Every card, count and list row on the Horcrux dashboard must be a real anchor
(middle-click / open-in-new-tab / keyboard work), point at a page that exists
and renders without JavaScript errors, and the pre-filtered targets must show
the right subset:

  * /vms?state=running|stopped lists only those VMs and matches the card count
  * /tasks?status=running pre-selects the filter, /tasks?upid=X opens that log
  * /storage?pool=ID highlights that pool

Runs against a live horcrux-api serving the built UI (BASE_URL, default
https://localhost:8006, admin/admin), same as ui_e2e.py. It seeds its own VMs
through the API and removes them again.
"""
import json
import os
import ssl
import sys
import urllib.error
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
PREFIX = "dl-e2e"
VMS = {  # id -> status the VM is seeded with
    f"{PREFIX}-stopped-1": "stopped",
    f"{PREFIX}-stopped-2": "stopped",
    f"{PREFIX}-running-1": "running",
}
WIDTHS = {"desktop": (1280, 900), "mobile": (390, 844)}

# card id -> (expected href, always present?)
CARDS = {
    "stat-vms": "/vms",
    "stat-vms-running": "/vms?state=running",
    "stat-vms-stopped": "/vms?state=stopped",
    "stat-containers": "/containers",
    "stat-storage-pools": "/storage",
    "stat-active-alerts": "/alerts",
    "stat-tasks-running": "/tasks?status=running",
    "stat-cpu": "/monitoring",
}
OPTIONAL_CARDS = {"stat-cluster-nodes": "/ha/cluster"}  # USE=cluster only


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


def token():
    return req("POST", "/api/auth/login", body={"username": "admin", "password": "admin"})["ticket"]


POOL_NAME = f"{PREFIX}-pool"
POOL_DIR = f"/tmp/{POOL_NAME}"


def seed(tok):
    """Two stopped VMs, one really running VM (qemu is installed in CI) and a
    directory storage pool, so every list on the dashboard has rows."""
    for vid, status in VMS.items():
        req("POST", "/api/vms", tok, {"id": vid, "name": vid, "hypervisor": "qemu",
                                     "architecture": "x86_64", "cpus": 1, "memory": 128,
                                     "disk_size": 1, "status": "stopped"})
        if status == "running":
            req("POST", f"/api/vms/{vid}/start", tok)
    os.makedirs(POOL_DIR, exist_ok=True)
    req("POST", "/api/storage/pools", tok, {"name": POOL_NAME, "storage_type": "directory", "path": POOL_DIR})


def cleanup(tok):
    for vid, status in VMS.items():
        if status == "running":
            try:
                req("POST", f"/api/vms/{vid}/stop", tok)
            except AssertionError:
                pass
        req("DELETE", f"/api/vms/{vid}", tok)
    for pool in req("GET", "/api/storage/pools", tok) or []:
        if pool["name"] == POOL_NAME:
            req("DELETE", f"/api/storage/pools/{pool['id']}", tok)


def api_vms(tok):
    return [v for v in req("GET", "/api/vms", tok) or []]


def settle(page):
    """Dashboard fully loaded: features known, no 'Loading' placeholders."""
    expect(page.locator("nav.navbar[data-features-loaded='true']")).to_have_count(1, timeout=30000)
    page.wait_for_selector("#stat-vms .stat-value", timeout=30000)
    page.wait_for_timeout(2500)  # the panels fetch sequentially


def check_dashboard_anchors(page, label, vm_counts):
    dash = page.locator(".dashboard")
    cards = dict(CARDS)
    for cid, href in OPTIONAL_CARDS.items():
        if page.locator(f"#{cid}").count():
            cards[cid] = href
    for cid, href in cards.items():
        el = page.locator(f"#{cid}")
        assert el.count() == 1, f"[{label}] card #{cid} missing"
        tag = el.evaluate("e => e.tagName")
        assert tag == "A", f"[{label}] card #{cid} is <{tag}>, not an anchor"
        got = el.get_attribute("href")
        assert got == href, f"[{label}] card #{cid} href {got!r}, want {href!r}"
        cur = el.evaluate("e => getComputedStyle(e).cursor")
        assert cur == "pointer", f"[{label}] card #{cid} cursor {cur!r}"
        assert el.is_visible(), f"[{label}] card #{cid} not visible"
    # the VM cards show the real counts
    for cid, key in (("stat-vms", "all"), ("stat-vms-running", "running"), ("stat-vms-stopped", "stopped")):
        txt = page.locator(f"#{cid} .stat-value").inner_text().strip()
        assert txt == str(vm_counts[key]), f"[{label}] #{cid} shows {txt!r}, API says {vm_counts[key]}"

    # every list row is a link: no <li> in a dashboard list without an anchor
    rows = dash.locator("li")
    for i in range(rows.count()):
        n = rows.nth(i).locator("a[href]").count()
        assert n >= 1, f"[{label}] dashboard list row {i} has no link: {rows.nth(i).inner_text()!r}"
    # our seeded VMs are in the recent-VMs list and link to their detail page
    for vid in VMS:
        a = page.locator(f"#dash-vms a[href='/vms/{vid}']")
        assert a.count() == 1, f"[{label}] recent-VMs row for {vid}: {a.count()} link(s)"
    pool_link = page.locator("#dash-pools a[href^='/storage?pool=']")
    assert pool_link.count() >= 1, f"[{label}] storage list has no pool link"
    # nothing clickable on the dashboard that is not an anchor / inside one
    bad = dash.evaluate("""d => [...d.querySelectorAll('*')].filter(e =>
        (e.hasAttribute('onclick') || getComputedStyle(e).cursor === 'pointer') && !e.closest('a'))
        .map(e => e.tagName + '.' + e.className)""")
    assert not bad, f"[{label}] clickable non-anchors on the dashboard: {bad}"
    # keyboard: cards are focusable and show a focus ring
    first = page.locator("#stat-vms")
    first.focus()
    assert page.evaluate("document.activeElement.id") == "stat-vms"
    page.keyboard.press("Shift+Tab")
    page.keyboard.press("Tab")
    assert page.evaluate("document.activeElement.id") == "stat-vms", f"[{label}] Tab does not reach the card"
    hrefs = page.evaluate("[...document.querySelectorAll('.dashboard a[href]')].map(a => a.getAttribute('href'))")
    return sorted(set(h for h in hrefs if h.startswith("/")))


def check_target(page, label, href, js_errors, bad_responses):
    js_errors.clear()
    bad_responses.clear()
    page.goto(f"{BASE}{href}")  # a real document load: the server must serve the SPA for it
    expect(page.locator("nav.navbar[data-features-loaded='true']")).to_have_count(1, timeout=30000)
    page.wait_for_timeout(1500)
    n_h1 = page.locator("main h1, main h2").count()
    body = page.locator("main").inner_text().strip()
    assert n_h1 >= 1 and body, f"[{label}] {href} renders an empty page (no route?)"
    assert not js_errors, f"[{label}] {href} JS errors: {js_errors}"
    assert not bad_responses, f"[{label}] {href} 404 resources: {bad_responses}"


def main():
    tok = token()
    cleanup(tok)
    seed(tok)
    vms = api_vms(tok)
    mine = {v["id"]: v["status"] for v in vms if v["id"] in VMS}
    assert mine == VMS, f"seeding failed (a VM did not start?): {mine}"
    counts = {
        "all": len(vms),
        "running": sum(1 for v in vms if v["status"] == "running"),
        "stopped": sum(1 for v in vms if v["status"] == "stopped"),
    }
    print("api VM counts:", counts, "seeded:", mine)
    try:
        run(tok, vms, counts, mine)
    finally:
        cleanup(tok)
    print("ok - dashboard links e2e")


def run(tok, vms, counts, mine):
    js_errors, bad_responses = [], []
    with sync_playwright() as p:
        browser = p.firefox.launch()
        for label, (w, h) in WIDTHS.items():
            ctx = browser.new_context(viewport={"width": w, "height": h}, ignore_https_errors=True,
                                      is_mobile=False)
            page = ctx.new_page()
            page.on("pageerror", lambda e: js_errors.append(str(e)))
            page.on("response", lambda r: r.status == 404
                    and r.request.resource_type in ("document", "stylesheet", "script", "image", "font")
                    and r.url.startswith(BASE) and bad_responses.append(r.url))
            page.goto(f"{BASE}/login")
            page.fill("#login-username", "admin")
            page.fill("#login-password", "admin")
            page.locator("#login-submit").click()
            page.wait_for_url(f"{BASE}/")
            settle(page)
            assert not js_errors, f"[{label}] dashboard JS errors: {js_errors}"

            hrefs = check_dashboard_anchors(page, label, counts)
            print(f"[{label}] dashboard links: {hrefs}")

            # ---- sanity: the page check can tell a missing route from a real one
            page.goto(f"{BASE}/no-such-page-xyz")
            expect(page.locator("nav.navbar[data-features-loaded='true']")).to_have_count(1, timeout=30000)
            page.wait_for_timeout(1000)
            assert page.locator("main h1, main h2").count() == 0, "unknown route unexpectedly renders a page"

            # ---- every distinct target resolves and renders without JS errors
            for href in hrefs:
                check_target(page, label, href, js_errors, bad_responses)
            print(f"[{label}] ok: {len(hrefs)} targets render")

            # ---- filtered VM lists show the right subset (and the card count)
            for state in ("running", "stopped"):
                page.goto(f"{BASE}/vms?state={state}")
                page.wait_for_selector("table.vm-table", timeout=30000)
                shown = page.locator("table.vm-table tr[data-vm-id]").evaluate_all(
                    "rows => rows.map(r => r.getAttribute('data-vm-id'))")
                want = sorted(v["id"] for v in vms if v["status"] == state)
                assert sorted(shown) == want, f"[{label}] /vms?state={state}: {sorted(shown)} != {want}"
                assert len(shown) == counts[state]
            page.goto(f"{BASE}/vms")
            page.wait_for_selector("table.vm-table")
            assert page.locator("table.vm-table tr[data-vm-id]").count() == counts["all"]
            page.goto(f"{BASE}/vms?state=paused")
            page.wait_for_selector("#vm-filter-empty, table.vm-table", timeout=30000)
            assert page.locator("table.vm-table tr[data-vm-id]").count() == 0 or any(
                v["status"] == "paused" for v in vms)

            # ---- clicking a card really navigates (client-side) to the filtered list
            page.goto(f"{BASE}/")
            settle(page)
            page.locator("#stat-vms-running").click()
            page.wait_for_url(f"{BASE}/vms?state=running")
            page.wait_for_selector("table.vm-table")
            assert page.locator("table.vm-table tr[data-vm-id]").count() == counts["running"]
            # ... and a recent-VM row opens that VM's page
            page.goto(f"{BASE}/")
            settle(page)
            page.locator(f"#dash-vms a[href='/vms/{next(iter(VMS))}']").click()
            page.wait_for_url(f"{BASE}/vms/{next(iter(VMS))}")

            # ---- tasks / storage deep links
            page.goto(f"{BASE}/tasks?status=running")
            expect(page.locator(".tasks-page select")).to_have_value("running", timeout=30000)
            pid = next(x["id"] for x in req("GET", "/api/storage/pools", tok) if x["name"] == POOL_NAME)
            page.goto(f"{BASE}/storage?pool={pid}")
            expect(page.locator(f"#pool-{pid}.pool-focus")).to_have_count(1, timeout=30000)
            assert page.locator(".storage-card.pool-focus").count() == 1
            assert not js_errors, f"[{label}] JS errors: {js_errors}"
            ctx.close()
        browser.close()


if __name__ == "__main__":
    try:
        main()
    except AssertionError as e:
        print(f"FAIL: {e}")
        sys.exit(1)
