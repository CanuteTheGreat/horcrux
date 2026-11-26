#!/usr/bin/env python3
"""Public product site: no unreviewed negative-capability claim ships (owner 2026-11-26).

The owner found "Secure Boot is not supported" / "must be off" on the product site and asked why QA had not. Such a
sentence is either a gap to close or an accurate disclosure; it must never sit on a public page unreviewed. This gate
scans the BUILT site (site/dist, before deploy) and fails on every sentence that says a capability is missing
("not supported", "unsupported", "must be off/disabled", "coming soon", "not yet", "TODO/TBD", "doesn't work/support",
"work in progress") unless site/claims-reviewed.txt lists it with a reason and an expiry date.

  site/claims-reviewed.txt:   REGEX  # reason (until YYYY-MM-DD)
  REGEX is matched case-insensitively against "<page path>|<sentence>". An expired line fails again (re-check the claim
  against the product: close the gap or extend with a fresh reason). Max 90 days ahead.

Scope: the sales pages (every page outside docs/, the 404 page excluded). docs/ pages are covered by the live public-pages
QA lane (cron public-pages-qa, ledger findings, never silent).
usage: check_site_claims.py [DIST] [--reviewed FILE] [--all]
"""
import datetime as dt
import html as htmllib
import os
import re
import sys

CLAIMS = [
    (r"\bnot (?:yet |currently |officially )?(?:supported|implemented|ready)\b|\bnot (?:yet )?available yet\b|\bnot available yet\b", "not-x"),
    (r"\bnot (?:yet )?tested on (?:real|the real|hardware)", "not-tested"),
    (r"\bunsupported\b", "unsupported"),
    (r"\bmust (?:be )?(?:disabled?|turned off|switched off|off)\b", "must-disable"),
    (r"\b(?:disable|turn off|switch off|turn) (?:secure ?boot|tpm|selinux|apparmor|the firewall)(?: off)?\b", "must-disable"),
    (r"\bcoming soon\b", "coming-soon"),
    (r"\bnot yet\b", "not-yet"),
    (r"(?<![A-Za-z])(?:TODO|TBD|FIXME)(?![A-Za-z])", "todo"),
    (r"\b(?:doesn't|does not|won't|will not) (?:yet )?work\b|\b(?:doesn't|does not|don't|do not|can't|cannot) (?:yet )?support\b", "doesnt-work"),
    (r"\bunder construction\b|\bwork in progress\b|\blorem ipsum\b", "wip"),
]
CLAIMS = [(re.compile(p, 0 if k == "todo" else re.I), k) for p, k in CLAIMS]
MAX_AHEAD_DAYS = 90


def visible_text(h):
    s = re.sub(r"(?is)<(script|style|pre|code|svg|noscript|template|video|audio|object|iframe)\b.*?</\1>", " ", h)
    s = re.sub(r"(?is)<!--.*?-->", " ", s)
    s = re.sub(r"(?i)</?(p|div|li|h[1-6]|tr|td|th|br|section|article|header|footer|nav|ul|ol|table|dd|dt|blockquote|figcaption|summary|details)\b[^>]*>", "\n", s)
    s = htmllib.unescape(re.sub(r"(?s)<[^>]+>", " ", s))
    return "\n".join(re.sub(r"[ \t\r\f\v\u00a0]+", " ", ln).strip() for ln in s.split("\n") if ln.strip())


def sentences(text):
    for block in text.split("\n"):
        for s in re.split(r"(?<=[.!?])\s+(?=[A-Z0-9\"'(])", block):
            s = s.strip()
            if len(s) >= 6:
                yield s


def load_reviewed(path, today):
    out, probs = [], []
    if not os.path.exists(path):
        return out, probs
    for n, ln in enumerate(open(path, encoding="utf-8"), 1):
        ln = ln.rstrip("\n")
        if not ln.strip() or ln.lstrip().startswith("#"):
            continue
        rx, sep, why = ln.partition("  # ")
        until = re.search(r"until (\d{4}-\d{2}-\d{2})", why)
        if not sep or not why.strip() or not until:
            probs.append(f"{path}:{n}: needs '  # reason (until YYYY-MM-DD)'")
            continue
        if (dt.date.fromisoformat(until.group(1)) - today).days > MAX_AHEAD_DAYS:
            probs.append(f"{path}:{n}: expiry more than {MAX_AHEAD_DAYS} days ahead")
        try:
            out.append((re.compile(rx.strip(), re.I), until.group(1), n))
        except re.error as e:
            probs.append(f"{path}:{n}: bad regex: {e}")
    return out, probs


def scan(dist, reviewed, today, all_pages=False):
    """-> (problems, used_line_numbers)"""
    probs, used = [], set()
    for dp, _, fs in os.walk(dist):
        for f in sorted(fs):
            if not f.endswith(".html"):
                continue
            p = os.path.join(dp, f)
            rel = "/" + os.path.relpath(p, dist).replace(os.sep, "/")
            if f == "404.html" or (not all_pages and rel.startswith("/docs/")):
                continue
            path = rel[:-len("index.html")] if rel.endswith("index.html") else rel
            for s in sentences(visible_text(open(p, encoding="utf-8").read())):
                for rx, kind in CLAIMS:
                    if not rx.search(s):
                        continue
                    key = f"{path}|{s}"
                    hit = next((r for r in reviewed if r[0].search(key)), None)
                    if hit and hit[1] >= today.isoformat():
                        used.add(hit[2])
                    elif hit:
                        probs.append(f"EXPIRED review (line {hit[2]}, until {hit[1]}) [{kind}] {path}: {s[:200]}")
                    else:
                        probs.append(f"UNREVIEWED [{kind}] {path}: {s[:200]}\n    close the gap and change the text, or add to claims-reviewed.txt:\n"
                                     f"    ^{re.escape(path)}\\|{re.escape(s[:60])}  # <why it is accurate> (until {(today + dt.timedelta(days=60)).isoformat()})")
                    break
    return probs, used


def main(argv):
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    dist = next((a for a in argv if not a.startswith("--") and argv[argv.index(a) - 1] != "--reviewed"), os.path.join(root, "site/dist"))
    rf = argv[argv.index("--reviewed") + 1] if "--reviewed" in argv else os.path.join(root, "site/claims-reviewed.txt")
    today = dt.date.today()
    reviewed, probs = load_reviewed(rf, today)
    p2, used = scan(dist, reviewed, today, "--all" in argv)
    probs += p2
    for r in reviewed:
        if r[2] not in used and r[1] >= today.isoformat():
            print(f"note: claims-reviewed line {r[2]} matches nothing any more (the claim is gone: delete the line)")
    for x in probs:
        print("FAIL", x)
    print(f"check_site_claims: {len(probs)} problem(s)")
    return 1 if probs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
