#!/usr/bin/env python3
"""Storage > SAN page and the Add SAN storage wizard, in a real browser.

1. The real server (no SAN on the CI node): /storage/san renders its empty
   states (no LUNs, no targets, no FC HBA) with the "Add SAN storage"
   action, has its own <title>, and throws no JS error.
2. The wizard end to end with the SAN API mocked in the browser (Playwright
   routes; a CI container has no iSCSI array): portals -> discovered target
   with every portal -> target id (POST /api/san/targets carries the portal
   list) -> the LUN the target brought (from /api/san/paths) -> LVM storage
   with "Snapshots as volume chain" -> review -> finish. The POST
   /api/san/lvm body must carry the picked WWID and the checkbox values.
3. A 424 from discovery (no iscsiadm) shows the node-lacks empty state, not
   an error banner.

Screenshots (desktop 1280 + phone 390, light + dark) go to $SHOTS
(default /tmp/san-shots).
"""
import json
import os
import sys

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
SHOTS = os.environ.get("SHOTS", "/tmp/san-shots")
IQN = "iqn.2003-01.org.example:array1"
WWID = "36001405aabbccddeeff00112233445566"
PORTALS = ["10.77.1.1:3260", "10.77.1.2:3260", "10.77.2.1:3260", "10.77.2.2:3260"]

errors = []


def login(page):
    page.goto(f"{BASE}/login")
    page.locator("input[type=text], input:not([type])").first.fill("admin")
    page.locator("input[type=password]").fill(os.environ.get("ADMIN_PASSWORD", "admin"))
    page.locator("button[type=submit]").click()
    page.wait_for_url(f"{BASE}/")


def watch(page, tag):
    page.on("pageerror", lambda e: errors.append(f"{tag}: pageerror {e}"))


def shot(page, name):
    os.makedirs(SHOTS, exist_ok=True)
    page.screenshot(path=f"{SHOTS}/{name}.png", full_page=True)


def real_page(browser, scheme, width):
    ctx = browser.new_context(ignore_https_errors=True, color_scheme=scheme,
                              viewport={"width": width, "height": 900})
    page = ctx.new_page()
    watch(page, f"san-{scheme}-{width}")
    login(page)
    page.goto(f"{BASE}/storage/san")
    expect(page).to_have_title("SAN · Horcrux", timeout=30000)
    expect(page.get_by_text("No SAN LUNs on this node")).to_be_visible(timeout=30000)
    expect(page.get_by_text("No SAN targets yet")).to_be_visible()
    expect(page.get_by_text("This node has no Fibre Channel HBA")).to_be_visible()
    add = page.locator("a[href='/storage/san/new']").first
    expect(add).to_be_visible()
    # Zero counts are never red: there are none on an empty page, and the
    # Rescan button is disabled without an HBA.
    expect(page.get_by_role("button", name="Rescan HBAs")).to_be_disabled()
    assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth + 1"), "horizontal scroll"
    shot(page, f"san-page-{scheme}-{width}")
    add.click()
    page.wait_for_url("**/storage/san/new")
    expect(page).to_have_title("Add SAN storage · Horcrux")
    shot(page, f"san-wizard-step1-{scheme}-{width}")
    ctx.close()


