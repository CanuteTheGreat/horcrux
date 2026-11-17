#!/usr/bin/env python3
"""Generate CHANGELOG.md from git history between semver release tags (vMAJOR.MINOR.PATCH[-pre]).

  ci/changelog.py                     # print the changelog for every release tag reachable from HEAD
  ci/changelog.py --write             # write CHANGELOG.md
  ci/changelog.py --check             # exit 1 if CHANGELOG.md differs from what the tags produce (CI)
  ci/changelog.py --release 0.2.0     # write CHANGELOG.md as if HEAD were tagged v0.2.0 (ci/release.sh uses it)
  ci/changelog.py --section v0.2.0    # print one version's section (release notes for the Forgejo release)

Only released versions are listed (no "Unreleased" section), so the file changes only when a release is cut
and `--check` is stable on every other commit. Commits are grouped by their subject prefix ("feat:", "fix(ui):",
"gentoo:" ...). Left out: CI/test/formatting-only commits, automated commits (package bumps, overlay syncs,
[skip ci]), merges, the release commits themselves, and any subject the public-repo leak guard
(ci/check-public-leaks.py) would flag.
"""
import argparse, importlib.util, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
SEMVER = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$")
SECTIONS = [  # (title, prefixes); first match wins, prefixes compared lower-case without "(scope)"
    ("Security", ("security", "sec", "cve", "vuln")),
    ("Features", ("feat", "feature", "add")),
    ("Fixes", ("fix", "bugfix", "hotfix", "revert")),
    ("Performance", ("perf",)),
    ("Web UI", ("ui", "webui", "web ui", "horcrux-ui")),
    ("Installer and ISO images", ("iso", "live-iso", "installer", "live-installer", "gate", "qemu-test", "common")),
    ("Packaging (Gentoo)", ("gentoo", "ebuild", "overlay", "packaging", "app-emulation/horcrux")),
    ("Clustering and HA", ("cluster", "ha", "migration", "drain")),
    ("Documentation", ("docs", "doc", "site")),
]
SKIP_PREFIXES = ("ci", "test", "tests", "style", "fmt", "chore", "wip", "refactor", "build", "release", "merge",
                 "bump", "lint", "qa", "e2e", "ui e2e")
SKIP_AUTHORS = ("Horcrux CI", "ctg release bot", "Hermes Cron", "forgejo-actions")
SKIP_SUBJECT = re.compile(r"\[skip ci\]|\[ci skip\]|^Merge |^Revert \"Merge|^release: v|^fixup!|^squash!|\bowner\b", re.I)
# Lab/infrastructure machine names that appear in old commit subjects (hashed like the leak guard's own list, so
# this public file does not name them). The leak guard can't list them yet because older tracked files still do;
# the changelog is new text, so it drops such subjects instead of republishing them.
EXTRA_HASHES = {
    "a3bd6b92c67aa77558c5a8bbaeb6373227301946ad0153b3b324af28fec04a81",
    "83842a2b9527bc04e9bac631562d6a06ef3f2f586a7bcf5febe8e9e7e5bcadb8",
    "b464e18c876bbe6b3ce4cc9bb6ef02a734edb1479022b7eaa9549ccbc6ad3672",
    "bc737106fd42e20cac05f0ca86c804de889086bf066e6fe06dcde740db389fa3",
    "006a7ee9adc7832ce3a426b84d022a4d1c038c47e8e7c7638d16da8451512b22",
    "48c20b8b5385a93317bc8c2c3c64305be8ab2da0d65072adb0f05233a7e42608",
    "2db2ca6ff9406a1071142a8a22682f70f10d3eb3b59bcb27795aac4e2a2fe632",
    "c557b1179eb5bebca12cb767b29f96a7292770e13b43422d10f6f4c82726695b",
    "e610ea143109259c2aa2c93c107a733e7d6d9c721c8248b71c26a33538bba815",
}
PREFIX = re.compile(r"^([A-Za-z0-9_./ -]{1,30}?)(?:\(([^)]*)\))?!?:\s+(.*)$")


def git(*a):
    return subprocess.check_output(["git", *a], text=True)


