#!/usr/bin/env python3
"""Daily upstream update check for a Gentoo overlay (stdlib + nvchecker).

For every [section] named like an ebuild package (category/name) in
.nvchecker/nvchecker.toml:
  1. the newest non-live ebuild version in the overlay is the "old" version,
  2. nvchecker asks upstream for the newest release,
  3. if upstream is newer, one Forgejo issue "update-check: <cat/pkg> <ver>" is
     opened (deduplicated against open AND closed issues, so a closed issue
     is never re-opened or repeated for the same version).
SELFTEST_OLD=cat/pkg=ver fakes an old version (workflow selftest).
A broken source (nvchecker error) opens/keeps one issue "update-check: source
broken: <cat/pkg>" instead of failing the job.

Coverage: every package that has an ebuild under OVERLAY_DIRS must be either
in nvchecker.toml, in .nvchecker/security.toml (security-only, e.g. a
commit-pinned snapshot), or matched by a pattern in .nvchecker/excluded.txt
(first-party packages released by their own repo's CI, account packages,
metapackages).  Anything else opens "update-check: uncovered: <cat/pkg>".

Env: FORGEJO_API (e.g. https://git.canutethegreat.com/api/v1), FORGEJO_REPO
(owner/name), FORGEJO_TOKEN. DRY_RUN=1 prints instead of creating issues.
OVERLAY_DIRS=dir[:dir...] (default ".") holds the cat/pkg trees.
"""
import fnmatch, glob, json, os, re, subprocess, sys, tempfile, urllib.request

CFG = ".nvchecker/nvchecker.toml"
SEC = ".nvchecker/security.toml"
EXCL = ".nvchecker/excluded.txt"
DIRS = [d for d in os.environ.get("OVERLAY_DIRS", ".").split(":") if d]


def sections(path):
    names = []
    for line in open(path):
        m = re.match(r'^\["?([a-z0-9+_.-]+/[A-Za-z0-9+_.-]+)"?\]\s*$', line)
        if m:
            names.append(m.group(1))
    return names


def vkey(v):
    # good-enough Gentoo-ish ordering: numeric chunks compare as ints
    return [(0, int(x)) if x.isdigit() else (1, x) for x in re.findall(r"\d+|[A-Za-z]+", v)]


def newest_ebuild(pkg):
    cat, name = pkg.split("/")
    best = None
    for d in DIRS:
        for f in glob.glob(f"{d}/{cat}/{name}/{name}-*.ebuild"):
            v = os.path.basename(f)[len(name) + 1:-7]
            v = re.sub(r"-r\d+$", "", v)
            if v.startswith("9999") or v.endswith("9999"):
                continue
            if best is None or vkey(v) > vkey(best):
                best = v
    return best


def all_packages():
    pk = set()
    for d in DIRS:
        for f in glob.glob(f"{d}/*/*/*.ebuild"):
            parts = f.split("/")
            pk.add(f"{parts[-3]}/{parts[-2]}")
    return sorted(pk)


def excluded_patterns():
    pats = []
    if os.path.exists(EXCL):
        for line in open(EXCL):
            line = line.strip()
            if line and not line.startswith("#"):
                pat, _, why = line.partition("#")
                pats.append((pat.strip(), why.strip()))
    return pats


def security_sections():
    return sections(SEC) if os.path.exists(SEC) else []


