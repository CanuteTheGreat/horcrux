#!/usr/bin/env python3
"""Access Log page e2e (Playwright Firefox, desktop AND phone width).

Makes real API traffic (a password session, an API token, failed requests),
then in the browser: filter bar -> table -> statistics panel -> click a
statistics value -> the filtered list. Also checks the page title, labelled
controls, that every statistics value and table value is a real <a href>,
exact start/end filtering, the row detail (link and double-click), the
settings panel and that the page renders without JavaScript errors.

Runs against a live horcrux-api serving the built UI (BASE_URL, default
https://localhost:8006, admin/admin), like ui_e2e.py.
"""
import json
import os
import ssl
import sys
import time
import urllib.error
import urllib.request
from urllib.parse import parse_qs, urlparse

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
WIDTHS = {"desktop": (1280, 900), "mobile": (390, 844)}
TOKEN_NAME = f"al-e2e-{int(time.time())}"


def req(method, path, tok=None, body=None, headers=None, ok_status=()):
    h = {"Content-Type": "application/json"}
    if tok:
        h["Authorization"] = f"Bearer {tok}"
    h.update(headers or {})
    r = urllib.request.Request(f"{BASE}{path}", method=method, headers=h,
                               data=json.dumps(body).encode() if body is not None else None)
    try:
        with urllib.request.urlopen(r, context=SSL) as resp:
            raw = resp.read()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        if e.code in ok_status:
            return e.code, None
        raise AssertionError(f"{method} {path} -> {e.code}: {e.read()[:300]}")


def seed():
    _, v = req("POST", "/api/auth/login", body={"username": "admin", "password": "admin"})
    tok = v["ticket"]
    _, k = req("POST", "/api/users/admin/api-keys", tok, {"name": TOKEN_NAME})
    key = k["key"]
    for _ in range(3):
        req("GET", "/api/vms", tok)
    for _ in range(2):
        req("GET", "/api/vms", headers={"X-API-Key": key})
    req("GET", "/api/vms/al-e2e-missing", tok, ok_status=(404,))
    req("GET", "/api/users", headers={"Authorization": "Bearer not-a-token"}, ok_status=(401,))
    req("GET", "/api/users", ok_status=(401,))
    return tok, key


def settle(page):
    expect(page.locator("nav.navbar[data-features-loaded='true']")).to_have_count(1, timeout=30000)
    page.wait_for_selector("#al-total", timeout=30000)
    page.wait_for_selector("#al-table tbody tr", timeout=30000)


def query(page):
    return {k: v[0] for k, v in parse_qs(urlparse(page.url).query).items()}


