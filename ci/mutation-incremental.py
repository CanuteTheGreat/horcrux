#!/usr/bin/env python3
"""Incremental mutation testing: mutate only what changed since the last green baseline.

Canonical copy: ctg-infrastructure ci-runners/mutation-incremental/ (README there). Each repo that
runs mutation testing on the colo lane (rw-man-01, colo-x86_64) carries an identical copy; fix
bugs there first, then copy.

Why: the colo lane has 2 slots on one CPU-bound host. Full mutation passes (1-3 h per job) every
night for ~20 repos queued 10+ h. Most nights only a few functions changed, so a full pass mostly
re-proves yesterday's result.

State (one per mutation job id). SHARED STORE (2026-10-22): the baseline lives on the orphan branch
`mutation-baseline` of the repo itself (one `<id>/baseline.json` per job id, no workflow files on it so no CI
fires), read by `plan` and written after every green `gate`/skip, so every runner (rw-man-01..05) sees the same
last green sha and known survivors. Until then it was a per-runner actions/cache entry: a nightly landing on
another runner restored an old baseline (or none: "Cache not found" -> a diff of the last 26 h with no known
survivors) and went red on old survivors or mutated a huge change set. The branch is read/written with the
checkout's own credentials (plain git plumbing: fetch, commit-tree, non-force push, retried on a race), only
inside Forgejo Actions (GITHUB_ACTIONS=true), pushes only from the default branch (main/master) runs. The
actions/cache copy stays as a fallback when the branch can't be reached (MUT_BASELINE_STORE=cache disables the
branch). The local layout the rest of this script uses is unchanged:
    .mutation-baseline/<id>/baseline.json
        {"id", "sha": last green commit, "last_full": "YYYY-MM-DD" (UTC) of the last green full
         run, "survivors": [keys of mutants that survived that run]}
The cache is saved only when the job succeeds, so the baseline always points at a green run.

Commands
  plan  --id ID --full-dow N [--mode auto|full|incremental] [--relative DIR] [--kind K] -- PATH...
        Decides the mode and writes the diff. Exports (to $GITHUB_ENV and stdout):
          MUT_MODE   full | incremental | skip
          MUT_BASE   commit the diff starts at (incremental/skip)
          MUT_DIFF   unified diff BASE..HEAD of PATH... (paths relative to --relative DIR)
          MUT_CHANGED  file listing the changed PATHs (relative to the repo root)
          MUT_TARGETS  (--kind pit) comma-separated PIT --targetClasses globs of changed classes
          MUT_STATE  the state dir
        auto: full on ISO weekday --full-dow (UTC) unless a green full already ran today; also
        full the day after if the last green full is > 6 days old (a delayed/red full day);
        full when there is no usable base at all. Otherwise incremental against the baseline
        sha, or - with no baseline yet - against the newest commit older than 26 h (mutmut: a full
        run instead, its unit is a whole function). skip when none of PATH... changed: the
        baseline is advanced to HEAD right away.
  mutmut-targets --diff F --mutants DIR [--sample N]
        (inside the mutmut container, after mutant generation) prints the mutant keys of every
        function the diff touches, plus every function whose tests live in a changed test file
        (from mutants/mutmut-stats.json). Prints ALL when a conftest/fixture/setup file changed.
  gate  --kind cargo|pit|mutmut --mode M [--exclude-desc-prefix P] [--allow-empty] [--diff F] -- RESULT...
        incremental: fails on every surviving mutant that is not a known survivor of the
        baseline (= a new survivor in changed code; with --diff, PIT survivors count only on the
        changed lines, like cargo-mutants --in-diff); full: records the survivors. Then writes
        the new baseline (sha = HEAD). Score floors stay with each repo's own floor script and
        apply to full runs (a score over a diff subset means nothing).
"""
import argparse, ast, datetime, fnmatch, glob, json, os, re, subprocess, sys, tempfile, time
import xml.etree.ElementTree as ET

STATE_ROOT = ".mutation-baseline"
STORE_BRANCH = "mutation-baseline"
PUSH_REFS = ("main", "master")


def git(*args, cwd=None, check=True):
    r = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed: {r.stderr.strip()}")
    return r.stdout if r.returncode == 0 else None


# ------------------------------------------------------------------ shared baseline store (git branch)
def store_enabled():
    return os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("MUT_BASELINE_STORE", "branch") == "branch"


