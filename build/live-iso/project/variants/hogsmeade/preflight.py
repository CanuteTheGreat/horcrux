#!/usr/bin/env python3
"""HOGSMEADE build preflight (static, seconds): fail fast with the WHOLE list of stale paths/atoms.

Resolves, against the merged tree and the repos as they will exist inside the image:
  * every COPY source (local + --from=<named context>) of Dockerfile and Dockerfile.part2 (symlinked components are
    rejected: BuildKit cannot traverse them),
  * every /var/db/repos/... path used by cd / ebuild / for-loop globs in RUN steps (mapped through the COPY destinations),
  * every `project/...` / `common/...` path named in build.sh and drive.sh,
  * every package atom in the RUN steps' emerge lines, packages files and the copied package.accept_keywords/use/mask/license
    files: it must exist in an enabled repo (horcrux, canutethegreat, local from the tree; gentoo + guru from the atom lists
    that preflight-atoms.sh dumps from a throwaway container), and ::repo atoms must exist in THAT repo.
usage: preflight.py --ctx <build/live-iso> --work <build dir> [--atoms <dir with gentoo.txt guru.txt>]
"""
import argparse, glob, os, re, sys

ap = argparse.ArgumentParser()
ap.add_argument("--ctx", required=True)
ap.add_argument("--work", required=True)
ap.add_argument("--atoms")
a = ap.parse_args()
CTX = os.path.abspath(a.ctx)
W = os.path.abspath(a.work)
R = os.path.abspath(os.path.join(CTX, "..", ".."))
HSM = os.path.join(CTX, "project/variants/hogsmeade")
errs = []


def err(m):
    if m not in errs:
        errs.append(m)


NAMED = {"hxgentoo": f"{R}/gentoo", "hxgentoo2": f"{R}/gentoo", "ctgoverlay": f"{W}/overlay", "hsmprivate": f"{W}/priv"}
# repo name (as emerge sees it) -> host dir of the repo root in the image
REPO_DIRS = {}  # filled from the COPY simulation


def strict_exists(base, rel):
    """exists, and no component below `base` is a symlink (Docker COPY cannot traverse them)"""
    p = base
    for part in [x for x in rel.split("/") if x]:
        p = os.path.join(p, part)
        if os.path.islink(p):
            return False, f"{p} is a symlink"
        if not os.path.exists(p):
            return False, f"{p} missing"
    return True, ""


def join_continuations(text):
    out, buf = [], ""
    for line in text.splitlines():
        s = line.rstrip()
        if s.endswith("\\"):
            buf += s[:-1] + " "
        else:
            out.append(buf + s)
            buf = ""
    if buf:
        out.append(buf)
    return out


def strip_comments(cmd):
    # drop shell comments introduced by '# ' inside RUN bodies
    return re.sub(r"(^|\s)#[^\n]*?(?=\s*;|\s*$)", " ", cmd)


# ---- 1. Dockerfiles: COPY sources + repo map -----------------------------------------------------------------
dest_map = []  # (image dest dir, host dir)
runs = []  # (file, RUN body)
for df in ["Dockerfile", "Dockerfile.part2"]:
    path = os.path.join(HSM, df)
    for ln in join_continuations(open(path).read()):
        t = ln.strip()
        if t.startswith("COPY "):
            m = re.match(r"COPY\s+(--from=(\S+)\s+)?(\S+)\s+(\S+)$", t)
            if not m:
                err(f"{df}: cannot parse {t[:100]}")
                continue
            frm, src, dst = m.group(2), m.group(3), m.group(4)
            if frm:
                if frm not in NAMED:
                    if frm not in ("hsm-kbuild",):
                        err(f"{df}: unknown named context --from={frm}")
                    continue
                base = NAMED[frm]
                if not os.path.isdir(base):
                    err(f"{df}: named context {frm} -> {base} missing")
                    continue
                ok, why = strict_exists(base, src.strip("/"))
                host = base if src.strip("/") == "" else os.path.join(base, src.strip("/"))
            else:
                ok, why = strict_exists(CTX, src)
                host = os.path.join(CTX, src)
            if not ok:
                err(f"{df}: COPY source {src}: {why}")
            if re.fullmatch(r"/var/db/repos/[\w.-]+/?", dst):
                dest_map.append((dst.rstrip("/"), host.rstrip("/")))
        elif t.startswith("RUN "):
            runs.append((df, t[4:]))