def run(tok, key):
    js_errors = []
    with sync_playwright() as p:
        browser = p.firefox.launch()
        for label, (w, h) in WIDTHS.items():
            ctx = browser.new_context(viewport={"width": w, "height": h}, ignore_https_errors=True)
            page = ctx.new_page()
            page.on("pageerror", lambda e: js_errors.append(str(e)))
            page.goto(f"{BASE}/login")
            page.fill("#login-username", "admin")
            page.fill("#login-password", "admin")
            page.locator("#login-submit").click()
            page.wait_for_url(f"{BASE}/")

            # own route, own title
            page.goto(f"{BASE}/access-log")
            settle(page)
            assert page.title() == "Access Log · Horcrux", page.title()
            assert page.locator("main h1").inner_text().strip() == "Access Log"

            # every form control is labelled
            for cid in ["al-start", "al-end", "al-user", "al-token", "al-client", "al-auth",
                        "al-method", "al-path", "al-group", "al-status", "al-node"]:
                assert page.locator(f"label[for='{cid}']").count() == 1, f"[{label}] no label for #{cid}"

            # ---- filter by API token through the form -> table + statistics agree
            page.fill("#al-token", TOKEN_NAME)
            page.locator("#al-apply").click()
            page.wait_for_function("() => location.search.includes('token=')")
            expect(page.locator("#al-count")).to_have_text("2 matching requests", timeout=30000)
            assert page.locator("#al-total").inner_text().strip() == "2"
            rows = page.locator("#al-table tbody tr.al-row")
            assert rows.count() == 2, f"[{label}] {rows.count()} rows for the token"
            assert TOKEN_NAME in rows.first.inner_text()
            assert query(page).get("token") == TOKEN_NAME

            # ---- statistics values are links; clicking one applies that filter
            tops = page.locator("#al-stats .al-top a")
            assert tops.count() >= 3
            for i in range(tops.count()):
                href = tops.nth(i).get_attribute("href")
                assert href and href.startswith("/access-log?"), f"[{label}] stats link {href!r}"
            page.locator("#al-stats .al-top[data-top='user'] a").first.click()
            page.wait_for_function("() => location.search.includes('user=admin')")
            q = query(page)
            assert q.get("user") == "admin" and q.get("token") == TOKEN_NAME, q
            expect(page.locator("#al-count")).to_have_text("2 matching requests", timeout=30000)
            assert page.input_value("#al-user") == "admin"

            # ---- clear, then the failed-request link of the summary
            page.locator("#al-clear").click()
            settle(page)
            page.locator("#al-errors-link").click()
            page.wait_for_function("() => location.search.includes('status=4xx')")
            # the old rows stay until the filtered answer arrives: wait for it
            page.wait_for_function(
                """() => { const t = [...document.querySelectorAll('#al-table tbody tr.al-row td:nth-child(8)')]
                    .map(e => e.innerText.trim()); return t.length > 0 && t.every(x => x.startsWith('4')); }""",
                timeout=30000)
            statuses = page.locator("#al-table tbody tr.al-row td:nth-child(8)").all_inner_texts()
            assert statuses and all(s.strip().startswith("4") for s in statuses), statuses
            page.wait_for_function("() => parseInt(document.querySelector('#al-total').innerText) >= 3", timeout=30000)
            n = int(page.locator("#al-total").inner_text().strip())
            assert n >= 3, f"[{label}] 4xx total {n}"

            # ---- exact start/end (second precision) filters the table
            first_time = page.locator("#al-table tbody tr.al-row td:nth-child(1)").first.inner_text().strip()
            stamp = first_time.split(".")[0].replace(" ", "T")
            page.goto(f"{BASE}/access-log?start={stamp}&end={stamp}")
            settle(page)
            page.wait_for_function(
                """(st) => { const t = [...document.querySelectorAll('#al-table tbody tr.al-row td:nth-child(1)')]
                    .map(e => e.innerText.trim()); return t.length > 0 && t.every(x => x.startsWith(st)); }""",
                arg=stamp.replace("T", " "), timeout=30000)
            times = page.locator("#al-table tbody tr.al-row td:nth-child(1)").all_inner_texts()
            assert times, f"[{label}] nothing in {stamp}"
            assert all(t.strip().startswith(stamp.replace("T", " ")) for t in times), times
            assert page.input_value("#al-start") == stamp

            # ---- a statistics bar narrows the time range
            page.goto(f"{BASE}/access-log?last=3600")
            settle(page)
            bar = page.locator("#al-stats .al-bar").last
            assert (bar.get_attribute("href") or "").startswith("/access-log?"), "bar is not a link"

            # ---- row detail: link and double-click
            page.goto(f"{BASE}/access-log?token={TOKEN_NAME}")
            settle(page)
            page.locator("#al-table tbody tr.al-row").first.locator("a", has_text="Details").click()
            page.wait_for_selector("#al-detail", timeout=30000)
            detail = page.locator("#al-detail").inner_text()
            assert TOKEN_NAME in detail and "GET /api/vms" in detail and key not in detail, detail
            page.locator("#al-detail a", has_text="Close").click()
            page.wait_for_selector("#al-detail", state="detached", timeout=30000)
            page.locator("#al-table tbody tr.al-row").first.dblclick()
            page.wait_for_selector("#al-detail", timeout=30000)

            # ---- nothing matching: an empty state, not an error
            page.goto(f"{BASE}/access-log?user=nobody-at-all")
            page.wait_for_function(
                "() => document.querySelector('#al-table td.no-data')?.innerText.includes('No requests match')",
                timeout=30000)
            assert "No requests match" in page.locator("#al-table").inner_text()
            assert page.locator("#al-error").count() == 0

            # ---- settings panel round trip
            page.goto(f"{BASE}/access-log")
            settle(page)
            page.locator("#al-settings summary").click()
            page.fill("#als-days", "21")
            page.locator("#als-save").click()
            expect(page.locator("#als-msg")).to_contain_text("saved", timeout=30000)
            for _i in range(60):
                _, s = req("GET", "/api/access-log/settings", tok)
                if s["retention_days"] == 21:
                    break
                time.sleep(0.5)
            assert s["retention_days"] == 21, s
            page.fill("#als-days", "30")
            page.locator("#als-save").click()
            expect(page.locator("#als-msg")).to_contain_text("saved", timeout=30000)
            for _i in range(60):
                _, s = req("GET", "/api/access-log/settings", tok)
                if s["retention_days"] == 30:
                    break
                time.sleep(0.5)
            assert s["retention_days"] == 30, s

            # ---- no horizontal page scroll
            fits = page.evaluate("document.documentElement.scrollWidth <= window.innerWidth + 1")
            assert fits, f"[{label}] page scrolls horizontally"
            assert not js_errors, f"[{label}] JS errors: {js_errors}"
            print(f"[{label}] ok")
            ctx.close()
        browser.close()


def main():
    tok, key = seed()
    try:
        run(tok, key)
    finally:
        _, keys = req("GET", "/api/users/admin/api-keys", tok)
        for k in keys or []:
            if k.get("name") == TOKEN_NAME:
                req("DELETE", f"/api/users/admin/api-keys/{k['id']}", tok, ok_status=(404,))
    print("ok - access log e2e")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as e:
        import traceback; traceback.print_exc(); print(f"FAIL: {e}")
        sys.exit(1)
