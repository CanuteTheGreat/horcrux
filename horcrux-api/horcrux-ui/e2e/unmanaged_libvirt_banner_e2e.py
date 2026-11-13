#!/usr/bin/env python3
"""The dashboard's unmanaged-libvirt banner never lists the source of a completed import.

Class of bug (owner, 2026-11-13): HOGSMEADE showed both "Old sources of imported VMs can be cleaned up
(captainjacksparrow, redweek-win11)" and "2 unmanaged libvirt VM(s) ... [Import into Horcrux]" for the same two
domains. A source belongs only in the cleanup line.

The libvirt list is mocked in the browser (Playwright routes): one domain that is the source of a completed import
(the cleanup offer names it; the list says imported_as even in the old buggy shape, null), one genuinely unmanaged
domain and one whose import failed. The banner must list exactly the genuine and the failed one, with the failed one's
state, and the cleanup line must list the imported source.
"""
import json
import os
import sys

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")


def dom(name, imported_as=None, state=None):
    return {"domain": name, "uuid": f"uuid-{name}", "state": "shut off", "cpus": 2, "memory_mb": 2048,
            "disks": [f"/var/lib/libvirt/images/{name}.qcow2"], "imported_as": imported_as, "problem": None,
            "import_state": state}


LIST = {"nodes": [{"node": "hogsmeade", "local": True, "error": None, "available": True, "domains": [
    dom("imported-source"),  # imported_as null: the pre-fix server answer; the cleanup offer must still hide it
    dom("really-unmanaged"),
    dom("failed-import", state="last import failed: no space"),
    dom("imported-source-2", imported_as="101"),
]}], "unmanaged": 2}
OFFER = [{"vm_id": "100", "source": "libvirt", "total_bytes": 90 * 2**30, "items": [
    {"kind": "libvirt-domain", "path": "imported-source", "bytes": 0, "remove": True, "reason": None},
    {"kind": "libvirt-domain", "path": "imported-source-2", "bytes": 0, "remove": True, "reason": None}]}]


LIST_LOCAL = {"available": True, "domains": [
    dom("imported-source", imported_as="100"),
    dom("really-unmanaged"),
]}
PREVIEW = {"vm_id": "100", "source": "libvirt", "total_bytes": 90 * 2**30, "items": [
    {"kind": "libvirt-domain", "path": "imported-source", "bytes": 0, "remove": True, "reason": None},
    {"kind": "file", "path": "/var/lib/libvirt/images/imported-source.qcow2", "bytes": 84 * 2**30, "remove": True,
     "reason": None}]}


def cleanup_flow(browser):
    """/vms/import-libvirt: Review lands on the imported-sources section; Remove old source needs the typed name."""
    posts = []
    page = browser.new_page(ignore_https_errors=True)
    ok = lambda body: (lambda r: r.fulfill(status=200, content_type="application/json", body=json.dumps(body)))
    page.route("**/api/vms/import/libvirt/list-cluster", ok(LIST))
    page.route("**/api/vms/import/libvirt/list", ok(LIST_LOCAL))
    page.route("**/api/vms/import/cleanup/pending", ok(OFFER))
    page.route("**/api/vms/import/cleanup/preview", ok(PREVIEW))

    def cleanup(route):
        body = json.loads(route.request.post_data or "{}")
        posts.append(body)
        if len(posts) == 1:  # the first attempt: the verification refuses (copy differs)
            route.fulfill(status=409, content_type="application/json", body=json.dumps(
                {"error": "the imported copy /h/a.qcow2 is not identical to /v/a.qcow2: nothing was removed"}))
        else:
            route.fulfill(status=200, content_type="application/json", body=json.dumps(
                {"removed": PREVIEW["items"], "kept": [], "xml_backup": "/var/lib/horcrux/libvirt-import/100.cleanup-backup.xml"}))
    page.route("**/api/vms/import/cleanup", cleanup)

    page.goto(f"{BASE}/login")
    page.locator("input[type=text], input:not([type])").first.fill("admin")
    page.locator("input[type=password]").fill(os.environ.get("ADMIN_PASSWORD", "admin"))
    page.locator("button[type=submit]").click()
    page.wait_for_url(f"{BASE}/")
    page.goto(f"{BASE}/")
    review = page.locator("#import-cleanup-review")
    expect(review).to_be_visible(timeout=30000)
    review.click()
    page.wait_for_url("**/vms/import-libvirt#imported-sources")
    section = page.locator("#imported-sources")
    expect(section).to_be_visible(timeout=30000)
    row = page.locator("tr.lv-imported-domain", has_text="imported-source")
    assert "Imported" in row.inner_text(), row.inner_text()
    assert row.locator("a.lv-vm-link").get_attribute("href") == "/vms/100"
    assert row.locator("button.lv-preview").count() == 0, "an imported source offers Import"
    assert "Unmanaged" not in row.inner_text()
    # the genuine domain keeps its Import action and is not in the imported section
    assert page.locator("#lv-domains tr.lv-domain", has_text="really-unmanaged").locator("button.lv-preview").count() == 1
    assert page.locator("#lv-domains tr", has_text="imported-source").count() == 0

    row.locator("#cleanup-show").click()
    expect(page.locator("#cleanup-offer")).to_be_visible()
    run = page.locator("#cleanup-run")
    expect(run).to_be_disabled()
    page.fill("#cleanup-confirm-name", "wrong-name")
    expect(run).to_be_disabled()
    page.fill("#cleanup-confirm-name", "imported-source")
    expect(run).to_be_enabled()
    run.click()
    expect(page.locator("#cleanup-error")).to_contain_text("nothing was removed")
    run.click()
    expect(page.locator("#cleanup-done")).to_contain_text("Removed 2 item(s)")
    assert len(posts) == 2 and all(
        b == {"vm_id": "100", "confirm": True, "confirm_domain": "imported-source"} for b in posts), posts
    page.close()


def main():
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(ignore_https_errors=True)
        page.route("**/api/vms/import/libvirt/list-cluster", lambda r: r.fulfill(
            status=200, content_type="application/json", body=json.dumps(LIST)))
        page.route("**/api/vms/import/cleanup/pending", lambda r: r.fulfill(
            status=200, content_type="application/json", body=json.dumps(OFFER)))
        page.goto(f"{BASE}/login")
        page.locator("input[type=text], input:not([type])").first.fill("admin")
        page.locator("input[type=password]").fill(os.environ.get("ADMIN_PASSWORD", "admin"))
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/")
        for path in ("/", "/vms"):
            page.goto(f"{BASE}{path}")
            banner = page.locator("#unmanaged-libvirt")
            expect(banner).to_be_visible(timeout=30000)
            names = page.locator("#unmanaged-libvirt .unmanaged-vm").all_inner_texts()
            joined = " | ".join(names)
            assert len(names) == 2, f"{path}: banner lists {names}"
            assert "really-unmanaged" in joined, f"{path}: genuine domain missing: {joined}"
            assert "failed-import (shut off, last import failed: no space)" in joined, f"{path}: {joined}"
            assert "imported-source" not in joined, f"{path}: an imported source is flagged unmanaged: {joined}"
            assert "2 unmanaged" in banner.inner_text(), banner.inner_text()
            cleanup = page.locator("#import-cleanup-pending").inner_text()
            assert "100" in cleanup, cleanup
        cleanup_flow(browser)
        browser.close()
    print("unmanaged-libvirt banner: only genuine domains are listed")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as e:
        print(f"FAIL: {e}")
        sys.exit(1)
