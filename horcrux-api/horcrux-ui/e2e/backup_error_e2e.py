#!/usr/bin/env python3
"""Backups page: a failed 'Back up now' must stay visible.

Regression: the page set the error, then reloaded the archive list, and the
reload's success path cleared it a moment later, so the user saw the button
re-enable and no message at all. The create call is answered with a 500 (a
full target disk) by a Playwright route; the message must appear and still be
there seconds later, after the reload has finished.
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
VID = "bkerr-e2e-vm"


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


def main():
    tok = req("POST", "/api/auth/login", body={"username": "admin", "password": "admin"})["ticket"]
    req("POST", "/api/vms", tok, {"id": VID, "name": VID, "hypervisor": "qemu",
                                 "architecture": "x86_64", "cpus": 1, "memory": 128,
                                 "disk_size": 1, "status": "stopped"})
    try:
        with sync_playwright() as p:
            b = p.firefox.launch()
            page = b.new_context(ignore_https_errors=True).new_page()
            page.goto(f"{BASE}/login")
            page.fill("#login-username", "admin")
            page.fill("#login-password", "admin")
            page.click("button[type=submit]")
            page.goto(f"{BASE}/backup")
            expect(page.locator("#backup-target option[value='%s']" % VID)).to_have_count(1, timeout=30000)
            page.select_option("#backup-target", VID)

            def fail(route):
                if route.request.method == "POST":
                    route.fulfill(status=500, content_type="application/json",
                                  body=json.dumps({"status": 500, "error": "INTERNAL_ERROR",
                                                   "message": "No space left on device"}))
                else:
                    route.continue_()
            page.route("**/api/backups", fail)
            page.click("#backup-now-btn")
            err = page.locator("#backup-error")
            expect(err).to_contain_text("failed", timeout=15000)
            page.wait_for_timeout(4000)  # the reload after the failure has long finished
            expect(err).to_contain_text("No space left on device")
            expect(page.locator("#backup-now-btn")).to_be_enabled()
            b.close()
        print("ok - failed backup message survives the list reload")
    finally:
        req("DELETE", f"/api/vms/{VID}", tok)


if __name__ == "__main__":
    sys.exit(main())