def store_fetch():
    """Fetch the store branch; returns its tip sha, or None (no branch yet / unreachable)."""
    r = subprocess.run(["git", "fetch", "--quiet", "--no-tags", "--depth=1", "origin",
                        f"+refs/heads/{STORE_BRANCH}:refs/remotes/origin/{STORE_BRANCH}"], capture_output=True, text=True)
    if r.returncode != 0:
        why = r.stderr.strip().splitlines()[-1] if r.stderr.strip() else "?"
        print(f"mutation baseline store: no '{STORE_BRANCH}' branch fetched ({why})")
        return None
    return git("rev-parse", f"refs/remotes/origin/{STORE_BRANCH}", check=False, ).strip() or None


def store_pull():
    """Materialise every <id>/baseline.json of the store branch under .mutation-baseline/ (remote wins over
    the runner-local cache copy). Returns the number restored."""
    if not store_enabled():
        return 0
    tip = store_fetch()
    if not tip:
        return 0
    n = 0
    for path in (git("ls-tree", "-r", "--name-only", tip, check=False) or "").splitlines():
        if not path.endswith("/baseline.json") or path.count("/") != 1:
            continue
        text = git("show", f"{tip}:{path}", check=False)
        try:
            b = json.loads(text or "")
        except ValueError:
            continue
        i = path.split("/")[0]
        os.makedirs(state_dir(i), exist_ok=True)
        with open(os.path.join(state_dir(i), "baseline.json"), "w") as f:
            json.dump(b, f, indent=1, sort_keys=True)
        n += 1
    print(f"mutation baseline store: restored {n} baseline(s) from branch {STORE_BRANCH} @ {tip[:10]}")
    return n


def store_push(i):
    """Publish .mutation-baseline/<i>/baseline.json to the store branch (non-force, retried on a race)."""
    if not store_enabled():
        return
    ref = os.environ.get("GITHUB_REF_NAME") or ""
    if ref not in PUSH_REFS and os.environ.get("MUT_BASELINE_PUSH_ANY") != "1":
        print(f"mutation baseline store: not pushing from '{ref}' (only {'/'.join(PUSH_REFS)})")
        return
    data = open(os.path.join(state_dir(i), "baseline.json")).read()
    path = f"{i}/baseline.json"
    ident = {"GIT_AUTHOR_NAME": "mutation-baseline", "GIT_AUTHOR_EMAIL": "ci@errant.solutions",
             "GIT_COMMITTER_NAME": "mutation-baseline", "GIT_COMMITTER_EMAIL": "ci@errant.solutions"}
    for attempt in range(1, 7):
        tip = store_fetch()
        idx = os.path.join(os.path.abspath(".git") if os.path.isdir(".git") else tempfile.gettempdir(),
                           f"mut-baseline-index-{os.getpid()}")
        env = dict(os.environ, GIT_INDEX_FILE=idx, **ident)
        def g(*a, inp=None):
            return subprocess.run(["git", *a], capture_output=True, text=True, env=env, input=inp)
        try:
            if tip:
                g("read-tree", tip)
            else:
                g("read-tree", "--empty")
            blob = g("hash-object", "-w", "--stdin", inp=data).stdout.strip()
            g("update-index", "--add", "--cacheinfo", f"100644,{blob},{path}")
            tree = g("write-tree").stdout.strip()
            if tip and git("rev-parse", f"{tip}^{{tree}}", check=False).strip() == tree:
                print(f"mutation baseline store: {path} already current")
                return
            msg = f"{i}: baseline {json.loads(data).get('sha', '')[:10]} ({os.environ.get('GITHUB_RUN_ID', 'local')} on {os.uname().nodename})"
            c = g("commit-tree", tree, *( ["-p", tip] if tip else []), "-m", msg)
            if c.returncode != 0:
                print(f"::warning::mutation baseline store: commit-tree failed: {c.stderr.strip()}")
                return
            r = subprocess.run(["git", "push", "--quiet", "origin", f"{c.stdout.strip()}:refs/heads/{STORE_BRANCH}"],
                               capture_output=True, text=True)
        finally:
            try:
                os.remove(idx)
            except OSError:
                pass
        if r.returncode == 0:
            print(f"mutation baseline store: pushed {path} -> {STORE_BRANCH} ({attempt} attempt(s))")
            return
        print(f"mutation baseline store: push attempt {attempt} failed: {r.stderr.strip().splitlines()[-1] if r.stderr.strip() else '?'}")
        time.sleep(min(2 * attempt, 10))
    print(f"::warning::mutation baseline store: could not publish {path}; only this runner's cache has it")


