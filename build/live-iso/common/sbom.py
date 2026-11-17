#!/usr/bin/env python3
"""sbom.py: CycloneDX 1.5 JSON software bill of materials for a live/installer ISO.

Components:
  * every installed Portage package of the image: the package database (/var/db/pkg/<cat>/<pf>/) read from the image's
    root (CATEGORY, PF/PVR, SLOT, LICENSE, USE, repository); the image's own packages database is the source of truth,
    so the SBOM lists exactly what the published ISO contains
  * the Rust workspace the product binaries are built from: every package of Cargo.lock (name, version, source,
    crates.io sha256), with licenses when a `cargo metadata` JSON is given (--cargo-metadata)

usage:
  sbom.py --root ROOT --cargo-lock Cargo.lock [--cargo-metadata meta.json] --name NAME --version VER
          [--iso ISO.sha256-file-or-iso] [--project horcrux] -o OUT.cdx.json
  sbom.py --from-iso ISO ...     extract LiveOS/squashfs.img's var/db/pkg from the ISO first (needs xorriso + unsquashfs)
Exit 1 when the package database is empty or missing (an SBOM without the OS packages is not published).
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import uuid

TOOL = "live-iso sbom.py"


def read(path, default=""):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read().strip()
    except OSError:
        return default


def portage_components(root):
    """One component per /var/db/pkg/<cat>/<pf> entry (sorted)."""
    vdb = os.path.join(root, "var", "db", "pkg")
    out = []
    if not os.path.isdir(vdb):
        return out
    for cat in sorted(os.listdir(vdb)):
        cdir = os.path.join(vdb, cat)
        if not os.path.isdir(cdir) or cat.startswith("."):
            continue
        for pf in sorted(os.listdir(cdir)):
            d = os.path.join(cdir, pf)
            if not os.path.isdir(d) or pf.startswith("-MERGING-"):
                continue
            pvr = read(os.path.join(d, "PVR")) or ""
            pn = read(os.path.join(d, "PN"))
            if not pn:  # older databases: split PF = PN-PVR
                m = re.match(r"^(.*?)-(\d[^-]*(?:-r\d+)?)$", pf)
                pn, pvr = (m.group(1), m.group(2)) if m else (pf, pvr)
            if not pvr:
                pvr = pf[len(pn) + 1:]
            repo = read(os.path.join(d, "repository")) or "gentoo"
            slot = read(os.path.join(d, "SLOT"))
            lic = " ".join(read(os.path.join(d, "LICENSE")).split())
            use = " ".join(read(os.path.join(d, "USE")).split())
            iuse = set(f.lstrip("+-") for f in read(os.path.join(d, "IUSE")).split())
            # USE also carries profile flags (arch, elibc, kernel); keep the flags the package itself declares
            use_pkg = " ".join(f for f in use.split() if f in iuse) if iuse else use
            c = {
                "type": "application" if repo != "gentoo" else "library",   # the product's own overlay packages vs ::gentoo
                "bom-ref": f"gentoo:{cat}/{pf}",
                "group": cat,
                "name": pn,
                "version": pvr,
                "purl": f"pkg:generic/gentoo/{cat}/{pn}@{pvr}?repository={repo}",
                "properties": [
                    {"name": "gentoo:category", "value": cat},
                    {"name": "gentoo:repository", "value": repo},
                    {"name": "gentoo:slot", "value": slot},
                    {"name": "gentoo:use", "value": use_pkg},
                ],
            }
            if lic:
                c["licenses"] = [{"license": {"name": lic}}]
            out.append(c)
    return out


def parse_cargo_lock(text):
    """[{name, version, source, checksum}] from a Cargo.lock (v3/v4), without a TOML dependency (stdlib only)."""
    pkgs, cur = [], None
    for line in text.splitlines():
        line = line.strip()
        if line == "[[package]]":
            cur = {}
            pkgs.append(cur)
            continue
        if line.startswith("[") and line != "[[package]]":
            cur = None
            continue
        if cur is None:
            continue
        m = re.match(r'^(name|version|source|checksum)\s*=\s*"([^"]*)"$', line)
        if m:
            cur[m.group(1)] = m.group(2)
    return [p for p in pkgs if "name" in p and "version" in p]


SPDX_TOKEN = re.compile(r"^[A-Za-z0-9.+-]+$")


def license_entry(expr):
    """cargo license strings are SPDX expressions, old crates use 'MIT/Apache-2.0'."""
    e = expr.replace("/", " OR ").strip()
    toks = [t for t in re.split(r"[\s()]+", e) if t]
    if toks and all(t in ("AND", "OR", "WITH") or SPDX_TOKEN.match(t) for t in toks):
        return [{"expression": " ".join(e.split())}]
    return [{"license": {"name": expr}}]


def cargo_components(lock_text, metadata=None):
    lic = {}
    members = set()
    if metadata:
        for p in metadata.get("packages", []):
            lic[(p["name"], p["version"])] = p.get("license") or ""
        for pid in metadata.get("workspace_members", []):
            for p in metadata.get("packages", []):
                if p.get("id") == pid:
                    members.add((p["name"], p["version"]))
    out = []
    for p in sorted(parse_cargo_lock(lock_text), key=lambda p: (p["name"], p["version"])):
        key = (p["name"], p["version"])
        local = "source" not in p
        c = {
            "type": "application" if local else "library",
            "bom-ref": f"cargo:{p['name']}@{p['version']}" + ("" if local else "#" + p["source"].split("#")[0]),
            "name": p["name"],
            "version": p["version"],
            "purl": f"pkg:cargo/{p['name']}@{p['version']}",
            "properties": [{"name": "cargo:source", "value": p.get("source", "workspace")}],
        }
        if not local and not p["source"].startswith("registry+https://github.com/rust-lang/crates.io-index"):
            c["purl"] += "?repository_url=" + p["source"].split("+", 1)[-1].split("#")[0]
        if p.get("checksum"):
            c["hashes"] = [{"alg": "SHA-256", "content": p["checksum"]}]
        if lic.get(key):
            c["licenses"] = license_entry(lic[key])
        if local or key in members:
            c["properties"].append({"name": "cargo:workspace-member", "value": "true"})
        out.append(c)
    return out


def extract_vdb_from_iso(iso, work):
    """LiveOS/squashfs.img out of the ISO (xorriso), then only var/db/pkg out of the squashfs (unsquashfs)."""
    sq = os.path.join(work, "squashfs.img")
    r = subprocess.run(["xorriso", "-osirrox", "on", "-indev", iso, "-extract", "/LiveOS/squashfs.img", sq],
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if r.returncode != 0 or not os.path.exists(sq):
        raise SystemExit(f"sbom.py: cannot extract LiveOS/squashfs.img from {iso}: {r.stderr[-400:]}")
    root = os.path.join(work, "root")
    r = subprocess.run(["unsquashfs", "-no-progress", "-d", root, sq, "var/db/pkg"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    os.unlink(sq)
    if r.returncode != 0:
        raise SystemExit(f"sbom.py: unsquashfs var/db/pkg failed: {r.stderr[-400:]}")
    return root


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def build(root, lock_text, metadata, name, version, project, iso_sha256=None, now=None):
    os_comps = portage_components(root)
    rust = cargo_components(lock_text, metadata) if lock_text else []
    main = {"type": "operating-system", "bom-ref": f"image:{name}", "name": name, "version": version,
            "supplier": {"name": "Errant Solutions", "url": ["https://errant.solutions/"]},
            "externalReferences": [{"type": "vcs", "url": f"https://github.com/CanuteTheGreat/{project}"},
                                   {"type": "website", "url": f"https://{project}.errant.solutions/"}]}
    if iso_sha256:
        main["hashes"] = [{"alg": "SHA-256", "content": iso_sha256}]
    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{uuid.uuid4()}",
        "version": 1,
        "metadata": {
            "timestamp": (now or datetime.datetime.now(datetime.timezone.utc)).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "tools": {"components": [{"type": "application", "name": TOOL}]},
            "component": main,
            "properties": [{"name": "portage:packages", "value": str(len(os_comps))},
                           {"name": "cargo:packages", "value": str(len(rust))}],
        },
        "components": os_comps + rust,
        "dependencies": [{"ref": main["bom-ref"], "dependsOn": [c["bom-ref"] for c in os_comps + rust]}],
    }
    return bom


def main(argv=None):
    ap = argparse.ArgumentParser()
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--root")
    src.add_argument("--from-iso")
    ap.add_argument("--cargo-lock")
    ap.add_argument("--cargo-metadata")
    ap.add_argument("--name", required=True)
    ap.add_argument("--version", required=True)
    ap.add_argument("--project", default="horcrux")
    ap.add_argument("--iso-sha256", help="hex sha256 of the image (default: computed for --from-iso)")
    ap.add_argument("-o", "--output", required=True)
    a = ap.parse_args(argv)
    work = None
    try:
        root = a.root
        isosum = a.iso_sha256
        if a.from_iso:
            work = tempfile.mkdtemp(prefix="sbom.", dir=os.path.dirname(os.path.abspath(a.output)))
            root = extract_vdb_from_iso(a.from_iso, work)
            isosum = isosum or sha256_file(a.from_iso)
        lock = open(a.cargo_lock, encoding="utf-8").read() if a.cargo_lock else ""
        meta = None
        if a.cargo_metadata:
            try:
                meta = json.load(open(a.cargo_metadata, encoding="utf-8"))
            except (OSError, ValueError) as e:
                print(f"sbom.py: WARNING: cargo metadata unusable ({e}); Rust components without licenses", file=sys.stderr)
        bom = build(root, lock, meta, a.name, a.version, a.project, isosum)
        n_os = int(bom["metadata"]["properties"][0]["value"])
        if n_os == 0:
            print(f"sbom.py: no Portage packages found under {root}/var/db/pkg: refusing to write an SBOM without the OS", file=sys.stderr)
            return 1
        tmp = a.output + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(bom, f, indent=1, sort_keys=False)
            f.write("\n")
        os.replace(tmp, a.output)
        print(f"sbom: {a.output}: {n_os} Portage packages, {bom['metadata']['properties'][1]['value']} Cargo packages")
        return 0
    finally:
        if work:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