def wizard(browser, scheme, width):
    ctx = browser.new_context(ignore_https_errors=True, color_scheme=scheme,
                              viewport={"width": width, "height": 900})
    page = ctx.new_page()
    watch(page, f"wizard-{scheme}-{width}")
    posts = {}

    def ok(body, status=200):
        return lambda r: r.fulfill(status=status, content_type="application/json", body=json.dumps(body))

    def record(name, body, status=200):
        def h(route):
            if route.request.method == "POST":
                posts[name] = json.loads(route.request.post_data or "{}")
            route.fulfill(status=status, content_type="application/json", body=json.dumps(body))
        return h

    page.route("**/api/san/discover", record("discover", {"targets": [{"iqn": IQN, "portals": PORTALS}], "errors": []}))
    page.route("**/api/san/targets", record("target", {"id": "array1", "iqn": IQN, "portals": PORTALS[:1]}, 201))
    page.route("**/api/san/apply", ok({"node": "n1", "actions": [], "errors": [], "missing": []}))
    lun = {"wwid": WWID, "map": WWID, "device": f"/dev/mapper/{WWID}", "size_bytes": 10 * 2**30,
           "vendor": "LIO-ORG", "product": "lun0", "target": IQN, "lun": 0, "active_paths": 4, "failed_paths": 0,
           "paths": [{"dev": f"sd{c}", "transport": "iscsi", "target": IQN, "portal": p, "local_address": None,
                      "lun": 0, "state": "active"} for c, p in zip("bcde", PORTALS)]}
    page.route("**/api/san/paths", ok({"node": "n1", "multipathd_running": True, "luns": [lun], "errors": []}))
    page.route("**/api/san/lvm", record("lvm", {"node": "n1", "actions": [
        "created shared VG vg_san on /dev/mapper/" + WWID, "added storage pool san-lvm"], "errors": [], "missing": []}, 201))

    login(page)
    page.goto(f"{BASE}/storage/san/new")
    page.get_by_label("Portals (host or host:port, comma separated)").fill("10.77.1.1")
    page.get_by_role("button", name="Discover").click()
    expect(page.get_by_text(IQN)).to_be_visible(timeout=15000)
    expect(page.get_by_text("4 paths:")).to_be_visible()
    shot(page, f"san-wizard-step2-{scheme}-{width}")
    page.get_by_label("Target ID (letters, digits, - and _)").fill("array1")
    page.get_by_role("button", name="Add target and log in").click()
    expect(page.get_by_text("Pick the LUN")).to_be_visible(timeout=15000)
    expect(page.get_by_text(WWID)).to_be_visible()
    shot(page, f"san-wizard-step3-{scheme}-{width}")
    page.get_by_role("button", name="Next").click()
    page.get_by_label("Storage ID").fill("san-lvm")
    page.get_by_label("Volume group name").fill("vg_san")
    chain = page.get_by_label("Snapshots as volume chain (qcow2 volumes on thick LVs)")
    expect(chain).to_be_checked()
    page.get_by_label("Zero removed volumes").check()
    shot(page, f"san-wizard-step4-{scheme}-{width}")
    page.get_by_role("button", name="Next").click()
    expect(page.get_by_text("volume chain")).to_be_visible()
    shot(page, f"san-wizard-step5-{scheme}-{width}")
    page.get_by_role("button", name="Finish").click()
    expect(page.get_by_text("SAN storage added")).to_be_visible(timeout=15000)
    expect(page.get_by_text("added storage pool san-lvm")).to_be_visible()
    shot(page, f"san-wizard-done-{scheme}-{width}")

    assert posts["discover"] == {"portals": ["10.77.1.1"]}, posts
    assert posts["target"]["iqn"] == IQN and posts["target"]["id"] == "array1", posts
    assert posts["target"]["portals"] == ["10.77.1.1"], posts
    assert "chap_secret" not in posts["target"], "no CHAP given, none sent"
    assert posts["lvm"] == {"id": "san-lvm", "vg_name": "vg_san", "device": WWID, "wipe_removed": True,
                            "discard_removed": True, "snapshot_as_volume_chain": True, "create_vg": True}, posts
    ctx.close()


def unavailable(browser):
    ctx = browser.new_context(ignore_https_errors=True, viewport={"width": 1280, "height": 900})
    page = ctx.new_page()
    watch(page, "424")
    page.route("**/api/san/discover", lambda r: r.fulfill(status=424, content_type="application/json", body=json.dumps({
        "error": "NOT_AVAILABLE", "reason": "san-iscsiadm-missing",
        "message": "This node has no iSCSI initiator (iscsiadm).",
        "details": "Install open-iscsi (emerge sys-block/open-iscsi) and start iscsid, or enable USE=san on horcrux."})))
    login(page)
    page.goto(f"{BASE}/storage/san/new")
    page.get_by_label("Portals (host or host:port, comma separated)").fill("10.0.0.9")
    page.get_by_role("button", name="Discover").click()
    st = page.locator(".unavailable-state[data-reason=san-iscsiadm-missing]")
    expect(st).to_be_visible(timeout=15000)
    expect(st).to_contain_text("no iSCSI initiator")
    expect(page.locator(".alert-error")).to_have_count(0)
    shot(page, "san-wizard-424")
    ctx.close()


def main():
    with sync_playwright() as p:
        browser = p.chromium.launch()
        for scheme in ("light", "dark"):
            for width in (1280, 390):
                real_page(browser, scheme, width)
                wizard(browser, scheme, width)
        unavailable(browser)
        browser.close()
    if errors:
        print("\n".join(errors))
        sys.exit(1)
    print("san wizard e2e: ok")


if __name__ == "__main__":
    main()
