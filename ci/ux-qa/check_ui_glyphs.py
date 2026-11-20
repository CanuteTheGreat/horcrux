#!/usr/bin/env python3
"""UI source must not use ASCII-art status markers ("[OK]", "[X]", "[!]", "[D]") as visible text.

Found by the UX lane: the GPU page showed "[X]No IOMMU groups..." and the SMART page "[OK]"/"[X]" stat icons.
Use a real glyph (check, cross, warning sign) or an icon instead.

usage: check_ui_glyphs.py [UI_SRC_DIR]
"""
import pathlib
import re
import sys

PAT = re.compile(r'"\s*\[(?:OK|X|!|D|WARN|ERR|INFO)\]|>\s*\[(?:OK|X|!|D)\]')


def scan(root):
    probs = []
    for p in sorted(pathlib.Path(root).rglob("*.rs")):
        for n, line in enumerate(p.read_text(errors="replace").splitlines(), 1):
            if PAT.search(line):
                probs.append(f"{p}:{n}: ASCII status marker in UI text: {line.strip()[:90]}")
    return probs


if __name__ == "__main__":
    root = sys.argv[1] if len(sys.argv) > 1 else "horcrux-api/horcrux-ui/src"
    probs = scan(root)
    print("\n".join(probs) or "check_ui_glyphs: ok")
    sys.exit(1 if probs else 0)
