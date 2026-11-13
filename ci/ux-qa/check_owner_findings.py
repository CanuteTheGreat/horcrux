#!/usr/bin/env python3
"""docs/qa/owner-findings.md is the learning loop: every issue the owner found by hand, and the check that now covers its CLASS.

This gate keeps the loop honest: every table row must name a check (a path in backticks) that exists in the repo, or a
`cron:<job>` / `manual:<why>` marker.  A row with '(none yet)' fails: the owner rule is that a bug found by hand always
gets a check for its class, so an unchecked row is a debt that CI must show.

usage: check_owner_findings.py [FILE] [REPO_ROOT]
"""
import os
import re
import sys


def check(path, root):
    probs = []
    rows = [l for l in open(path) if l.startswith("|") and not re.match(r"^\|[\s:|-]+\|$", l) and not re.match(r"^\|\s*#\s*\|", l)]
    if not rows:
        probs.append("no findings rows")
    for l in rows:
        cells = [c.strip() for c in l.strip().strip("|").split("|")]
        if len(cells) < 5:
            probs.append(f"malformed row: {l.strip()[:60]}")
            continue
        n, check_cell = cells[0], cells[-1]
        refs = re.findall(r"`([^`]+)`", check_cell)
        paths = [r for r in refs if "/" in r or r.endswith((".py", ".sh", ".yml", ".rs", ".mjs"))]
        if re.search(r"none yet|todo|tbd", check_cell, re.I) or not refs:
            probs.append(f"row {n}: no check covers this finding")
            continue
        if re.match(r"^(cron|manual):", refs[0]):
            continue
        for r in paths:
            r = r.split("::")[0].split(" ")[0]
            if not os.path.exists(os.path.join(root, r)):
                probs.append(f"row {n}: check `{r}` does not exist")
    return probs


if __name__ == "__main__":
    root = sys.argv[2] if len(sys.argv) > 2 else "."
    f = sys.argv[1] if len(sys.argv) > 1 else os.path.join(root, "docs/qa/owner-findings.md")
    p = check(f, root)
    for x in p:
        print("FAIL", x)
    print(f"check_owner_findings: {len(p)} problem(s)")
    sys.exit(1 if p else 0)
