#!/usr/bin/env python3
"""Owner rule 2026-10-30: the web UI follows the device's light/dark setting by default.

Static regression check (no browser, runs in the lint job): index.html declares both schemes and a
theme-color per scheme, style/main.css has a prefers-color-scheme: dark palette and never pins one
scheme, and the text/background pairs the UI really uses are >= 4.5:1 in BOTH palettes. The rendered
proof (every text node, both schemes, live switch, real pages) is theme_checks() in e2e/ui_e2e.py.
"""
import pathlib
import re
import sys

here = pathlib.Path(__file__).resolve().parent.parent
css = (here / "style" / "main.css").read_text()
html = (here / "index.html").read_text()
bad = []

if 'name="color-scheme" content="light dark"' not in html:
    bad.append("index.html: <meta name=color-scheme content='light dark'> missing")
for sch in ("light", "dark"):
    if not re.search(r'name="theme-color"[^>]*media="\(prefers-color-scheme: %s\)"' % sch, html):
        bad.append(f"index.html: theme-color for {sch} missing")
if re.search(r"color-scheme:\s*(dark|light)\s*;", css):
    bad.append("style/main.css pins color-scheme to one scheme")
if not re.search(r"color-scheme:\s*light dark\s*;", css):
    bad.append("style/main.css: :root must declare color-scheme: light dark (form controls, scrollbars)")
m = re.search(r"@media \(prefers-color-scheme: dark\) \{\s*:root \{(.*?)\n    \}\n\}", css, re.S)
if not m:
    bad.append("style/main.css: no @media (prefers-color-scheme: dark) { :root { ... } } palette")
root = re.search(r":root \{(.*?)\n\}", css, re.S).group(1)


def tokens(block):
    return {k: v.strip() for k, v in re.findall(r"(--[\w-]+):\s*([^;]+);", block)}


light = tokens(root)
dark = {**light, **(tokens(m.group(1)) if m else {})}


def rgb(v, pal):
    v = v.strip()
    mv = re.match(r"var\((--[\w-]+)\)", v)
    if mv:
        return rgb(pal[mv.group(1)], pal)
    mh = re.match(r"#([0-9a-fA-F]{6})$", v)
    if mh:
        return tuple(int(mh.group(1)[i:i + 2], 16) for i in (0, 2, 4))
    if v == "white":
        return (255, 255, 255)
    raise ValueError(v)


def lum(c):
    f = lambda x: (x / 255 / 12.92) if x / 255 <= 0.03928 else ((x / 255 + 0.055) / 1.055) ** 2.4
    return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])


def ratio(a, b):
    hi, lo = sorted((lum(a), lum(b)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)


# (what, text, background) as the stylesheet uses them
PAIRS = [
    ("body text on page", "var(--text)", "var(--light)"),
    ("body text on card", "var(--text)", "var(--surface)"),
    ("muted text on card", "var(--secondary)", "var(--surface)"),
    ("muted text on page", "var(--secondary)", "var(--light)"),
    ("accent text on card", "var(--primary)", "var(--surface)"),
    ("accent text on page", "var(--primary)", "var(--light)"),
    ("success text on card", "var(--success)", "var(--surface)"),
    ("warning text on card", "var(--warning-text)", "var(--surface)"),
    ("danger text on card", "var(--danger)", "var(--surface)"),
    ("info text on card", "var(--info)", "var(--surface)"),
    ("danger text on error banner", "var(--danger)", "var(--crit-bg)"),
    ("text on ok alert", "var(--text)", "var(--ok-bg)"),
    ("text on warning alert", "var(--text)", "var(--warn-bg)"),
    ("text on error alert", "var(--text)", "var(--bad-bg)"),
    ("text on info alert", "var(--text)", "var(--note-bg)"),
    ("white on primary button", "white", "var(--primary-solid)"),
    ("white on secondary button", "white", "var(--secondary-solid)"),
    ("white on success button", "white", "var(--success-solid)"),
    ("white on danger button", "white", "var(--danger-solid)"),
    ("white on navbar", "white", "var(--dark)"),
    ("dark text on warning button", "#212b36", "var(--warning)"),
]
for name, pal in (("light", light), ("dark", dark)):
    for what, fg, bg in PAIRS:
        try:
            r = ratio(rgb(fg, pal), rgb(bg, pal))
        except (KeyError, ValueError) as e:
            bad.append(f"{name}: {what}: cannot resolve {e}")
            continue
        if r < 4.5:
            bad.append(f"{name}: {what} = {r:.2f}:1 (< 4.5)")

if bad:
    print("check_theme: FAIL")
    print("\n".join(bad))
    sys.exit(1)
print("check_theme: OK (light + dark palettes, %d pairs each)" % len(PAIRS))
