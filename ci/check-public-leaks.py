#!/usr/bin/env python3
"""Public-repo leak guard: fail if owner-identifying material is in tracked files or (with --commits RANGE) commit messages.

Owner-specific hostnames/usernames are matched by SHA-256 of the lowercase word token, so this public script does not itself name them.
Also flagged: the owner's home LAN prefix, home-directory paths, and owner presence language (owner is away/active/home...).
Owner-specific content belongs in the PRIVATE overlay (ctg-private-overlay), never here. Usage:
  ci/check-public-leaks.py                 # scan tracked files (git ls-files)
  ci/check-public-leaks.py --commits A..B  # also scan commit messages in the range (use the push/PR range)
Exit 1 with file:line list on a hit. Add a legit, reviewed exception to ci/public-leaks.allow as 'path:sha256-of-token'.
"""
import argparse, hashlib, os, re, subprocess, sys

HASHES = set("""
1a6e515b6b300805e85ddecc16ae844c2482a960e111e445be6690537c873b67
61a4c4d71bfa0cd0ef5aa7e5893a2148d80c05850ab238dbc70d25e61ee5d6d4
6d1492132b3ce158a5e9bcbd2c68f59f73e2fad07a8902881c59335e862285e9
8a2bb5c683a6e540ff545b6eb1b556eb67d9448d8d7c761dd3c6baeb6565fd31
8c1c13fc6b8ec7e70f6e18a91d6fad205df150056f02afbb256937199fdc386e
91285b08ab8845d0c6a5a25e3578adb013935843374aa4c3c4f57d0efcb7855e
a3aca4ad69fcef6ffd72fb068e99080e165cd3942e3c697f78d7e2cf0117d135
c148a33eb3ec2f72f91f82f0ff61b80374f60c31dd76d6bf849c41f28f5b27b9
f38d2441193e413285371ffac98a54d9da43392881e0f440025888a4405c3c84
""".split())
TOKEN = re.compile(r"[A-Za-z0-9]+")
PATTERNS = [
    (re.compile(r"\b10\.66\.69\.\d{1,3}\b"), "owner home LAN address"),
    (re.compile(r"/home/[a-z][a-z0-9_-]*/\.hermes"), "personal home path"),
    (re.compile(r"\b(?:owner|ron|he)\b[^.\n]{0,24}\b(?:is|are|was|went|gets?|comes?|coming)\s+(?:back|away|home|idle|asleep|active|out)\b", re.I), "owner presence language"),
    (re.compile(r"while (?:he|the owner) (?:is|was) (?:away|home|out|asleep|idle)", re.I), "owner presence language"),
    (re.compile(r"\bHogsm\w*\s+Station\b", re.I), "home network name"),
]
HERE = os.path.dirname(os.path.abspath(__file__))
SELF = {"ci/check-public-leaks.py", "ci/public-leaks.allow"}


def h(w):
    return hashlib.sha256(w.lower().encode()).hexdigest()


def allowed():
    p = os.path.join(HERE, "public-leaks.allow")
    return {l.split("#")[0].strip() for l in open(p)} - {""} if os.path.exists(p) else set()


def scan_text(name, text, ok, out):
    for n, line in enumerate(text.split("\n"), 1):
        # public org/repo URLs (github.com/<org>/..., forgejo clone urls) are fine: strip them before matching the username token
        l = re.sub(r"(?i)(?:github\.com|git\.[\w.-]+)[/:][A-Za-z0-9_-]+(?:/[A-Za-z0-9_.-]+)?", " ", line)
        for t in TOKEN.findall(l):
            if h(t) in HASHES and f"{name}:{h(t)}" not in ok:
                out.append(f"{name}:{n}: owner-identifying token (hash {h(t)[:12]})")
        for rx, why in PATTERNS:
            if rx.search(line):
                out.append(f"{name}:{n}: {why}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--commits")
    a = ap.parse_args()
    top = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
    os.chdir(top)
    ok, out = allowed(), []
    for f in subprocess.check_output(["git", "ls-files", "-z"]).decode().split("\0"):
        if not f or f in SELF or not os.path.isfile(f):
            continue
        scan_text(f, f, ok, out)  # file names too
        try:
            data = open(f, "rb").read()
        except OSError:
            continue
        if b"\0" in data[:4096]:
            continue
        scan_text(f, data.decode("utf-8", "replace"), ok, out)
    if a.commits:
        for c in subprocess.check_output(["git", "rev-list", a.commits], text=True).split():
            msg = subprocess.check_output(["git", "log", "-1", "--format=%an <%ae>%n%B", c], text=True)
            scan_text(f"commit {c[:10]}", msg, ok, out)
    for o in sorted(set(out)):
        print("LEAK", o)
    print(f"check-public-leaks: {'FAIL' if out else 'PASS'} ({len(set(out))} hit(s))")
    return 1 if out else 0


if __name__ == "__main__":
    sys.exit(main())