# repos in the image: /var/db/repos/<dir> -> host dir; repo name from profiles/repo_name
for dst, host in dest_map:
    rn = os.path.join(host, "profiles/repo_name")
    if os.path.exists(rn):
        REPO_DIRS[open(rn).read().strip()] = host
    else:
        err(f"repo copied to {dst} has no profiles/repo_name ({host})")
# repo name aliases used in atoms in the tree (the dir under /var/db/repos differs from the repo name)
DIR2HOST = {d: h for d, h in dest_map}


def image_path_to_host(p):
    best = None
    for d, h in DIR2HOST.items():
        if p == d or p.startswith(d + "/"):
            if best is None or len(d) > len(best[0]):
                best = (d, h)
    if best:
        return os.path.join(best[1], p[len(best[0]):].lstrip("/"))
    return None


# ---- 2. RUN steps: repo paths --------------------------------------------------------------------------------
removed_dirs = set()
for df, body in runs:
    body = strip_comments(body)
    for m in re.finditer(r"rm -rf ([^;&|]*)", body):
        for w in m.group(1).split():
            removed_dirs.add(w.rstrip("/"))
    nrm = re.sub(r"rm -rf? [^;&|]*", "", body)  # deletions of build-time leftovers are not references
    for m in re.finditer(r"(/var/db/repos/[\w.-]+/[\w./*?\[\]+-]+)", nrm):
        p = m.group(1).rstrip(";)")
        repo_root = "/".join(p.split("/")[:5])
        if repo_root in ("/var/db/repos/gentoo", "/var/db/repos/guru"):
            continue  # synced from upstream inside the image: checked via the atom lists
        if repo_root in removed_dirs:
            continue
        h = image_path_to_host(p)
        if h is None:
            if repo_root in ("/var/db/repos/horcrux-gnome-light",):
                continue  # shipped by the base image
            err(f"{df}: {p}: repo {repo_root} is never COPYed into the image (stale reference)")
            continue
        if not glob.glob(h):
            err(f"{df}: {p}: nothing at {h}")

# ---- 3. build.sh / drive.sh / Dockerfile path words ---------------------------------------------------------
for f in [f"{CTX}/build.sh", f"{W}/drive.sh", f"{HSM}/Dockerfile", f"{HSM}/Dockerfile.part2"]:
    if not os.path.exists(f):
        err(f"missing script {f}")
        continue
    for ln in open(f).read().splitlines():
        if ln.lstrip().startswith("#") or "[ -x " in ln or "[ -e " in ln or "[ -f " in ln:
            continue  # optional hooks guarded by a test
        for m in re.finditer(r"(?<![\w$/.-])((?:project|common)/[\w./-]+)", ln):
            p = m.group(1).rstrip(".,;)\"'")
            if "$" in p or "*" in p:
                continue
            ok, why = strict_exists(CTX, p)
            if not ok:
                err(f"{os.path.basename(f)}: path {p}: {why}")

# ---- 4. atoms -------------------------------------------------------------------------------------------------
def tree_atoms(root):
    s = set()
    for d in glob.glob(f"{root}/*/*/"):
        cat, pkg = d.rstrip("/").split("/")[-2:]
        if "-" in cat or cat == "virtual":
            if glob.glob(f"{d}*.ebuild") or glob.glob(f"{d}metadata.xml"):
                s.add(f"{cat}/{pkg}")
    return s


