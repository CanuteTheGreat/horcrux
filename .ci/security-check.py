#!/usr/bin/env python3
"""Daily security-advisory check for the packages an overlay ships (stdlib only).

For every package in .nvchecker/nvchecker.toml and .nvchecker/security.toml the
newest non-live ebuild version is checked against
  * OSV.dev   (ecosystem PyPI/crates.io/npm/Go when known, else the upstream
               GitHub repo as ecosystem GIT),
  * GitHub repository security advisories (public, best effort: rate limited
    when GITHUB_TOKEN is not set),
  * Gentoo GLSA feed (only advisories naming a package we ship),
and each advisory that affects the shipped version opens ONE Forgejo issue
"security: <cat/pkg> <ID>" (deduplicated by every alias/ID against open AND
closed issues).  The body carries machine-readable lines the overlay manager
reads: severity, exploited (CISA KEV), affected range, fixed version, overlay
and newest upstream version.

security.toml entry (optional, overrides what is derived from nvchecker.toml):
  ["cat/pkg"]
  github = "owner/repo"          # GHSA + OSV GIT lookups
  osv = [["PyPI", "name"], ...]  # OSV ecosystem lookups
  none = "reason"                # no public advisory source exists
  glsa = false                   # skip the Gentoo GLSA feed for this package
Env: FORGEJO_API, FORGEJO_REPO, FORGEJO_TOKEN, DRY_RUN=1, GITHUB_TOKEN (optional),
SELFTEST_SEC=cat/pkg=ver (pretend that overlay version; opens at most one issue),
UPSTREAM_JSON (update-check.py's nvchecker result, for "newest upstream"),
SEC_MAX_NEW (cap of new issues per run, default 20).
"""
import importlib.util, json, os, re, sys, urllib.error, urllib.request, urllib.parse
import xml.etree.ElementTree as ET

try:
    import tomllib
except ImportError:  # python < 3.11 (nvchecker venv brings tomli)
    import tomli as tomllib

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("update_check", os.path.join(here, "update-check.py"))
uc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(uc)

UA = {"User-Agent": "overlay-security-check/1"}
EXPLOITED = set()
gh_remaining = [None]
ghsa_skipped = []


def http(url, data=None, headers=None, timeout=45):
    h = dict(UA, **(headers or {}))
    if data is not None:
        h["Content-Type"] = "application/json"
        data = json.dumps(data).encode()
    req = urllib.request.Request(url, data=data, headers=h)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read(), r.headers


def jget(url, data=None, headers=None):
    try:
        body, hdr = http(url, data, headers)
        return json.loads(body), hdr
    except Exception as e:
        print(f"  ! {url[:70]}: {type(e).__name__} {e}", file=sys.stderr)
        return None, None


def norm(v):
    v = re.sub(r"^[vV]", "", str(v).strip())
    v = re.sub(r"_(p|pre|rc|alpha|beta)", r".\1", v)
    return uc.vkey(v)


def sat(ver, op, ref):
    a, b = norm(ver), norm(ref)
    return {"lt": a < b, "le": a <= b, "gt": a > b, "ge": a >= b, "eq": a == b,
            "<": a < b, "<=": a <= b, ">": a > b, ">=": a >= b, "=": a == b}[op]


def in_range(ver, rng, patched=""):
    """Is ver inside a GHSA vulnerable_version_range?  Handles '>= 1.0, < 1.2.3', '< 1.2', 'All'
    and plain version lists ('0.9.18, 0.9.17'); a patched version <= ver always means not affected."""
    for tok in re.split(r"[,\s]+", (patched or "").replace(">=", " ")):
        if tok and re.match(r"^v?\d", tok) and norm(ver) >= norm(tok):
            return False
    rng = (rng or "").strip()
    if not rng or rng.lower() == "all":
        return True
    parts = [p for p in rng.split(",") if p.strip()]
    if all(re.match(r"^\s*(<=|>=|<|>|=)\s*\S+\s*$", p) for p in parts):
        ok = True
        for part in parts:
            m = re.match(r"^\s*(<=|>=|<|>|=)\s*(\S+)\s*$", part)
            ok = ok and sat(ver, m.group(1), m.group(2))
        return ok
    return any(norm(ver) == norm(p) for p in parts)  # explicit version list


