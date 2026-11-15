#!/usr/bin/env python3
"""CI gate: every cat/pkg with an ebuild must be watched, or explicitly exempt.

A package passes when ONE of these holds:
  * it has a section in .nvchecker/nvchecker.toml AND a security-feed mapping
    (derived from a github/pypi/cratesio/npm nvchecker source, or an entry in
    .nvchecker/security.toml with github=, osv=, or none="reason");
  * it has an entry in .nvchecker/security.toml with no_version_check="reason"
    (e.g. a commit-pinned snapshot) and a security mapping;
  * it matches a pattern in .nvchecker/excluded.txt that carries a one-line
    reason (`pattern  # reason`): first-party, account or meta packages.
Env OVERLAY_DIRS=dir[:dir] as for update-check.py.  Exit 1 lists every offender.
"""
import fnmatch, importlib.util, os, sys

here = os.path.dirname(os.path.abspath(__file__))


def load(name, file):
    s = importlib.util.spec_from_file_location(name, os.path.join(here, file))
    m = importlib.util.module_from_spec(s)
    s.loader.exec_module(m)
    return m


def check():
    uc = load("update_check", "update-check.py")
    sc = load("security_check", "security-check.py")
    nv = sc.tomllib.load(open(uc.CFG, "rb")) if os.path.exists(uc.CFG) else {}
    sec = sc.tomllib.load(open(uc.SEC, "rb")) if os.path.exists(uc.SEC) else {}
    pats = uc.excluded_patterns()
    bad = []
    for p in pats:
        if not p[1]:
            bad.append(f"{uc.EXCL}: pattern '{p[0]}' has no reason (write `pattern  # reason`)")
    stats = {"watched": 0, "exempt": 0}
    for pkg in uc.all_packages():
        reason = next((r for pat, r in pats if fnmatch.fnmatch(pkg, pat)), None)
        if reason:
            stats["exempt"] += 1
            continue
        gh, osv, none, _ = sc.sources(pkg, nv, sec)
        mapped = bool(gh or osv or none)
        if pkg in nv and mapped:
            stats["watched"] += 1
        elif pkg in sec and sec[pkg].get("no_version_check") and mapped:
            stats["watched"] += 1
        elif pkg in nv:
            bad.append(f"{pkg}: nvchecker entry but no security-feed mapping (add it to {uc.SEC}: github=, osv= or none=\"reason\")")
        elif pkg in sec:
            bad.append(f"{pkg}: in {uc.SEC} but no nvchecker entry (add one, or no_version_check=\"reason\")")
        else:
            bad.append(f"{pkg}: unwatched. Add an nvchecker section + security mapping, or list it in {uc.EXCL} with a reason")
    return bad, stats


if __name__ == "__main__":
    bad, st = check()
    print(f"coverage-check: {st['watched']} watched, {st['exempt']} exempt, {len(bad)} offending")
    for b in bad:
        print("FAIL:", b)
    sys.exit(1 if bad else 0)
