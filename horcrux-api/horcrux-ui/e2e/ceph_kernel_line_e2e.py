#!/usr/bin/env python3
"""Ceph install wizard, kernel line card and platform versions (Storage > Ceph, System > Packages, System > About).

The node in CI has no Portage, so the release list, the kernel line and the install task are mocked in the browser
(Playwright routes) with the shapes the server answers on a Gentoo node. Checks:
  * Ceph: release cards (recommended + testing tags), Next is blocked until the public network is filled, the review
    step shows what will be sent, Install posts {release, mode, public_network, ...}, the live task log follows new
    lines and ends with a Finished badge, and the card then shows the installed release with its upgrade grid.
  * Kernel line: choosing Newest sends PUT {line: newest}; the installed-kernel grid marks the running kernel;
    Update now opens the task log in place.
  * About: every component row is present.
Screenshots (desktop 1280 and phone 390, light and dark) go to SHOTS (default /tmp/ceph-kernel-shots).
"""
import json
import os
import sys
from urllib.parse import parse_qs, urlparse

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
SHOTS = os.environ.get("SHOTS", "/tmp/ceph-kernel-shots")
UPID = "UPID:node1:0000ABCD:ceph-install:test"
KUPID = "UPID:node1:0000ABCE:kernel-line:test"

RELEASES = {"arch": "amd64", "package_manager": True, "installed": None, "installed_major": None, "pinned": None,
            "recommended": 20, "configured": False, "releases": [
                {"major": 19, "name": "squid", "newest": "19.2.3", "versions": ["19.2.3"], "keyword": "stable",
                 "repo": "gentoo", "available": True},
                {"major": 20, "name": "tentacle", "newest": "20.2.0", "versions": ["20.2.0"], "keyword": "stable",
                 "repo": "gentoo", "available": True},
                {"major": 21, "name": "umbrella", "newest": "21.2.0", "versions": ["21.2.0"], "keyword": "testing",
                 "repo": "horcrux", "available": True}]}
INSTALLED = dict(RELEASES, installed="20.2.0", installed_major=20, pinned=20, configured=True)
UPGRADE = {"installed": "20.2.0", "installed_major": 20, "pinned": 20, "targets": [21], "next_step": "packages",
           "error": None, "daemons": {"by_type": [["mon", [[20, 1]]], ["mgr", [[20, 1]]], ["osd", [[20, 3]]]]}}
KERNEL = {"line": "default", "arch": "amd64", "running": "6.12.58-gentoo-dist",
          "installed": ["6.12.58-gentoo-dist", "6.12.41-gentoo-dist"], "provider": "sys-kernel/gentoo-kernel-bin",
          "provider_installed": "6.12.58", "default_version": "6.12.58", "newest_version": "6.18.9",
          "newest_available": "7.0.2", "cap": {"max": "6.18", "by": "sys-fs/zfs-kmod-2.4.0"},
          "held_back": "7.0.2 is held back: the installed OpenZFS supports kernels up to 6.18.",
          "update_pending": None, "pins_in_sync": True, "reboot_required": False}

failures = []


def check(cond, what):
    print(("ok - " if cond else "FAIL - ") + what)
    if not cond:
        failures.append(what)


def login(page):
    """Log in unless this browser context already holds a session (one login per context)."""
    page.goto(f"{BASE}/")
    pw = page.locator("input[type=password]")
    try:
        pw.wait_for(timeout=8000)
    except Exception:
        if "/login" not in page.url:
            return
    page.goto(f"{BASE}/login")
    page.locator("input[type=text], input:not([type])").first.fill("admin")
    page.locator("input[type=password]").fill(os.environ.get("ADMIN_PASSWORD", "admin"))
    page.locator("button[type=submit]").click()
    page.wait_for_url(f"{BASE}/")


def json_route(body, status=200):
    return lambda r: r.fulfill(status=status, content_type="application/json", body=json.dumps(body))