def cvss_rating(vec):
    """Rough rating from a CVSS vector: exact v3.x base score, heuristic for v4. Returns e.g. 'HIGH'."""
    if not vec or not vec.startswith("CVSS:"):
        return vec
    m = dict(x.split(":") for x in vec.split("/")[1:])
    try:
        if vec.startswith("CVSS:4"):
            hi = sum(m.get(k) == "H" for k in ("VC", "VI", "VA"))
            easy = m.get("AV") == "N" and m.get("PR") == "N" and m.get("UI") == "N" and m.get("AC") == "L"
            return "CRITICAL" if easy and hi >= 2 else "HIGH" if hi else "MEDIUM" if any(m.get(k) == "L" for k in ("VC", "VI", "VA")) else "LOW"
        av = {"N": .85, "A": .62, "L": .55, "P": .2}[m["AV"]]
        ac = {"L": .77, "H": .44}[m["AC"]]
        ch = m["S"] == "C"
        pr = {"N": .85, "L": .68 if ch else .62, "H": .5 if ch else .27}[m["PR"]]
        ui = {"N": .85, "R": .62}[m["UI"]]
        c, i, a = (({"H": .56, "L": .22, "N": 0})[m[k]] for k in ("C", "I", "A"))
        iss = 1 - (1 - c) * (1 - i) * (1 - a)
        imp = 7.52 * (iss - .029) - 3.25 * (iss - .02) ** 15 if ch else 6.42 * iss
        sc = 0 if imp <= 0 else min((1.08 if ch else 1) * (imp + 8.22 * av * ac * pr * ui), 10)
        return "CRITICAL" if sc >= 9 else "HIGH" if sc >= 7 else "MEDIUM" if sc >= 4 else "LOW" if sc > 0 else "NONE"
    except Exception:
        return "UNKNOWN"


def load_kev():
    d, _ = jget("https://www.cisa.gov/sites/default/files/feeds/known_exploited_vulnerabilities.json")
    for v in (d or {}).get("vulnerabilities", []):
        EXPLOITED.add(v["cveID"])
    print(f"CISA KEV entries: {len(EXPLOITED)}")


def sources(pkg, nvcfg, seccfg):
    """-> (github 'o/r' or None, osv [(eco,name)], skip_reason, glsa_enabled)"""
    s = seccfg.get(pkg, {})
    n = nvcfg.get(pkg, {})
    if s.get("none"):
        return None, [], s["none"], s.get("glsa", True)
    gh = s.get("github") or (n.get("github") if n.get("source") == "github" else None)
    osv = [tuple(x) for x in s.get("osv", [])]
    src = n.get("source")
    if not osv:
        if src == "pypi":
            osv.append(("PyPI", n["pypi"]))
        elif src == "cratesio":
            osv.append(("crates.io", n["cratesio"]))
        elif src == "npm":
            osv.append(("npm", n["npm"]))
    return gh, osv, None, s.get("glsa", True)


