#!/usr/bin/env python3
"""Every CSS class a UI view uses must have a rule in style/main.css (or a reason in css-class-allowlist.txt).

Owner-findings rows 25, 28, 31 and 34: pages rendered as run-together stacked text because the classes their markup uses
were never defined. This is the deterministic check for that class of bug: it lists class names used in
horcrux-api/horcrux-ui/src/**/*.rs (class="a b", class=format!("a-{}"), class:name=) that no selector in main.css matches.

  * `.ha-page .x` rules only count for source files that mention `ha-page` (they apply nowhere else).
  * A dynamic class (`format!("usage-{}", c)`) needs at least one rule starting with `usage-`.
  * css-class-allowlist.txt: `name reason` or `prefix* reason` (hook classes that deliberately carry no style).
  * css-class-baseline.txt: `file class` pairs of known debt on pages not yet styled. The check fails on any pair
    that is NOT in the baseline (no new debt) and on baseline pairs that are now fixed (the list only shrinks).

Pure python, no browser.  Usage: check_css_classes.py [repo-root] [--list] [--write-baseline]
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
UI = "horcrux-api/horcrux-ui"
ALLOW = os.path.join(HERE, "css-class-allowlist.txt")
BASELINE = os.path.join(HERE, "css-class-baseline.txt")
SCOPE_ROOT = "ha-page"

CLASS_ATTR = re.compile(r'\bclass\s*=\s*"([^"]*)"')
FORMAT_CLASS = re.compile(r'\bclass\s*=\s*\{?\s*(?:move\s*\|\|\s*)?format!\(\s*"([^"]*)"')
CLASS_DIRECTIVE = re.compile(r"\bclass:([A-Za-z_][\w-]*)\s*=")
CSS_CLASS = re.compile(r"\.((?:[\w-]|\\.)+)")
UTIL_LITERAL = re.compile(r"\b((?:text|bg)-(?:red|orange|yellow|green|blue|purple|gray)-\d{2,3})\b")
TOKEN = re.compile(r"^-?[A-Za-z_][\w:/.-]*$")


def css_classes(css):
    """(global names, names only defined under .ha-page) from main.css selectors."""
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    glob, scoped = set(), set()
    for sels in re.findall(r"([^{}]+)\{", css):
        if sels.strip().startswith("@"):
            continue
        for sel in sels.split(","):
            sel = sel.strip()
            sel = re.sub(r'"[^"]*"|\'[^\']*\'|\[[^\]]*\]', "", sel)
            names = [re.sub(r"\\(.)", r"\1", n) for n in CSS_CLASS.findall(sel)]
            if names and names[0] == SCOPE_ROOT:
                scoped.update(names[1:])
            else:
                glob.update(names)
    return glob, scoped


def used_classes(src_dir):
    """({(file, class): first line}, files mentioning ha-page). Holes in a plain class string are dropped; format! holes make `prefix*`."""
    used = {}
    scoped_files = set()
    for dp, _, fns in os.walk(src_dir):
        for fn in sorted(fns):
            if not fn.endswith(".rs"):
                continue
            path = os.path.join(dp, fn)
            rel = os.path.relpath(path, src_dir)
            text = open(path, encoding="utf-8", errors="replace").read()
            if SCOPE_ROOT in text:
                scoped_files.add(rel)

            def add(tok, pos):
                used.setdefault((rel, tok), text.count("\n", 0, pos) + 1)

            for m in CLASS_ATTR.finditer(text):
                for tok in re.sub(r"\{[^{}]*\}", " ", m.group(1)).split():
                    if TOKEN.match(tok):
                        add(tok, m.start())
            for m in FORMAT_CLASS.finditer(text):
                for tok in m.group(1).split():
                    if "{" in tok:
                        pre = tok.split("{")[0]
                        if pre:
                            add(pre + "*", m.start())
                    elif TOKEN.match(tok):
                        add(tok, m.start())
            for m in CLASS_DIRECTIVE.finditer(text):
                add(m.group(1), m.start())
            # colour classes returned from helper closures (`"text-red-600 bg-red-50"`) never sit in a class= attribute
            for m in UTIL_LITERAL.finditer(text):
                add(m.group(1), m.start())
    return used, scoped_files


def load_allow(path=ALLOW):
    exact, prefixes = {}, {}
    if os.path.exists(path):
        for n, ln in enumerate(open(path, encoding="utf-8"), 1):
            ln = ln.strip()
            if not ln or ln.startswith("#"):
                continue
            name, _, reason = ln.partition(" ")
            if not reason.strip():
                raise SystemExit(f"{path}:{n}: allow-list entry '{name}' needs a reason")
            (prefixes if name.endswith("*") else exact)[name.rstrip("*")] = reason.strip()
    return exact, prefixes


def load_baseline(path=BASELINE):
    out = set()
    if os.path.exists(path):
        for ln in open(path, encoding="utf-8"):
            ln = ln.strip()
            if ln and not ln.startswith("#"):
                f, _, c = ln.partition(" ")
                out.add((f, c.strip()))
    return out


def undefined(root=ROOT):
    """Sorted [(file, class, line)] used in markup, defined nowhere, not allow-listed."""
    css = open(os.path.join(root, UI, "style/main.css"), encoding="utf-8").read()
    glob, scoped = css_classes(css)
    glob.add(SCOPE_ROOT)  # the scope marker itself (rules exist as `.ha-page ...`)
    exact, prefixes = load_allow()
    used, scoped_files = used_classes(os.path.join(root, UI, "src"))
    out = []
    for (f, c), line in used.items():
        visible = glob | (scoped if f in scoped_files else set())
        if c.endswith("*"):
            ok = any(n.startswith(c[:-1]) for n in visible) or c[:-1] in prefixes
        else:
            ok = c in visible or c in exact or any(c.startswith(p) for p in prefixes)
        if not ok:
            out.append((f, c, line))
    return sorted(out)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    root = args[0] if args else ROOT
    missing = undefined(root)
    if "--write-baseline" in sys.argv:
        with open(BASELINE, "w", encoding="utf-8") as fh:
            fh.write("# Known debt: classes used by pages that nobody has styled yet (file class). Generated by\n"
                     "# check_css_classes.py --write-baseline. The list may only shrink: fix a page, then regenerate.\n")
            for f, c, _ in missing:
                fh.write(f"{f} {c}\n")
        print(f"baseline written: {len(missing)} pairs")
        return 0
    if "--list" in sys.argv:
        for f, c, line in missing:
            print(f"{f}:{line}  .{c}")
        print(f"css-classes: {len(missing)} (file, class) pairs undefined")
        return 0
    base = load_baseline()
    cur = {(f, c) for f, c, _ in missing}
    new = [(f, c, ln) for f, c, ln in missing if (f, c) not in base]
    fixed = sorted(base - cur)
    print(f"css-classes: {len(cur)} undefined (file, class) pairs, {len(base)} in baseline, {len(new)} new, {len(fixed)} fixed-but-listed")
    for f, c, ln in new[:80]:
        print(f"  NEW  {f}:{ln}  .{c}  has no rule in style/main.css")
    for f, c in fixed[:80]:
        print(f"  STALE baseline entry (now defined): {f} {c}  -> run check_css_classes.py --write-baseline")
    if new:
        print("Define the classes in horcrux-api/horcrux-ui/style/main.css, or list hook-only classes with a reason in "
              "ci/ux-qa/css-class-allowlist.txt")
    return 1 if (new or fixed) else 0


if __name__ == "__main__":
    sys.exit(main())