def today():
    return datetime.datetime.now(datetime.timezone.utc).date()


def state_dir(i):
    return os.path.join(STATE_ROOT, i)


def load(i):
    try:
        with open(os.path.join(state_dir(i), "baseline.json")) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def save(i, b):
    os.makedirs(state_dir(i), exist_ok=True)
    with open(os.path.join(state_dir(i), "baseline.json"), "w") as f:
        json.dump(b, f, indent=1, sort_keys=True)


def export(**kv):
    env = os.environ.get("GITHUB_ENV")
    out = os.environ.get("GITHUB_OUTPUT")
    for k, v in kv.items():
        print(f"{k}={v}")
        for p in (env, out):
            if p:
                with open(p, "a") as f:
                    f.write(f"{k}={v}\n")


def usable(sha):
    if not sha:
        return False
    if git("cat-file", "-e", f"{sha}^{{commit}}", check=False) is None:
        return False
    return git("merge-base", "--is-ancestor", sha, "HEAD", check=False) is not None


def pit_targets(changed):
    """PIT --targetClasses globs for changed Kotlin/Java sources under src/main."""
    pats = set()
    for p in changed:
        m = re.search(r"/src/main/(?:kotlin|java)/(.+)\.(kt|java)$", p)
        if not m or not os.path.exists(p):
            continue
        text = open(p, encoding="utf-8", errors="replace").read()
        pkg = re.search(r"^\s*package\s+([\w.]+)", text, re.M)
        pkg = pkg.group(1) + "." if pkg else ""
        stem = os.path.basename(m.group(1))
        names = {stem}
        if m.group(2) == "kt":
            names.add(stem + "Kt")  # top-level functions
        names |= set(re.findall(r"^\s*(?:[a-z]+\s+)*(?:class|object|interface)\s+([A-Z]\w*)", text, re.M))
        for n in names:
            pats.add(f"{pkg}{n}")
            pats.add(f"{pkg}{n}$*")
    return sorted(pats)


