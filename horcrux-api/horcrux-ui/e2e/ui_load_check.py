#!/usr/bin/env python3
"""Headless smoke check of a deployed Horcrux web UI: it must render and throw no JS errors.

usage: ui_load_check.py https://host:8006

Loads /login in Chromium (or Firefox with BROWSER=firefox; cert errors ignored) and fails
on any uncaught exception (a wasm-bindgen/crate mismatch shows up as
`WebAssembly.Table.grow(): failed to grow table` and leaves a BLANK page) or when the
login form never appears. Needs `pip install playwright && playwright install chromium`.
"""
import os
import sys

from playwright.sync_api import sync_playwright

base = (sys.argv[1] if len(sys.argv) > 1 else "https://127.0.0.1:8006").rstrip("/")
errors = []
with sync_playwright() as p:
    kind = getattr(p, os.environ.get("BROWSER", "chromium"))
    browser = kind.launch()
    ctx = browser.new_context(ignore_https_errors=True)
    page = ctx.new_page()
    page.on("pageerror", lambda e: errors.append(f"pageerror: {e}"))
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text and errors.append(f"console: {m.text}"))
    page.goto(f"{base}/login", wait_until="load")
    try:
        page.locator("input[type=password]").wait_for(timeout=20000)
    except Exception as e:  # noqa: BLE001
        errors.append(f"login form did not render: {e}")
    body = page.inner_text("body").strip()
    if not body:
        errors.append("page body is empty (blank UI)")
    browser.close()
if errors:
    print("UI LOAD CHECK FAILED:\n  " + "\n  ".join(errors))
    sys.exit(1)
print(f"UI load check OK: {base}/login rendered, zero JS errors")
