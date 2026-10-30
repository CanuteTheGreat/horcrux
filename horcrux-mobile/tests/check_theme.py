#!/usr/bin/env python3
"""Owner rule 2026-10-30: the mobile UI follows the device's light/dark setting.

Static regression check (no browser): index.html declares both schemes + a theme-color per scheme,
style.css has a prefers-color-scheme: dark block and never pins one scheme, and every text/background
pair in BOTH palettes is >= 4.5:1 (3:1 for large/graphic). Rendered proof: tests/render_theme.py.
"""
import pathlib, re, sys
here = pathlib.Path(__file__).resolve().parent.parent
css = (here / "style.css").read_text()
html = (here / "index.html").read_text()
bad = []
if 'name="color-scheme" content="light dark"' not in html: bad.append("index.html: color-scheme meta must be 'light dark'")
for sch in ("light", "dark"):
    if not re.search(r'name="theme-color"[^>]*media="\(prefers-color-scheme: %s\)"' % sch, html):
        bad.append(f"index.html: theme-color for {sch} missing")
if re.search(r"color-scheme:\s*(dark|light)\s*;", css): bad.append("style.css pins color-scheme to one scheme")
m = re.search(r"@media \(prefers-color-scheme: dark\) \{(.*)\n\}", css, re.S)
if not m: bad.append("style.css: no prefers-color-scheme: dark block")

def rules(text):
    out = {}
    for sm in re.finditer(r"([^{}]+)\{([^{}]*)\}", text):
        decl = dict((k.strip(), v.strip()) for k, v in (d.split(":", 1) for d in sm.group(2).split(";") if ":" in d))
        for sel in sm.group(1).split(","):
            sel = sel.strip()
            if sel and not sel.startswith("@") and not sel.startswith("/*"):
                out.setdefault(sel, {}).update(decl)
    return out

def hexrgb(v):
    mm = re.search(r"#([0-9a-fA-F]{6})\b", v or "")
    if mm: return tuple(int(mm.group(1)[i:i + 2], 16) for i in (0, 2, 4))
    if v and v.strip() == "white": return (255, 255, 255)
def lum(c):
    f = lambda x: (x / 255 / 12.92) if x / 255 <= 0.03928 else ((x / 255 + 0.055) / 1.055) ** 2.4
    return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
def ratio(a, b):
    hi, lo = sorted((lum(a), lum(b)), reverse=True); return (hi + 0.05) / (lo + 0.05)

light = rules(re.sub(r"@media \(prefers-color-scheme: dark\) \{.*\n\}", "", css, flags=re.S))
dark = {**light}
if m:
    for sel, d in rules(m.group(1)).items(): dark[sel] = {**light.get(sel, {}), **d}
# (selector for text, background selector) pairs actually used in the app
PAGE = ("body", "body")
pairs = [("body", "body"), (".card-title", ".card"), (".stat-label", ".card"), (".stat-value", ".card"), (".label", ".card"),
         (".value", ".card"), (".value.success", ".card"), (".value.error", ".card"), (".nav-item", ".bottom-nav"),
         (".status-running", ".status-running"), (".status-stopped", ".status-stopped"), (".status-paused", ".status-paused"),
         (".status-error", ".status-error"), (".error-message", ".error-message"), (".node-badge", ".node-badge"),
         (".mobile-header", ".mobile-header"), (".mobile-button.primary", ".mobile-button.primary"),
         (".mobile-button.success", ".mobile-button.success"), (".mobile-button.danger", ".mobile-button.danger"),
         (".action-button", ".action-button"), (".empty-state", "body"), (".loading-container", "body"), (".node-details", ".card")]
for name, pal in (("light", light), ("dark", dark)):
    for tsel, bsel in pairs:
        fg = hexrgb(pal.get(tsel, {}).get("color")) or hexrgb(pal.get("body", {}).get("color"))
        bgv = pal.get(bsel, {}).get("background") or pal.get(bsel, {}).get("background-color")
        bgc = hexrgb(bgv)
        if fg and bgc and ratio(fg, bgc) < 4.5:
            bad.append(f"{name}: {tsel} on {bsel} = {ratio(fg, bgc):.2f}:1 (< 4.5)")
if bad:
    print("check_theme: FAIL"); print("\n".join(bad)); sys.exit(1)
print("check_theme: OK (both palettes)")
