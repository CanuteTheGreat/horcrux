#!/usr/bin/env python3
"""Gate for the shared map component (ui-kit/map): size budget, class prefix, pure-function tests.

* map.js <= 24 KB and map.css <= 6 KB as served (unminified): the router tier serves them too;
* every class map.css defines starts with ui-map (scoped) or styles a kit class inside .ui-map;
* `node test.js` passes (layout + Mermaid export); skipped with a notice when node is missing.
"""
import pathlib, re, shutil, subprocess, sys

D = pathlib.Path(__file__).resolve().parent
bad = []
js, css = (D / "map.js").read_bytes(), (D / "map.css").read_text()
if len(js) > 24 * 1024:
    bad.append(f"map.js {len(js)} B > 24 KB")
if len(css.encode()) > 6 * 1024:
    bad.append(f"map.css {len(css.encode())} B > 6 KB")
def selectors(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    depth, buf = 0, ""
    for ch in text:
        if ch == "{":
            sel = buf.strip()
            if not sel.startswith("@"):
                yield sel
            depth, buf = depth + 1, ""
        elif ch == "}":
            depth, buf = depth - 1, ""
        elif ch == ";":
            buf = ""
        else:
            buf += ch


for sel in selectors(css):
    for part in sel.split(","):
        part = part.strip()
        if part and not part.startswith(".ui-map"):
            bad.append(f"map.css: selector not scoped to .ui-map: {part}")
node = shutil.which("node") or shutil.which("nodejs")
if node:
    r = subprocess.run([node, str(D / "test.js")], capture_output=True, text=True)
    if r.returncode != 0:
        bad.append("test.js failed:\n" + r.stdout + r.stderr)
else:
    print("ui-kit/map check: node not found, test.js skipped")
if bad:
    print("ui-kit/map check: FAIL\n" + "\n".join(bad))
    sys.exit(1)
print(f"ui-kit/map check: OK (map.js {len(js)} B, map.css {len(css.encode())} B)")
