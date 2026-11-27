#!/usr/bin/env python3
"""check-default-editor.py: no default image may resolve its editor to nano (owner 2026-11-27: "I hate nano as the default editor").

Static gate (the QEMU gate headless_gates.editor_gate proves the booted image). Fails when
  * a default image/package list (packages*.txt, gentoo/sets/*, catalyst specs, manual-build.sh) names app-editors/nano uncommented;
  * the shared base list lacks app-editors/vim, or /etc/default-editor.conf does not say EDITOR_CHOICE=vim;
  * something sets EDITOR/VISUAL/eselect editor/alternatives to nano;
  * a doc or script example tells the reader to run `nano <file>` (use vim);
  * an embedded overlay that ships a profile editor file points it at nano.
Same file in Horcrux and Patronus (ci/check-default-editor.py). Exit 0 = ok."""
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
SKIP_DIRS = {".git", "target", "node_modules", "dist", "vendor", ".cargo"}
SKIP_FILES = {"CHANGELOG.md", "THIRD-PARTY-LICENSES.txt", "Cargo.lock", "check-default-editor.py"}
fails = []


def walk():
    for d, dirs, files in os.walk(ROOT):
        dirs[:] = [x for x in dirs if x not in SKIP_DIRS]
        for f in files:
            if f in SKIP_FILES:
                continue
            yield os.path.join(d, f)


def rel(p):
    return os.path.relpath(p, ROOT)


def is_list(p):
    r = rel(p)
    b = os.path.basename(p)
    return (re.match(r"packages[\w.-]*\.txt$", b) or r.startswith("gentoo/sets/") or b.endswith(".spec")
            or b == "manual-build.sh" or r.startswith("gentoo/world") or b == "world" or b == "world.allowlist")


NANO_ATOM = re.compile(r"^\s*(?:[\w-]+\?\s+)?(?:app-editors|app-misc)/nano\s*(?:#.*)?$")
SET_NANO = re.compile(r"(?i)(EDITOR|VISUAL|SUDO_EDITOR|GIT_EDITOR)=[\"']?[^\s\"']*nano|eselect\s+(editor|visual)\s+set\s+\S*nano|update-alternatives[^\n]*editor[^\n]*nano")
DOC_NANO = re.compile(r"^\s*(?:[$#]\s*)?(?:sudo\s+)?nano\s+[~/.\w-]")

for p in walk():
    r = rel(p)
    try:
        txt = open(p, encoding="utf-8").read()
    except (UnicodeDecodeError, OSError):
        continue
    if is_list(p):
        for i, l in enumerate(txt.splitlines(), 1):
            if NANO_ATOM.match(l) and not l.lstrip().startswith("#"):
                fails.append(f"{r}:{i}: default image/package list names nano ({l.strip()})")
    for i, l in enumerate(txt.splitlines(), 1):
        if SET_NANO.search(l) and not l.lstrip().startswith("#") and "/tests/" not in r:
            fails.append(f"{r}:{i}: makes nano the editor: {l.strip()[:100]}")
        if (r.endswith((".md", ".txt", ".html", ".sh", ".yml", ".yaml", ".toml")) and DOC_NANO.match(l)):
            fails.append(f"{r}:{i}: example runs nano (use vim): {l.strip()[:100]}")
    if re.search(r"(^|/)profile\.d/[^/]*editor[^/]*$", r) and re.search(r"nano", txt):
        fails.append(f"{r}: embedded editor profile points at nano")

common = None
for cand in ("build/live-iso/common", "gentoo/live-iso/common"):
    if os.path.isdir(os.path.join(ROOT, cand)):
        common = os.path.join(ROOT, cand)
if common:
    pk = open(os.path.join(common, "packages.txt")).read().splitlines()
    if not any(re.match(r"^\s*app-editors/vim\s*(#.*)?$", l) for l in pk):
        fails.append(f"{rel(common)}/packages.txt: app-editors/vim missing from the base list (every image carries it)")
    conf = os.path.join(common, "overlay", "etc", "default-editor.conf")
    if not os.path.isfile(conf) or not re.search(r"(?m)^EDITOR_CHOICE=vim$", open(conf).read()):
        fails.append(f"{rel(conf)}: default must be EDITOR_CHOICE=vim")
    sr = open(os.path.join(common, "setup-rootfs.sh")).read()
    if "default-editor apply" not in sr:
        fails.append(f"{rel(common)}/setup-rootfs.sh does not run default-editor apply")
else:
    fails.append("no common/ layer found")

if fails:
    print("check-default-editor: FAIL")
    for f in fails:
        print("  " + f)
    sys.exit(1)
print("check-default-editor: ok")