def cmd_plan(a):
    head = git("rev-parse", "HEAD").strip()
    store_pull()
    b = load(a.id)
    now = today()
    base, why = None, ""
    if b and usable(b.get("sha")):
        base, why = b["sha"], "last green baseline"
    elif b:
        why = f"baseline {str(b.get('sha'))[:10]} is not in this history"
    if not base:
        cutoff = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=26)).strftime("%Y-%m-%dT%H:%M:%SZ")
        old = (git("rev-list", "-1", f"--before={cutoff}", "HEAD", check=False) or "").strip()
        if old:
            base, why = old, (why + "; " if why else "") + "no baseline yet: changes of the last 26 h"
    last_full = b.get("last_full") if b else None
    days_since_full = (now - datetime.date.fromisoformat(last_full)).days if last_full else None
    dow = now.isoweekday()
    mode = a.mode
    if mode == "auto":
        if dow == a.full_dow and last_full != now.isoformat():
            mode, why = "full", f"weekly full day (ISO weekday {a.full_dow})"
        elif dow == a.full_dow % 7 + 1 and (days_since_full is None or days_since_full > 6) and b is not None:
            mode, why = "full", f"last green full {last_full or 'never'}: catching up the day after the full day"
        elif not base:
            mode, why = "full", "no usable base commit"
        else:
            mode = "incremental"
    elif mode == "incremental" and not base:
        mode, why = "full", "incremental asked, but there is no usable base commit"
    os.makedirs(state_dir(a.id), exist_ok=True)
    sd = os.path.abspath(state_dir(a.id))
    diff_path = os.path.join(sd, "diff.patch")
    changed_path = os.path.join(sd, "changed.txt")
    rel = a.relative or "."
    if mode == "full":
        open(diff_path, "w").close()
        open(changed_path, "w").close()
        export(MUT_MODE="full", MUT_BASE="", MUT_DIFF=diff_path, MUT_CHANGED=changed_path, MUT_TARGETS="", MUT_STATE=sd)
        print(f"mutation {a.id}: FULL run ({why or a.mode})")
        return
    paths = a.paths or ["."]
    diff = git("-C", rel, "diff", "--no-ext-diff", "--no-color", "--relative", base, head, "--", *paths)
    changed = git("-C", rel, "diff", "--name-only", "--relative", base, head, "--", *paths).split()
    root_rel = os.path.relpath(os.path.abspath(rel), os.path.abspath(git("rev-parse", "--show-toplevel").strip()))
    changed_root = [os.path.normpath(os.path.join(root_rel, c)) for c in changed]
    with open(diff_path, "w") as f:
        f.write(diff)
    with open(changed_path, "w") as f:
        f.write("\n".join(changed_root) + ("\n" if changed_root else ""))
    targets = ""
    if a.kind == "pit":
        targets = ",".join(pit_targets(changed_root))
    if a.kind == "mutmut" and diff.strip() and not b:
        # mutmut's unit is a whole function and there is no list of known survivors yet: a full
        # run (minutes for most tools) is the baseline every later night diffs against
        open(diff_path, "w").close()
        export(MUT_MODE="full", MUT_BASE="", MUT_DIFF=diff_path, MUT_CHANGED=changed_path, MUT_TARGETS="", MUT_STATE=sd)
        print(f"mutation {a.id}: FULL run (code changed and no baseline yet: this run becomes it)")
        return
    if not diff.strip() or (a.kind == "pit" and not targets):
        nb = dict(b or {}, id=a.id, sha=head)
        nb.setdefault("survivors", [])
        nb.setdefault("last_full", None)
        save(a.id, nb)
        store_push(a.id)
        export(MUT_MODE="skip", MUT_BASE=base, MUT_DIFF=diff_path, MUT_CHANGED=changed_path, MUT_TARGETS="", MUT_STATE=sd)
        print(f"mutation {a.id}: SKIP - nothing it mutates changed since {base[:10]} ({why}); baseline -> {head[:10]}")
        return
    export(MUT_MODE="incremental", MUT_BASE=base, MUT_DIFF=diff_path, MUT_CHANGED=changed_path, MUT_TARGETS=targets, MUT_STATE=sd)
    print(f"mutation {a.id}: INCREMENTAL since {base[:10]} ({why}); {len(changed_root)} changed file(s): {' '.join(changed_root)}")
    if targets:
        print(f"  PIT target classes: {targets}")


# ------------------------------------------------------------------ mutmut targets
HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def changed_lines(diff_text):
    """{path: set(new-side line numbers touched)} (a pure deletion marks the line after it)."""
    out, cur, ln = {}, None, 0
    for line in diff_text.splitlines():
        if line.startswith("+++ "):
            p = line[4:].strip()
            cur = None if p == "/dev/null" else re.sub(r"^b/", "", p)
            continue
        if line.startswith("--- ") or cur is None:
            continue
        m = HUNK.match(line)
        if m:
            ln = int(m.group(1))
            if m.group(2) == "0":  # deletion only
                out.setdefault(cur, set()).add(max(ln, 1))
            continue
        if line.startswith("+"):
            out.setdefault(cur, set()).add(ln)
            ln += 1
        elif line.startswith("-"):
            out.setdefault(cur, set()).add(ln)
        else:
            ln += 1
    return out


def mangled_functions(path, lines):
    """mutmut mangled names (x_func / xǁClassǁmethod) of top-level functions/methods touching lines."""
    try:
        tree = ast.parse(open(path, encoding="utf-8").read())
    except (OSError, SyntaxError):
        return None
    names = set()

    def hit(node):
        start = min([node.lineno] + [d.lineno for d in getattr(node, "decorator_list", [])])
        return any(start <= l <= node.end_lineno for l in lines)

    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and hit(node):
            names.add(f"x_{node.name}")
        elif isinstance(node, ast.ClassDef):
            for sub in node.body:
                if isinstance(sub, (ast.FunctionDef, ast.AsyncFunctionDef)) and hit(sub):
                    names.add(f"x\u01c1{node.name}\u01c1{sub.name}")
    return names


def all_mutmut_keys(mutants):
    keys = {}
    for p in glob.glob(os.path.join(mutants, "**", "*.meta"), recursive=True):
        try:
            d = json.load(open(p))
        except ValueError:
            continue
        for k, v in d.get("exit_code_by_key", {}).items():
            keys[k] = v
    return keys