def ceph_flow(ctx, shot):
    page = ctx.new_page()
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    state = {"installed": False, "log_calls": 0, "posts": []}
    page.route("**/api/ceph/releases", lambda r: json_route(INSTALLED if state["installed"] else RELEASES)(r))
    page.route("**/api/ceph/upgrade", lambda r: json_route(UPGRADE)(r) if r.request.method == "GET"
               else json_route({"upid": UPID})(r))

    def install(route):
        state["posts"].append(json.loads(route.request.post_data or "{}"))
        route.fulfill(status=200, content_type="application/json", body=json.dumps({"upid": UPID}))
    page.route("**/api/ceph/install", install)

    full = ["Writing Portage pins for Ceph 21", ">>> Emerging sys-cluster/ceph-21.2.0",
            "Bootstrapping monitor and manager", "Ceph 21 installed"]

    def log(route):
        # like the server: lines from ?start=N; the first answer is mid-run, later ones finish the task
        state["log_calls"] += 1
        q = parse_qs(urlparse(route.request.url).query)
        start = int(q.get("start", ["0"])[0])
        have = 2 if state["log_calls"] == 1 else len(full)
        if have == len(full):
            state["installed"] = True
        body = {"lines": full[start:have], "total": have, "running": have < len(full)}
        route.fulfill(status=200, content_type="application/json", body=json.dumps(body))
    page.route("**/api/tasks/*/log*", log)
    page.route("**/api/tasks/UPID*", json_route({"upid": UPID, "node": "node1", "task_type": "ceph-install",
                                                "user": "admin", "status": "ok", "start_time": 0}))
    login(page)
    page.goto(f"{BASE}/storage/ceph")
    card = page.locator("#ceph-install")
    expect(card.locator(".hx-choice")).to_have_count(3, timeout=30000)
    check(card.locator("#ceph-release-20").is_checked(), "recommended release is preselected")
    check("Testing" in card.locator("label[for=ceph-release-21]").inner_text(), "testing release carries a tag")
    shot(page, "ceph-1-release")
    card.locator("#ceph-release-21").check()
    card.locator("#ceph-install-next").click()
    check(card.locator("#ceph-install-next").is_disabled(), "Next is blocked without a public network")
    card.locator("#ceph-public-net").fill("10.0.0.0/24")
    expect(card.locator("#ceph-install-next")).to_be_enabled()
    shot(page, "ceph-2-cluster")
    card.locator("#ceph-install-next").click()
    summary = card.locator("#ceph-install-summary").inner_text()
    check("10.0.0.0/24" in summary and "same as public" in summary and "21" in summary, "review lists the choices")
    shot(page, "ceph-3-review")
    card.locator("#ceph-install-submit").click()
    log_body = card.locator(".hx-tasklog-body")
    expect(log_body).to_contain_text("Emerging", timeout=20000)
    expect(card.locator("#ceph-installed")).to_be_visible(timeout=20000)
    expect(card.locator(".hx-tasklog [data-status=ok]")).to_be_visible(timeout=20000)
    expect(log_body).to_contain_text("Ceph 21 installed", timeout=20000)
    check("Emerging" in log_body.inner_text(), "task log shows the lines as they arrive")
    p = state["posts"][0] if state["posts"] else {}
    b = p.get("bootstrap") or {}
    check(str(p.get("release")) == "21" and b.get("mode") == "new" and b.get("public_network") == "10.0.0.0/24",
          f"install request carries the choices ({p})")
    expect(card.locator("#ceph-daemon-versions tbody tr")).to_have_count(3)
    shot(page, "ceph-4-installed")
    check(not errors, f"no page errors on the Ceph page ({errors[:3]})")
    page.close()


def kernel_flow(ctx, shot):
    page = ctx.new_page()
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    puts = []
    st = dict(KERNEL)

    def kl(route):
        if route.request.method == "PUT":
            puts.append(json.loads(route.request.post_data or "{}"))
            st.update(line="newest", update_pending="6.18.9")
        route.fulfill(status=200, content_type="application/json", body=json.dumps(st))
    page.route("**/api/system/kernel-line", kl)
    page.route("**/api/system/kernel-line/apply", json_route({"upid": KUPID}))
    page.route("**/api/tasks/*/log*", json_route({"lines": [">>> Emerging sys-kernel/gentoo-kernel-bin-6.18.9"],
                                                 "total": 1, "running": True}))
    login(page)
    page.goto(f"{BASE}/system/packages")
    card = page.locator("#kernel-line")
    expect(card.locator(".hx-choice")).to_have_count(2, timeout=30000)
    check(card.locator("#kernel-line-default").is_checked(), "saved line is selected")
    rows = card.locator("#kernel-line-installed tbody tr")
    expect(rows).to_have_count(2)
    check("Running" in rows.nth(0).inner_text(), "running kernel is marked")
    card.locator("#kernel-line-newest").check()
    expect(card.locator("#kernel-line-apply")).to_be_visible(timeout=10000)
    check(puts == [{"line": "newest"}], f"choosing Newest saves it ({puts})")
    card.locator("#kernel-line-apply").click()
    expect(card.locator(".hx-tasklog [data-status=running]")).to_be_visible(timeout=10000)
    card.scroll_into_view_if_needed()
    shot(page, "kernel-line")
    check(not errors, f"no page errors on the Packages page ({errors[:3]})")
    page.close()


def about_flow(ctx, shot):
    page = ctx.new_page()
    login(page)
    page.goto(f"{BASE}/system/about")
    rows = page.locator("#about-versions-table tbody tr")
    expect(rows.first).to_be_visible(timeout=30000)
    ids = [rows.nth(i).get_attribute("id") for i in range(rows.count())]
    for c in ("kernel", "qemu", "zfs", "ceph"):
        check(any(c in (i or "") for i in ids), f"About lists {c}")
    page.locator("#about-versions").scroll_into_view_if_needed()
    shot(page, "about-versions")
    page.close()


def main():
    os.makedirs(SHOTS, exist_ok=True)
    with sync_playwright() as pw:
        browser = pw.chromium.launch()
        for width, height, form in ((1280, 900, "desktop"), (390, 844, "phone")):
            for scheme in ("light", "dark"):
                ctx = browser.new_context(ignore_https_errors=True, viewport={"width": width, "height": height},
                                          color_scheme=scheme, is_mobile=form == "phone")

                def shot(page, name, form=form, scheme=scheme):
                    page.screenshot(path=f"{SHOTS}/{name}-{form}-{scheme}.png", full_page=True)
                print(f"== {form} {scheme}")
                ceph_flow(ctx, shot)
                kernel_flow(ctx, shot)
                about_flow(ctx, shot)
                ctx.close()
        browser.close()
    if failures:
        print(f"{len(failures)} failure(s)")
        sys.exit(1)
    print("ALL OK")


if __name__ == "__main__":
    main()