def vuln_info(v, ver):
    """Normalise an OSV record -> dict."""
    ids = [v["id"]] + v.get("aliases", [])
    sev = (v.get("database_specific") or {}).get("severity")
    if not sev:
        for s_ in v.get("severity", []):
            sev = cvss_rating(s_.get("score")) or sev
    sev = {"MODERATE": "MEDIUM"}.get(str(sev).upper(), sev) if sev else sev
    fixed, ranges, tags = [], [], []
    for a in v.get("affected", []):
        for r in a.get("ranges", []):
            if r.get("type") == "GIT":
                continue
            intro = None
            for e in r.get("events", []):
                if "introduced" in e:
                    intro = e["introduced"]
                if "fixed" in e:
                    fixed.append(e["fixed"])
                    ranges.append(f">= {intro}, < {e['fixed']}" if intro not in (None, "0") else f"< {e['fixed']}")
                    intro = None
                if "last_affected" in e:
                    ranges.append(f">= {intro}, <= {e['last_affected']}" if intro not in (None, "0") else f"<= {e['last_affected']}")
                    intro = None
            if intro is not None:
                ranges.append(f">= {intro}" if intro != "0" else "all versions")
        tags += a.get("versions", [])
    return {"ids": ids, "summary": v.get("summary") or (v.get("details") or "")[:200].replace("\n", " "),
            "severity": (sev or "UNKNOWN").upper(), "fixed": fixed, "ranges": ranges, "tags": tags,
            "url": f"https://osv.dev/vulnerability/{v['id']}", "src": "OSV"}


def osv_query(eco, name, ver):
    out = {}
    versions = {ver, "v" + ver} if eco == "GIT" else {ver}
    for vv in versions:
        d, _ = jget("https://api.osv.dev/v1/query", {"package": {"name": name, "ecosystem": eco}, "version": vv})
        for v in (d or {}).get("vulns", []):
            if v.get("withdrawn"):
                continue
            out[v["id"]] = v
    return [vuln_info(v, ver) for v in out.values()]


def ghsa_query(repo, ver):
    if gh_remaining[0] is not None and gh_remaining[0] < 3:
        print("  ! GitHub API rate limit reached, skipping GHSA for", repo, file=sys.stderr)
        ghsa_skipped.append(repo)
        return []
    h = {"Accept": "application/vnd.github+json"}
    if os.environ.get("GITHUB_TOKEN"):
        h["Authorization"] = "Bearer " + os.environ["GITHUB_TOKEN"]
    d, hdr = jget(f"https://api.github.com/repos/{repo}/security-advisories?state=published&per_page=100", headers=h)
    if d is None:
        ghsa_skipped.append(repo)
    if hdr is not None:
        gh_remaining[0] = int(hdr.get("X-RateLimit-Remaining", 99))
    out = []
    for a in d or []:
        if a.get("withdrawn_at"):
            continue
        hit, ranges, fixed = False, [], []
        for vu in a.get("vulnerabilities", []):
            r = vu.get("vulnerable_version_range")
            if in_range(ver, r, vu.get("patched_versions") or ""):
                hit = True
            if r:
                ranges.append(r)
            if vu.get("patched_versions"):
                fixed.append(vu["patched_versions"])
        if not hit:
            continue
        ids = [a["ghsa_id"]] + ([a["cve_id"]] if a.get("cve_id") else [])
        out.append({"ids": ids, "summary": a.get("summary", ""), "severity": {"MODERATE": "MEDIUM"}.get((a.get("severity") or "unknown").upper(), (a.get("severity") or "unknown").upper()),
                    "fixed": fixed, "ranges": ranges, "tags": [], "url": a["html_url"], "src": "GHSA"})
    return out


def glsa_feed():
    try:
        body, _ = http("https://security.gentoo.org/glsa/feed.rss")
        root = ET.fromstring(body)
    except Exception as e:
        print("  ! GLSA feed:", e, file=sys.stderr)
        return []
    items = []
    for it in root.iter("item"):
        m = re.match(r"GLSA (\d{6}-\d+): (.*)", it.findtext("title") or "")
        if m:
            items.append((m.group(1), m.group(2)))
    return items


