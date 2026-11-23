#!/usr/bin/env python3
"""Fail when a packaging or image file makes /etc/portage config unreadable for ordinary users.

Administrators run `emerge -p`, `equery` and `eix` as themselves (portage group) and Portage's unprivileged build user reads repos.conf
and package.*: every file under /etc/portage (except the gnupg/ keyring) must be world-readable and every directory traversable, whatever
umask the writer runs with. This static check catches the ways a packager can break that:

  * `insopts -m0600` (or any mode without other-read) still in force at an `insinto /etc/portage/...` in the same function
  * `fperms`/`chmod`/`install -m` with a mode lacking other-read on a path under /etc/portage (gnupg excluded)
  * a group/other-masking `umask` (077, 027, ...) within three lines of a write to /etc/portage

Usage: check-portage-perms.py [dir ...]   (default: the overlay this script lives in; also run against the private overlay)
A line can opt out with `# portage-perms: allow <reason>`. The installed system and images are checked for real by
build/live-iso/common/normalize-portage-perms.sh (image build) and headless_gates.portage_group_gate (QEMU).
"""
import os
import re
import sys

FILE_RE = re.compile(r"(\.ebuild|\.eclass|\.sh|\.py|\.initd|)$")
SKIP_DIRS = {".git", "target", "node_modules", "__pycache__", "tests", "orig"}
ALLOW = "portage-perms: allow"
PORTAGE = re.compile(r"/etc/portage(?!/gnupg)")
MODE = r"(?:0?[0-7]{3,4}|[ugoa]+[-+=][rwxXst,ugoa+=-]+)"


def other_read(mode):
    """True when `mode` (octal text) leaves other-read; symbolic modes are judged only when they clearly remove it"""
    if re.fullmatch(r"0?[0-7]{3,4}", mode):
        return int(mode, 8) & 0o004 != 0
    for clause in mode.split(","):
        who, op, perm = re.fullmatch(r"([ugoa]*)([-+=])([rwxXst]*)", clause).groups() if re.fullmatch(r"([ugoa]*)([-+=])([rwxXst]*)", clause) else ("", "", "")
        hits_other = who == "" or any(c in who for c in "oa")
        if hits_other and ((op == "-" and "r" in perm) or (op == "=" and "r" not in perm)):
            return False
    return True


def check_text(name, text):
    out, lines = [], text.splitlines()
    mentions = bool(PORTAGE.search(text))
    if not mentions:
        return out
    insopts = None                       # last insopts mode seen in the current function
    for n, line in enumerate(lines, 1):
        if ALLOW in line:
            continue
        code = line.split("#", 1)[0] if not line.lstrip().startswith("#") else ""
        if re.match(r"^\w[\w-]*\(\)\s*\{", line):
            insopts = None
        m = re.search(r"\binsopts\s+(?:-m\s*|--mode=)?(\d+)", code)
        if m:
            insopts = (m.group(1), n)
        if re.search(r"\binsinto\s+/etc/portage(?!/gnupg)", code) and insopts and not other_read(insopts[0]):
            out.append(f"{name}:{n}: insinto /etc/portage with insopts -m{insopts[0]} (line {insopts[1]}) in force: not world-readable")
        for rx in (r"\bfperms\s+(?:-R\s+)?(%s)\s+(\S*)" % MODE, r"\bchmod\s+(?:-R\s+)?(%s)\s+(.*)" % MODE):
            for mm in re.finditer(rx, code):
                if PORTAGE.search(mm.group(2)) and not other_read(mm.group(1)):
                    out.append(f"{name}:{n}: mode {mm.group(1)} on {mm.group(2).strip()[:80]}: /etc/portage config must stay world-readable")
        mm = re.search(r"\binstall\b[^;&|]*?(?:-m\s*|--mode=)(%s)([^;&|]*)" % MODE, code)
        if mm and PORTAGE.search(mm.group(2)) and not other_read(mm.group(1)):
            out.append(f"{name}:{n}: install -m{mm.group(1)} into /etc/portage: must stay world-readable")
        mu = re.search(r"\bumask\s+(0?[0-7]{3})\b", code)
        if mu and int(mu.group(1), 8) & 0o044:
            near = "\n".join(lines[n - 1:n + 3])
            if PORTAGE.search(near):
                out.append(f"{name}:{n}: umask {mu.group(1)} right before a write to /etc/portage")
    return out


def scan(root):
    bad = []
    for d, dirs, files in os.walk(root):
        dirs[:] = [x for x in dirs if x not in SKIP_DIRS]
        for f in files:
            if f == "check-portage-perms.py" or not (FILE_RE.search(f) or f.startswith("Dockerfile")):
                continue
            p = os.path.join(d, f)
            try:
                text = open(p, encoding="utf-8").read()
            except (UnicodeDecodeError, OSError):
                continue
            bad += check_text(os.path.relpath(p, root), text)
    return bad


if __name__ == "__main__":
    roots = sys.argv[1:] or [os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))]
    problems = []
    for r in roots:
        problems += [f"{r}: {x}" if len(roots) > 1 else x for x in scan(r)]
    if problems:
        print("FAIL: /etc/portage config would not be readable for ordinary users:")
        print("\n".join(problems))
        sys.exit(1)
    print("ok: no packaging file restricts /etc/portage config")