def cmd_mutmut_targets(a):
    diff = open(a.diff).read()
    keys = all_mutmut_keys(a.mutants)
    touched = changed_lines(diff)
    wanted = set()
    everything = False
    tests_changed = set()
    for path, lines in touched.items():
        base = os.path.basename(path)
        if path.startswith("tests/") or "/tests/" in path:
            if not (base.startswith("test_") and base.endswith(".py")):
                everything = True  # conftest / helper module / fixture data: any test may behave differently
            else:
                tests_changed.add(path)
            continue
        if not path.endswith(".py"):
            if base in ("setup.sh", "pyproject.toml", "setup.cfg") or base.startswith("requirements"):
                everything = True
            continue
        names = mangled_functions(path, lines)
        if names is None:
            everything = True
            continue
        mod = path[:-3].replace("/", ".")
        mod = mod[4:] if mod.startswith("src.") else mod
        for n in names:
            wanted.add(f"{mod}.{n}")
    if tests_changed:
        try:
            stats = json.load(open(os.path.join(a.mutants, "mutmut-stats.json")))
        except (OSError, ValueError):
            stats = {}
            everything = True
        for fn, tests in stats.get("tests_by_mangled_function_name", {}).items():
            if any(t.split("::")[0] in tests_changed for t in tests):
                wanted.add(fn)
    if everything:
        pick = sorted(keys)
    else:
        pick = sorted(k for k in keys if k.partition("__mutmut_")[0] in wanted)
    if a.sample > 1:
        import zlib
        pick = [k for k in pick if zlib.crc32(k.encode()) % a.sample == 0]
    print(" ".join(pick))
    print(f"mutmut-targets: {len(pick)} of {len(keys)} mutants ({'everything: a shared test input changed' if everything else str(len(wanted)) + ' function(s) touched'})", file=sys.stderr)


# ------------------------------------------------------------------ gate
def strip_line(name):
    return re.sub(r"^(.*?):\d+(?::\d+)?:\s*", r"\1: ", name)


TIMEOUT_SHARE_LIMIT = 0.5


def survivors_cargo(paths):
    """Missed mutants; also fails when most mutants timed out: cargo-mutants counts a timeout as
    caught, so a test timeout that is too tight for the loaded colo host turns every mutant into a
    false catch (horcrux storage 2026-10-18: 35 of 35 TIMEOUT with 2 libtest threads; rustinion
    run 4302: every web-auth mutant)."""
    cur, seen = set(), False
    timeouts = viable = 0
    for p in paths:
        f = p if p.endswith(".json") else os.path.join(p, "outcomes.json")
        if not os.path.exists(f) and os.path.exists(os.path.join(p, "mutants.out", "outcomes.json")):
            f = os.path.join(p, "mutants.out", "outcomes.json")
        if not os.path.exists(f):
            continue
        seen = True
        for o in json.load(open(f)).get("outcomes", []):
            sc = o.get("scenario")
            if isinstance(sc, dict) and "Mutant" in sc and o.get("summary") in ("CaughtMutant", "MissedMutant", "Timeout"):
                viable += 1
                timeouts += o.get("summary") == "Timeout"
            if o.get("summary") == "MissedMutant" and isinstance(sc, dict) and "Mutant" in sc:
                m = sc["Mutant"]
                cur.add(strip_line(m.get("name") or f"{m.get('file')}: {m.get('replacement')}"))
    if timeouts >= 5 and timeouts > TIMEOUT_SHARE_LIMIT * viable:
        raise SystemExit(f"::error::mutation gate: {timeouts} of {viable} mutants TIMED OUT - the test timeout is too "
                         "tight for this host (timeouts count as caught, so the score means nothing); raise "
                         "--minimum-test-timeout / --timeout for this job")
    return cur, seen


