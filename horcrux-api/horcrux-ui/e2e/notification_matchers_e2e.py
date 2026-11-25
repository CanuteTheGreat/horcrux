#!/usr/bin/env python3
"""Notifications > Routing: build a matcher tree in the browser, drag a condition between groups,
reload, and check the saved tree; then test routing and clean up.

Covers: add group / conditions through the labelled controls, weekday toggles and time pickers, drag and
drop of a leaf into another group and of a whole group into another group (persisted after Save + reload),
the test panel's per-node reasons, and that every control in the editor has an accessible name.
"""
import json
import os
import ssl
import urllib.error
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
NAME = "e2e-tree-matcher"


def req(method, path, tok=None, body=None):
    h = {"Content-Type": "application/json"}
    if tok:
        h["Authorization"] = f"Bearer {tok}"
    r = urllib.request.Request(f"{BASE}{path}", method=method, headers=h,
                               data=json.dumps(body).encode() if body is not None else None)
    try:
        with urllib.request.urlopen(r, context=SSL) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        if method == "DELETE":
            return None
        raise AssertionError(f"{method} {path} -> {e.code}: {e.read()[:300]}")


def saved(tok):
    return next(m for m in req("GET", "/api/notifications/matchers", tok) if m["name"] == NAME)


def types(nodes):
    """Compact shape of a tree: leaf type, or [group children...]."""
    return [[types(n["children"])] if n["type"] == "group" else n["type"] for n in nodes]


def cleanup(tok):
    for m in req("GET", "/api/notifications/matchers", tok):
        if m["name"] == NAME:
            req("DELETE", f"/api/notifications/matchers/{m['id']}", tok)


def main():
    tok = req("POST", "/api/auth/login", body={"username": "admin", "password": "admin"})["ticket"]
    cleanup(tok)
    try:
        with sync_playwright() as p:
            b = p.firefox.launch()
            page = b.new_context(ignore_https_errors=True, viewport={"width": 1400, "height": 1200}).new_page()
            errors = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(f"{BASE}/login")
            page.fill("#login-username", "admin")
            page.fill("#login-password", "admin")
            page.locator("#login-submit").click()
            page.wait_for_url(f"{BASE}/")
            page.goto(f"{BASE}/monitoring/notifications?tab=routing")
            expect(page.locator("#routing-section")).to_be_visible(timeout=30000)
            # the default matcher exists from the start
            expect(page.locator("#matcher-table tr[data-matcher='default']")).to_have_count(1, timeout=30000)

            page.click("#matcher-new")
            expect(page.locator("#matcher-editor")).to_be_visible()
            page.fill("#matcher-name", NAME)
            page.locator("#matcher-name").dispatch_event("change")
            page.select_option("#matcher-mode", "any")

            # two groups, one condition in each, plus a top-level severity condition
            page.click("#matcher-add-group")
            page.click("#matcher-add-group")
            groups = page.locator("#matcher-tree > ul > li.mt-group")
            expect(groups).to_have_count(2)
            g1_add = groups.nth(0).locator("select[aria-label='Add a condition to this group']").first
            g1_add.select_option("event_type")
            g2_add = groups.nth(1).locator("select[aria-label='Add a condition to this group']").first
            g2_add.select_option("calendar")
            page.select_option("select[aria-label='Add a condition at the top level']", "severity")

            # weekday toggles and time pickers (calendar leaf): Saturday on, 22:00 -> 06:00
            cal = page.locator("li.mt-node:has(> .mt-body .mt-kind:text-is('Calendar'))")
            expect(cal).to_have_count(1)
            cal.get_by_label("Weekday Sat").check()
            cal.get_by_label("Start time").fill("22:00")
            cal.get_by_label("End time (earlier than the start crosses midnight)").fill("06:00")
            cal.get_by_label("Time zone, for example Europe/Berlin").fill("UTC")
            cal.get_by_label("Time zone, for example Europe/Berlin").dispatch_event("change")

            # event-type leaf: tick "Package updates"
            ev = page.locator("li.mt-node:has(> .mt-body .mt-kind:text-is('Event type'))")
            ev.get_by_label("Event type Package updates").check()

            # target: every enabled channel (no channel needed)
            page.check("#matcher-all-channels")

            # drag the calendar leaf from group 2 into group 1
            src = cal.locator(".mt-handle").first
            dst = groups.nth(0).locator(".mt-group-head").first
            src.drag_to(dst)
            page.wait_for_timeout(300)
            expect(groups.nth(0).locator("ul.mt-list li.mt-node")).to_have_count(2)
            expect(groups.nth(1).locator("ul.mt-list li.mt-node")).to_have_count(0)

            # drag the (now empty) second group into the first one
            src = groups.nth(1).locator(".mt-handle").first
            dst = groups.nth(0).locator(".mt-group-head").first
            src.drag_to(dst)
            page.wait_for_timeout(300)
            expect(page.locator("#matcher-tree > ul > li.mt-group")).to_have_count(1)
            expect(page.locator("#matcher-tree li.mt-group li.mt-group")).to_have_count(1)

            # every control in the editor has an accessible name
            unnamed = page.evaluate("""() => [...document.querySelectorAll('#matcher-editor input, #matcher-editor select, #matcher-editor button')]
                .filter(e => !(e.getAttribute('aria-label') || e.labels?.length || e.textContent.trim()))
                .map(e => e.outerHTML.slice(0, 80))""")
            assert not unnamed, f"controls without a label: {unnamed}"

            page.click("#matcher-save")
            expect(page.locator("#routing-notice")).to_contain_text("Saved matcher", timeout=15000)

            m = saved(tok)
            assert m["mode"] == "any" and m["all_channels"] is True, m
            # the severity leaf stayed on top; the dragged leaf and the dragged group live in the first group
            assert types(m["children"]) == [[["event_type", "calendar", [[]]]], "severity"], types(m["children"])
            group = m["children"][0]
            cal_saved = group["children"][1]
            assert cal_saved["weekdays"] == ["mon", "tue", "wed", "thu", "fri", "sat"], cal_saved
            assert (cal_saved["start"], cal_saved["end"], cal_saved["timezone"]) == ("22:00", "06:00", "UTC"), cal_saved
            assert sorted(group["children"][0]["kinds"]) == ["alert", "package_update"], group["children"][0]

            # reload: the tree is shown as saved
            page.goto(f"{BASE}/monitoring/notifications?tab=routing&matcher={m['id']}")
            expect(page.locator("#matcher-editor")).to_be_visible(timeout=30000)
            expect(page.locator("#matcher-tree > ul > li.mt-group")).to_have_count(1)
            expect(page.locator("#matcher-tree li.mt-group li.mt-group")).to_have_count(1)
            expect(page.locator("li.mt-node:has(> .mt-body .mt-kind:text-is('Calendar'))")
                   .get_by_label("Start time")).to_have_value("22:00")

            # test panel: a Saturday 23:30 UTC package update matches through the calendar leaf
            page.select_option("#test-kind", "package_update")
            page.fill("#test-at", "2026-11-28T23:30")
            page.locator("#test-at").dispatch_event("change")
            page.click("#test-run")
            expect(page.locator("#test-result")).to_be_visible(timeout=15000)
            expect(page.locator(f"#test-result li[data-verdict='{NAME}']")).to_contain_text("match")
            expect(page.locator("#test-result")).to_contain_text("Goes to")
            assert not errors, errors
            b.close()
        print("ok - routing tree editor: build, drag between groups, save, reload, test")
    finally:
        cleanup(tok)


if __name__ == "__main__":
    main()
