#!/usr/bin/env python3
"""GitHub is a MIRROR of this Forgejo repo: no GitHub workflow may run by itself there (owner 2026-11-26).

A leftover .github/workflows/pages.yml ran on every mirror push, failed, and GitHub mailed the owner about it. CI is
Forgejo (.forgejo/workflows). This gate fails when a file under .github/workflows/ has any trigger other than
`workflow_dispatch` (manual, deliberate) unless ci/ux-qa/github-workflows-allow.txt lists it with a reason.
(The mirror itself has GitHub Actions disabled by ctg-ghsync and the ci-red-tracker gate re-checks that every 20 min;
this check keeps the source repo from carrying an auto-running workflow in the first place.)

  github-workflows-allow.txt:   <file name>  # reason [(until YYYY-MM-DD) for a temporary allowance]
usage: check_github_workflows.py [REPO_ROOT]
"""
import datetime
import os
import re
import sys

AUTO = ("push", "pull_request", "pull_request_target", "schedule", "release", "workflow_run", "create", "delete",
        "deployment", "issues", "issue_comment", "page_build", "registry_package", "repository_dispatch", "status", "watch",
        "fork", "gollum", "check_run", "check_suite", "discussion", "label", "merge_group", "milestone", "project", "public")


def triggers(text):
    """Event names under the top-level `on:` key (block map, list or scalar form). No yaml dependency."""
    m = re.search(r"(?m)^(?:on|\"on\"|'on'|true):[ \t]*(.*)$", text)
    if not m:
        return set()
    inline = m.group(1).split("#")[0].strip()
    if inline:
        return set(re.findall(r"[A-Za-z_]+", inline))
    ev = set()
    for ln in text[m.end():].splitlines():
        if not ln.strip() or ln.lstrip().startswith("#"):
            continue
        if not ln.startswith((" ", "\t")):
            break
        mm = re.match(r"^( {1,4}|\t)(?:- )?([A-Za-z_]+)\s*:?", ln)
        if mm and len(mm.group(1)) == len(re.match(r"^[ \t]*", ln).group(0)):
            ev.add(mm.group(2))
    return ev


def check(root):
    d = os.path.join(root, ".github", "workflows")
    allow = {}
    af = os.path.join(root, "ci", "ux-qa", "github-workflows-allow.txt")
    if os.path.exists(af):
        for ln in open(af):
            name, sep, why = ln.strip().partition("#")
            if name.strip() and not ln.lstrip().startswith("#"):
                allow[name.strip()] = why.strip()
    probs = []
    today = datetime.date.today().isoformat()
    for n, why in list(allow.items()):
        if not why:
            probs.append(f"github-workflows-allow.txt: {n} has no reason")
        until = re.search(r"until (\d{4}-\d{2}-\d{2})", why)
        if until and until.group(1) < today:
            allow[n] = ""   # an expired temporary allowance no longer allows
        if not os.path.exists(os.path.join(d, n)):
            print(f"note: github-workflows-allow.txt lists {n}, which no longer exists: delete the line")
    if not os.path.isdir(d):
        return probs
    for f in sorted(os.listdir(d)):
        if not f.endswith((".yml", ".yaml")):
            continue
        ev = triggers(open(os.path.join(d, f), encoding="utf-8").read())
        auto = sorted(e for e in ev if e in AUTO)
        if auto and not allow.get(f):
            probs.append(f".github/workflows/{f} runs on the GitHub mirror by itself ({', '.join(auto)}): CI is Forgejo. "
                         f"Delete it, make it workflow_dispatch-only, or list it in ci/ux-qa/github-workflows-allow.txt with a reason")
    return probs


if __name__ == "__main__":
    p = check(sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
    for x in p:
        print("FAIL", x)
    print(f"check_github_workflows: {len(p)} problem(s)")
    sys.exit(1 if p else 0)