def glsa_query(pkg, ver, feed, cache):
    """GLSA entries whose <package name> is exactly cat/pkg (the ::gentoo package of the same name)."""
    pn = pkg.split("/")[1].lower()
    out = []
    for gid, title in feed:
        if pn not in title.lower().replace(" ", "-") and pn not in title.lower():
            continue
        if gid not in cache:
            try:
                cache[gid] = ET.fromstring(http(f"https://security.gentoo.org/glsa/{gid}.xml")[0])
            except Exception as e:
                print("  ! GLSA", gid, e, file=sys.stderr)
                cache[gid] = None
        root = cache[gid]
        if root is None:
            continue
        for pk in root.iter("package"):
            if pk.get("name") != pkg:
                continue
            hit = False
            for vu in pk.findall("vulnerable"):
                op = {"lt": "lt", "le": "le", "eq": "eq", "gt": "gt", "ge": "ge", "rlt": "lt", "rle": "le", "rge": "ge"}.get(vu.get("range"))
                if op and sat(ver, op, vu.text.strip()):
                    hit = True
            if not hit:
                continue
            refs = [u.text for u in root.iter("uri") if u.text and u.text.startswith("CVE-")]
            imp = (root.find("impact").get("type") if root.find("impact") is not None else "normal")
            sev = {"high": "HIGH", "normal": "MEDIUM", "low": "LOW"}.get(imp, imp.upper())
            out.append({"ids": ["GLSA-" + gid] + refs, "summary": root.findtext("title") or title, "severity": sev,
                        "fixed": [u.get("range") + " " + u.text.strip() for u in pk.findall("unaffected")],
                        "ranges": [vu.get("range") + " " + vu.text.strip() for vu in pk.findall("vulnerable")],
                        "tags": [], "url": f"https://security.gentoo.org/glsa/{gid}", "src": "GLSA"})
    return out


def merge(found):
    """Collapse records sharing any ID (same vuln via OSV + GHSA + GLSA)."""
    groups = []
    for f in found:
        for g in groups:
            if set(g["ids"]) & set(f["ids"]):
                g["ids"] = list(dict.fromkeys(g["ids"] + f["ids"]))
                g["fixed"] = list(dict.fromkeys(g["fixed"] + f["fixed"]))
                g["ranges"] = list(dict.fromkeys(g["ranges"] + f["ranges"]))
                g["tags"] += f["tags"]
                g["src"] += "+" + f["src"]
                if g["severity"] == "UNKNOWN":
                    g["severity"] = f["severity"]
                break
        else:
            groups.append(dict(f))
    return groups


def primary(ids):
    cves = [i for i in ids if i.startswith("CVE-")]
    return cves[0] if cves else ids[0]


def fixed_status(g, upstream):
    """-> (text, has_fix). Commit-range (GIT) advisories only list affected tags: infer from the newest release."""
    fx = [f for f in g["fixed"] if re.search(r"\d", f) and not re.fullmatch(r"[0-9a-f]{40}", f)]
    if fx:
        return ", ".join(dict.fromkeys(fx)), True
    if g["tags"] and upstream:
        top = max(g["tags"], key=norm)
        if norm(upstream) > norm(top):
            return f"not stated; newest affected release is {top}, so upstream {upstream} should contain the fix (verify)", True
    return "NONE KNOWN", False


def body_for(pkg, ver, g, upstream):
    fx, has_fix = fixed_status(g, upstream)
    expl = [i for i in g["ids"] if i in EXPLOITED]
    sev = g["severity"]
    crit = sev in ("CRITICAL",) or bool(expl)
    lines = [
        f"`{pkg}` {ver} is affected by **{', '.join(g['ids'])}**: {g['summary']}",
        "",
        f"- severity: {sev}",
        f"- exploited: {'YES (CISA KEV: ' + ', '.join(expl) + ')' if expl else 'no known exploitation'}",
        f"- affected: {'; '.join(dict.fromkeys(g['ranges'])) or 'see advisory'}",
        f"- fixed-version: {fx}",
        f"- overlay-version: {ver}",
        f"- upstream-newest: {upstream or 'unknown'}",
        f"- source: {g['src']} {g['url']}",
        "",
    ]
    if has_fix:
        lines.append("**FAST-TRACK**: bump to the fixed version in the same manager run once the test-emerge passes, then push to hosts through the normal update path.")
    else:
        lines.append("No fixed version exists yet. Re-check upstream; if severity is CRITICAL or exploited"
                     + (" (it is)" if crit else "") + " the manager escalates for a mitigation, otherwise leave open and watch.")
    lines += ["", "Opened by `.forgejo/workflows/update-check.yml` (`.ci/security-check.py`)."]
    return "\n".join(lines), has_fix, crit


