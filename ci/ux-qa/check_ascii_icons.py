#!/usr/bin/env python3
"""Fail when the UI uses ASCII "[OK]" / "[X]" text markers as status icons (owner-findings class: placeholder glyphs)."""
import pathlib
import re
import sys

PAT = re.compile(r"\[(OK|X|WARN|FAIL)\]")


def scan(root):
    hits = []
    for p in sorted(pathlib.Path(root).rglob("*.rs")):
        for n, line in enumerate(p.read_text(errors="replace").splitlines(), 1):
            if PAT.search(line):
                hits.append(f"{p}:{n}: {line.strip()[:100]}")
    return hits


if __name__ == "__main__":
    hits = scan(sys.argv[1] if len(sys.argv) > 1 else "horcrux-api/horcrux-ui/src")
    print("\n".join(hits) or "ok: no ASCII status markers")
    sys.exit(1 if hits else 0)
