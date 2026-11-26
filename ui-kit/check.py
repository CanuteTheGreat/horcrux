#!/usr/bin/env python3
"""ui-kit CI gate: size budgets (minified-ish), WCAG AA contrast of every text/background token pair in both palettes,
and no bare-element selectors in components.css (adoption page by page must not fight existing CSS).
usage: python3 ui-kit/check.py"""
import os, re, sys

D = os.path.dirname(os.path.abspath(__file__))
fail = []


def mini(s):
    s = re.sub(r"/\*.*?\*/", "", s, flags=re.S)
    return re.sub(r"\s+", " ", s).strip()


css = mini(open(f"{D}/tokens.css").read()) + mini(open(f"{D}/components.css").read())
js = mini(open(f"{D}/ui.js").read())
if len(css.encode()) > 30 * 1024:
    fail.append(f"tokens+components {len(css)} B > 30 KB budget")
if len(js.encode()) > 15 * 1024:
    fail.append(f"ui.js {len(js)} B > 15 KB budget")

# every selector in components.css starts with .ui-, :root, @ or a keyframe step
comp = re.sub(r"/\*.*?\*/", "", open(f"{D}/components.css").read(), flags=re.S)
for sel in re.findall(r"(?:^|})\s*([^{}@]+?)\s*{", comp):
    for part in sel.split(","):
        p = part.strip()
        if not p or re.fullmatch(r"(from|to|\d+%)", p):
            continue
        if not p.startswith(".ui-"):
            fail.append(f"components.css selector not ui- prefixed: {p}")


def blocks(src):
    """{'light': {var: value}, 'dark': {...}} from tokens.css"""
    src = re.sub(r"/\*.*?\*/", "", src, flags=re.S)
    light, dark = {}, {}
    m = re.search(r":root\s*{(.*?)}", src, re.S)
    for k, v in re.findall(r"(--[\w-]+)\s*:\s*([^;]+);", m.group(1)):
        light[k] = v.strip()
    m = re.search(r':root\[data-theme="dark"\]\s*{(.*?)}', src, re.S)
    dark = dict(light)
    for k, v in re.findall(r"(--[\w-]+)\s*:\s*([^;]+);", m.group(1)):
        dark[k] = v.strip()
    return {"light": light, "dark": dark}


def lum(h):
    h = h.lstrip("#")
    r, g, b = (int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))
    f = lambda c: c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)


def ratio(a, b):
    x, y = sorted((lum(a), lum(b)), reverse=True)
    return (x + 0.05) / (y + 0.05)


PAIRS = [("--c-ink", "--c-surface"), ("--c-ink", "--c-canvas"), ("--c-ink", "--c-sunken"), ("--c-muted", "--c-surface"),
         ("--c-muted", "--c-sunken"), ("--c-muted", "--c-canvas"), ("--c-accent", "--c-surface"), ("--c-accent", "--c-accent-subtle"),
         ("--c-on-solid", "--c-accent-solid"), ("--c-ok", "--c-ok-bg"), ("--c-warn", "--c-warn-bg"), ("--c-crit", "--c-crit-bg"),
         ("--c-info", "--c-info-bg"), ("--c-on-solid", "--c-crit-solid"), ("--c-on-solid", "--c-ok-solid"), ("--c-ok", "--c-surface"),
         ("--c-warn", "--c-surface"), ("--c-crit", "--c-surface"), ("--c-info", "--c-surface"), ("--c-chrome-ink", "--c-chrome"),
         ("--c-chrome-muted", "--c-chrome"), ("--c-ink", "--c-row-hover"), ("--c-accent", "--c-row-hover")]
for scheme, t in blocks(open(f"{D}/tokens.css").read()).items():
    for fg, bg in PAIRS:
        a, b = t.get(fg), t.get(bg)
        if not (a and b and a.startswith("#") and b.startswith("#")):
            fail.append(f"{scheme}: {fg}/{bg} not a hex token")
            continue
        r = ratio(a, b)
        if r < 4.5:
            fail.append(f"{scheme}: {fg} {a} on {bg} {b} = {r:.2f} < 4.5")
print(f"ui-kit: css {len(css)} B, js {len(js)} B, {len(PAIRS) * 2} contrast pairs")
for f in fail:
    print("FAIL", f)
sys.exit(1 if fail else 0)