def api(method, path, data=None):
    url = os.environ["FORGEJO_API"].rstrip("/") + path
    req = urllib.request.Request(url, method=method,
        data=None if data is None else json.dumps(data).encode(),
        headers={"Authorization": "token " + os.environ["FORGEJO_TOKEN"],
                 "Content-Type": "application/json", "User-Agent": "overlay-update-check/2"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read() or "null")


def existing_issues():
    """All issues (open + closed): list of (title, body)."""
    repo = os.environ["FORGEJO_REPO"]
    out, page = [], 1
    while True:
        items = api("GET", f"/repos/{repo}/issues?state=all&type=issues&limit=50&page={page}")
        if not items:
            return out
        out += [(i["title"], i.get("body") or "") for i in items]
        page += 1


def existing_titles():
    return {t for t, _ in existing_issues()}


def open_issue(title, body):
    api("POST", f"/repos/{os.environ['FORGEJO_REPO']}/issues", {"title": title, "body": body})


def coverage(pkgs):
    nv, sec = set(pkgs), set(security_sections())
    pats = excluded_patterns()
    rows = {"covered": [], "security-only": [], "excluded": [], "uncovered": []}
    for p in all_packages():
        if p in nv:
            rows["covered"].append(p)
        elif p in sec:
            rows["security-only"].append(p)
        elif any(fnmatch.fnmatch(p, pat) for pat, _ in pats):
            rows["excluded"].append(p)
        else:
            rows["uncovered"].append(p)
    return rows


def main():
    pkgs = sections(CFG) if os.path.exists(CFG) else []
    old = {p: newest_ebuild(p) for p in pkgs}
    if os.environ.get("SELFTEST_OLD"):  # workflow_dispatch selftest: pretend an old version to exercise issue creation
        p_, v_ = os.environ["SELFTEST_OLD"].split("=")
        old[p_] = v_
    tmp = tempfile.mkdtemp()
    oldf, newf = f"{tmp}/old.json", f"{tmp}/new.json"
    json.dump({"version": 2, "data": {p: {"version": v} for p, v in old.items() if v}}, open(oldf, "w"))
    open(newf, "w").write("")
    cfg = open(CFG).read()
    cfg = re.sub(r"(?m)^\[__config__\].*?(?=^\[|\Z)", "", cfg, flags=re.S)
    cfgf = f"{tmp}/nvchecker.toml"
    open(cfgf, "w").write(f'[__config__]\noldver = "{oldf}"\nnewver = "{newf}"\n\n' + cfg)
    keyfile = os.environ.get("NVCHECKER_KEYFILE")
    cmd = ["nvchecker", "-c", cfgf, "--logger", "json"]
    if keyfile:
        cmd += ["-k", keyfile]
    r = subprocess.run(cmd, capture_output=True, text=True)
    errors = {}
    for line in r.stdout.splitlines() + r.stderr.splitlines():
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if e.get("level") in ("error", "exception") and e.get("name"):
            errors[e["name"]] = e.get("event", "error")
    new = {}
    try:
        data = json.load(open(newf))["data"]
        new = {p: d["version"] for p, d in data.items()}
    except Exception as ex:
        print("could not read nvchecker result:", ex, r.stderr[-500:])
    json.dump(new, open(os.environ.get("NEWVER_OUT", f"{tmp}/upstream.json"), "w"))
    outdated = [(p, old[p], new[p]) for p in pkgs if old.get(p) and new.get(p) and vkey(new[p]) > vkey(old[p])]
    print(f"{'package':40} {'overlay':20} upstream")
    for p in pkgs:
        flag = " <-- UPDATE" if any(o[0] == p for o in outdated) else ""
        print(f"{p:40} {str(old.get(p)):20} {new.get(p, 'ERROR: ' + errors.get(p, '?'))}{flag}")
    stale = [p for p in pkgs if not old.get(p)]
    if stale:
        print("nvchecker sections with no ebuild here (stale, remove):", ", ".join(stale))

    cov = coverage(pkgs)
    total = sum(len(v) for v in cov.values())
    watched = len(cov["covered"]) + len(cov["security-only"])
    print(f"\nCOVERAGE {os.environ.get('FORGEJO_REPO', '')}: watched {watched}/{total - len(cov['excluded'])} "
          f"third-party packages (version {len(cov['covered'])}, security-only {len(cov['security-only'])}); "
          f"{len(cov['excluded'])} excluded (first-party/account/meta), {len(cov['uncovered'])} UNCOVERED")
    for k in ("security-only", "excluded", "uncovered"):
        if cov[k]:
            print(f"  {k}: {', '.join(cov[k])}")
    json.dump({k: v for k, v in cov.items()}, open(os.environ.get("COVERAGE_OUT", f"{tmp}/coverage.json"), "w"))

    dry = os.environ.get("DRY_RUN") == "1"
    titles = set() if dry else existing_titles()
    for p, o, n in outdated:
        title = f"update-check: {p} {n}"
        body = (f"Upstream released **{n}** (overlay has **{o}**).\n\n"
                f"Bump `{p}`: copy the newest ebuild to `{p.split('/')[1]}-{n}.ebuild`, delete the superseded ebuild(s) "
                f"(owner rule: only the newest release + live 9999 stay) and its unused `files/`, "
                f"regenerate the Manifest, `pkgcheck scan`, test-emerge in a Gentoo container on an rw-man host, commit.\n\n"
                f"Opened by `.forgejo/workflows/update-check.yml` ({CFG}).")
        if title in titles:
            print("issue exists:", title); continue
        if dry:
            print("[dry-run] would open:", title); continue
        open_issue(title, body)
        print("opened:", title)
    for p, msg in errors.items():
        title = f"update-check: source broken: {p}"
        if dry or title in titles:
            print("broken source:", p, msg); continue
        open_issue(title, f"nvchecker could not determine the upstream version of `{p}`:\n\n    {msg}\n\nFix the entry in `{CFG}` (upstream page/API changed?).")
        print("opened:", title)
    for p in cov["uncovered"]:
        title = f"update-check: uncovered: {p}"
        if dry or title in titles:
            print("uncovered:", p); continue
        open_issue(title, f"`{p}` has an ebuild here but no nvchecker source, security source or exclusion.\n\n"
                   f"Add a section to `{CFG}` (upstream releases), plus `{SEC}` if the upstream repo differs from the nvchecker source, "
                   f"or list it in `{EXCL}` with the reason (first-party / account / meta package).")
        print("opened:", title)


if __name__ == "__main__":
    main()