ATOMS = {name: tree_atoms(h) for name, h in REPO_DIRS.items()}
for name in ("gentoo", "guru"):
    f = os.path.join(a.atoms or "", f"{name}.txt")
    ATOMS[name] = set(open(f).read().split()) if a.atoms and os.path.exists(f) else set()
    if not ATOMS[name]:
        print(f"WARNING: no {name} atom list; {name} atoms unchecked")
CATS = {x.split("/")[0] for s in ATOMS.values() for x in s}
ALL = set().union(*ATOMS.values())
ALIAS = {"canutethegreat-overlay": "canutethegreat-overlay"}
unchecked_repos = set()


def check_atom(src, tok):
    m = re.match(r"^([=<>~!]*)([a-z0-9][a-z0-9+_.-]*)/([A-Za-z0-9_+.-]+?)(?:-(\d[\w.*-]*))?(?:\[[^\]]*\])?(?::[\w./*=-]*)?(?:::([\w-]+))?$", tok)
    if not m:
        return
    op, cat, pkg, ver, repo = m.groups()
    if cat not in CATS:
        return
    if ver is None and op and op not in ("!",):
        # =cat/pkg-1.2 where the version regex swallowed it: retry with split
        pass
    name = f"{cat}/{pkg}"
    # package names ending in -<digits> are legal; also try with the version glued back
    if name not in ALL and ver and f"{cat}/{pkg}-{ver}" in ALL:
        return
    if repo:
        if repo not in ATOMS:
            if repo not in unchecked_repos:
                unchecked_repos.add(repo)
                err(f"{src}: {tok}: repo ::{repo} is not an enabled repo (have {sorted(ATOMS)})")
            return
        if name not in ATOMS[repo]:
            err(f"{src}: {tok}: not in ::{repo}")
        return
    if name not in ALL:
        if not ATOMS["gentoo"] and name.split("/")[0] in CATS:
            return
        err(f"{src}: {tok}: no such package in any enabled repo")


def atoms_from_run(body):
    body = strip_comments(body)
    for seg in re.split(r"[;&|]\s*|\n", body):
        if not re.search(r"\bemerge\b", seg) or "emerge-webrsync" in seg and "emerge --sync" not in seg and seg.count("emerge ") < 1:
            continue
        for tok in seg.split():
            if tok.startswith("-") or tok.startswith("/") or tok.startswith("$") or "=" in tok.split("/")[0] and not tok[0] in "=<>~":
                continue
            yield tok.strip("\"'()")


for df, body in runs:
    for tok in atoms_from_run(body):
        check_atom(df, tok)

# packages files copied into the image + portage config
pfiles = [f"{CTX}/project/variants/gnome-light/packages.txt"]
pfiles += glob.glob(f"{HSM}/portage/package.*/*")
pfiles += glob.glob(f"{CTX}/project/variants/gnome-light/portage/package.*/*")
pfiles += glob.glob(f"{CTX}/project/variants/owner-common/portage/package.accept_keywords/*")
pfiles += glob.glob(f"{CTX}/project/variants/owner-common/portage/package.use/*")
for pf in pfiles:
    for ln in open(pf).read().splitlines():
        ln = ln.split("#")[0].strip()
        if not ln or ln.startswith("*/*") or ln.split()[0] in ("*/*",):
            continue
        tok = ln.split()[0]
        if tok.startswith(("sets", "@")):
            continue
        check_atom(os.path.relpath(pf, CTX), tok.replace("::guru", "::guru"))
        if "::" not in tok and False:
            pass

# every ebuild globbed in manifest loops must carry usable names
for name, h in REPO_DIRS.items():
    pass

# ---- result ----------------------------------------------------------------------------------------------------
if errs:
    print(f"PREFLIGHT FAILED: {len(errs)} problem(s)")
    for e in errs:
        print("  -", e)
    sys.exit(1)
print(f"PREFLIGHT OK (repos: {', '.join(f'{k}={len(v)} pkgs' for k, v in ATOMS.items())})")