def main():
    nvcfg = tomllib.load(open(uc.CFG, "rb")) if os.path.exists(uc.CFG) else {}
    seccfg = tomllib.load(open(uc.SEC, "rb")) if os.path.exists(uc.SEC) else {}
    pkgs = [p for p in dict.fromkeys(list(nvcfg) + list(seccfg)) if "/" in p]
    try:
        upstream = json.load(open(os.environ.get("UPSTREAM_JSON", "/nonexistent")))
    except Exception:
        upstream = {}
    st = os.environ.get("SELFTEST_SEC")
    st_pkg, st_ver = (st.split("=", 1) if st else (None, None))
    dry = os.environ.get("DRY_RUN") == "1"
    load_kev()
    feed = glsa_feed()
    gcache = {}
    issues = [] if dry else uc.existing_issues()
    blob = "\n".join(t + "\n" + b for t, b in issues if t.startswith("security:"))
    cap = 1 if st else int(os.environ.get("SEC_MAX_NEW", "20"))
    opened, nosrc, checked = 0, [], 0
    todo = [st_pkg] if st else pkgs
    for pkg in todo:
        ver = st_ver if pkg == st_pkg else uc.newest_ebuild(pkg)
        if not ver:
            continue
        gh, osv, none, glsa_on = sources(pkg, nvcfg, seccfg)
        if st and pkg == st_pkg and not (gh or osv):
            print("selftest package has no security source"); continue
        if none:
            nosrc.append(f"{pkg} ({none})")
        found = []
        if gh:
            found += osv_query("GIT", f"https://github.com/{gh}", ver)
            found += ghsa_query(gh, ver)
        for eco, name in osv:
            found += osv_query(eco, name, ver)
        if glsa_on:
            found += glsa_query(pkg, ver, feed, gcache)
        if gh or osv or none is None:
            checked += 1 if (gh or osv) else 0
        if not (gh or osv) and not none:
            nosrc.append(f"{pkg} (no upstream repo/ecosystem mapped)")
        for g in merge(found):
            key = primary(g["ids"])
            if any(i in blob for i in g["ids"]):
                continue
            title = f"security: {pkg} {key}"
            body, has_fix, crit = body_for(pkg, ver, g, upstream.get(pkg))
            tag = "CRITICAL/EXPLOITED no fix" if crit and not has_fix else g["severity"]
            if opened >= cap:
                print(f"[cap] not opened this run: {title} ({tag})")
                continue
            if dry:
                print(f"[dry-run] would open: {title} [{tag}] fixed={fixed_status(g, upstream.get(pkg))[0][:60]}")
            else:
                uc.open_issue(title, body)
                blob += "\n" + title + "\n" + body
                print("opened:", title, f"[{tag}]")
            opened += 1
    print(f"\nSECURITY {os.environ.get('FORGEJO_REPO', '')}: {checked} packages checked against OSV/GHSA, "
          f"{len(feed)} GLSAs scanned, {opened} new issue(s)")
    if ghsa_skipped:
        print(f"  WARNING: GitHub advisories skipped for {len(ghsa_skipped)} repo(s) (API rate limit; set GITHUB_TOKEN): {', '.join(ghsa_skipped)}")
    if nosrc:
        print("  no public advisory source:", "; ".join(nosrc))


if __name__ == "__main__":
    main()