def survivors_pit(paths, exclude, lines_by_path=None):
    """lines_by_path (incremental): only survivors on lines the diff touched count."""
    cur, seen = set(), False
    for p in paths:
        for f in ([p] if p.endswith(".xml") else glob.glob(os.path.join(p, "**", "mutations.xml"), recursive=True)):
            if not os.path.exists(f):
                continue
            seen = True
            for m in ET.parse(f).getroot().iter("mutation"):
                if m.get("status") not in ("SURVIVED", "NO_COVERAGE"):
                    continue
                d = m.findtext("description") or ""
                if any(d.startswith(x) for x in exclude):
                    continue
                if lines_by_path is not None:
                    cls = m.findtext("mutatedClass") or ""
                    suffix = "/".join(cls.split(".")[:-1] + [m.findtext("sourceFile") or ""])
                    ln = int(m.findtext("lineNumber") or 0)
                    if not any(p.endswith(suffix) and ln in ls for p, ls in lines_by_path.items()):
                        continue
                cur.add(f"{m.findtext('mutatedClass')}.{m.findtext('mutatedMethod')}: {m.findtext('mutator')}: {d}")
    return cur, seen


def survivors_mutmut(paths):
    cur, seen = set(), False
    for p in paths:
        keys = all_mutmut_keys(p)
        if keys:
            seen = True
        cur |= {k for k, v in keys.items() if v in (0, 5, 33)}  # survived, no tests
    return cur, seen


def cmd_gate(a):
    b = load(a.id) or {"id": a.id, "survivors": [], "last_full": None}
    if a.kind == "cargo":
        cur, seen = survivors_cargo(a.results)
    elif a.kind == "pit":
        lines = None
        if a.mode == "incremental" and a.diff:
            lines = changed_lines(open(a.diff).read())
        cur, seen = survivors_pit(a.results, a.exclude_desc_prefix, lines)
    else:
        cur, seen = survivors_mutmut(a.results)
    if not seen and not a.allow_empty:
        raise SystemExit(f"mutation gate {a.id}: no results found in {a.results}")
    head = git("rev-parse", "HEAD").strip()
    if a.mode == "full":
        nb = dict(b, id=a.id, sha=head, survivors=sorted(cur), last_full=today().isoformat())
        save(a.id, nb)
        store_push(a.id)
        print(f"mutation gate {a.id}: full run recorded ({len(cur)} survivor(s)); baseline -> {head[:10]}")
        return
    known = set(b.get("survivors", []))
    new = sorted(cur - known)
    if new:
        print(f"::error::mutation gate {a.id}: {len(new)} NEW surviving mutant(s) in code changed since {str(b.get('sha'))[:10]} - add tests that catch them:")
        for n in new:
            print(f"  NEW SURVIVOR {n}")
        sys.exit(1)
    nb = dict(b, id=a.id, sha=head)
    nb["survivors"] = sorted(known | cur)
    save(a.id, nb)
    store_push(a.id)
    print(f"mutation gate {a.id}: incremental run clean ({len(cur)} survivor(s), all known from the last full run{'' if seen else '; no mutants in the changed code'}); baseline -> {head[:10]}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("plan")
    p.add_argument("--id", required=True)
    p.add_argument("--full-dow", type=int, required=True, help="ISO weekday (1=Mon..7=Sun, UTC) of the weekly full run")
    # "" = a scheduled run (workflow_dispatch inputs are empty then) = auto
    p.add_argument("--mode", default="auto", choices=["", "auto", "full", "incremental"])
    p.add_argument("--relative", default="")
    p.add_argument("--kind", default="cargo", choices=["cargo", "pit", "mutmut"])
    p.add_argument("paths", nargs="*")
    m = sub.add_parser("mutmut-targets")
    m.add_argument("--diff", required=True)
    m.add_argument("--mutants", default="mutants")
    m.add_argument("--sample", type=int, default=1)
    g = sub.add_parser("gate")
    g.add_argument("--id", required=True)
    g.add_argument("--kind", required=True, choices=["cargo", "pit", "mutmut"])
    g.add_argument("--mode", required=True, choices=["full", "incremental"])
    g.add_argument("--exclude-desc-prefix", action="append", default=[])
    g.add_argument("--allow-empty", action="store_true")
    g.add_argument("--diff", default="", help="(pit, incremental) count only survivors on the diff's changed lines")
    g.add_argument("results", nargs="+")
    a = ap.parse_args()
    if a.cmd == "plan":
        if a.mode == "": a.mode = "auto"
        cmd_plan(a)
    elif a.cmd == "mutmut-targets":
        cmd_mutmut_targets(a)
    else:
        cmd_gate(a)


if __name__ == "__main__":
    main()