def leak_scanner():
    spec = importlib.util.spec_from_file_location("leaks", os.path.join(HERE, "check-public-leaks.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    ok = mod.allowed()

    def leaks(text):
        out = []
        mod.scan_text("commit", text, ok, out)
        return bool(out) or any(mod.h(t) in EXTRA_HASHES for t in mod.TOKEN.findall(text))
    return leaks


def vkey(tag):
    m = SEMVER.match(tag)
    pre = m.group(4)
    # a pre-release sorts before its release
    return (int(m.group(1)), int(m.group(2)), int(m.group(3)), 0 if pre else 1, pre or "")


def release_tags():
    tags = [t for t in git("tag", "--merged", "HEAD").split() if SEMVER.match(t)]
    return sorted(tags, key=vkey)


def classify(subject):
    m = PREFIX.match(subject)
    if not m:
        return "Other changes", subject
    pre, text = m.group(1).strip().lower(), m.group(3).strip()
    if pre in SKIP_PREFIXES or pre.startswith(("ci ", "test ")):
        return None, None
    scope = m.group(2)
    for title, prefixes in SECTIONS:
        if pre in prefixes:
            if scope and title in ("Features", "Fixes", "Security", "Performance"):
                text = f"{scope}: {text}"
            return title, text
    return "Other changes", f"{m.group(1).strip()}: {text}"


def entries(rng, leaks):
    out = {}
    log = git("log", "--no-merges", "--format=%H%x1f%an%x1f%s%x1e", rng)
    for rec in log.split("\x1e"):
        rec = rec.strip()
        if not rec:
            continue
        sha, author, subject = rec.split("\x1f")
        if author in SKIP_AUTHORS or SKIP_SUBJECT.search(subject) or leaks(subject):
            continue
        title, text = classify(subject)
        if not title:
            continue
        text = text[0].upper() + text[1:] if text else text
        out.setdefault(title, []).append(f"- {text} ({sha[:8]})")
    return out


def tag_date(ref):
    return git("log", "-1", "--format=%cs", ref).strip()


def render(versions, leaks):
    """versions: list of (tag, git-ref) oldest first."""
    order = [t for t, _ in SECTIONS] + ["Other changes"]
    parts = []
    prev = None
    for tag, ref in versions:
        rng = f"{prev}..{ref}" if prev else ref
        body = entries(rng, leaks)
        lines = [f"## [{tag[1:]}] - {tag_date(ref)}", ""]
        if not prev:
            lines += ["First tagged release.", ""]
        for title in order:
            if body.get(title):
                lines += [f"### {title}", ""] + body[title] + [""]
        if prev and not body:
            lines += ["No user-facing changes.", ""]
        parts.append("\n".join(lines))
        prev = ref
    head = ("# Changelog\n\nAll notable changes to Horcrux. Versions follow [Semantic Versioning](https://semver.org/);\n"
            "see [docs/UPDATE-POLICY.md](docs/UPDATE-POLICY.md) for what each kind of release means and how to update\n"
            "or roll back. Generated from the commit history by `ci/changelog.py`; do not edit by hand.\n")
    return head + "\n" + "\n".join(reversed(parts))


def main():
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--write", action="store_true")
    g.add_argument("--check", action="store_true")
    g.add_argument("--release", metavar="X.Y.Z")
    g.add_argument("--section", metavar="vX.Y.Z")
    a = ap.parse_args()
    os.chdir(git("rev-parse", "--show-toplevel").strip())
    leaks = leak_scanner()
    versions = [(t, t) for t in release_tags()]
    if a.release:
        tag = "v" + a.release.lstrip("v")
        if not SEMVER.match(tag):
            sys.exit(f"not a semver version: {a.release}")
        if versions and vkey(tag) <= vkey(versions[-1][0]):
            sys.exit(f"{tag} is not newer than the last release {versions[-1][0]}")
        versions.append((tag, "HEAD"))
    text = render(versions, leaks)
    path = "CHANGELOG.md"
    if a.section:
        m = re.search(rf"^## \[{re.escape(a.section.lstrip('v'))}\].*?(?=^## \[|\Z)", text, re.M | re.S)
        if not m:
            sys.exit(f"no changelog section for {a.section}")
        print(m.group(0).strip())
    elif a.check:
        cur = open(path).read() if os.path.exists(path) else ""
        if cur != text:
            print(f"{path} is out of date with the release tags: run ci/changelog.py --write "
                  "(releases are cut with ci/release.sh, which does this)")
            sys.exit(1)
        print(f"{path} matches the {len(versions)} release tag(s)")
    elif a.write or a.release:
        open(path, "w").write(text)
        print(f"wrote {path} ({len(versions)} release(s))")
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
