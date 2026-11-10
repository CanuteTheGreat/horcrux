#!/usr/bin/env python3
"""Render tests/harness.html at 390px with prefers-color-scheme light AND dark (Playwright), assert the
page background follows the scheme (live switch, no reload) and save PNGs: render_theme.py OUTDIR"""
import pathlib, sys
from playwright.sync_api import sync_playwright
out = pathlib.Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
url = (pathlib.Path(__file__).parent / "harness.html").resolve().as_uri()
lum = lambda rgb: sum(w * (((c / 255) / 12.92) if c / 255 <= .03928 else (((c / 255) + .055) / 1.055) ** 2.4) for w, c in zip((.2126, .7152, .0722), rgb))
import re
with sync_playwright() as p:
    b = p.chromium.launch(**({"executable_path": sys.argv[2]} if len(sys.argv) > 2 else {}))
    pg = b.new_page(viewport={"width": 390, "height": 844}, device_scale_factor=2, is_mobile=True)
    pg.emulate_media(color_scheme="light"); pg.goto(url); pg.add_style_tag(content=".bottom-nav{position:static!important}")  # full-page capture: no fixed overlay
    bg = lambda: [float(x) for x in re.findall(r"[\d.]+", pg.evaluate("getComputedStyle(document.body).backgroundColor"))[:3]]
    assert lum(bg()) > 0.8, "light"; pg.wait_for_timeout(400); pg.screenshot(path=str(out / "harness-light.png"), full_page=True)
    pg.emulate_media(color_scheme="dark")  # live, no reload
    assert lum(bg()) < 0.05, "dark (live)"; pg.wait_for_timeout(400); pg.screenshot(path=str(out / "harness-dark.png"), full_page=True)
    b.close()
print("render_theme: OK")
