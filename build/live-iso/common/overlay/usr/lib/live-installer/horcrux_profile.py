#!/usr/bin/env python3
"""horcrux-profile: apply a declarative MACHINE PROFILE to this system (plan / apply / generations / rollback).

One stock installer ISO serves every machine. What makes a machine "the laptop in the office" or "a lab server" is a small TOML file (the machine
profile) plus optional bundle files next to it. The live installer runs this program inside the freshly installed system; the same
program (package sys-apps/horcrux-profile) applies later edits of the profile, so a reinstall and an edited profile converge to the
same result. Everything comes from BINARY packages (the binhosts in /etc/portage/binrepos.conf); a source build is refused unless it
comes from a repository the profile trusts for source installs (script/config-only packages: the horcrux and private overlays).

Profile (schema 1), every key optional except schema and machine.hostname:

    schema = 1
    [machine]   hostname = "box"  edition = "server"|"gnome-light"|"gnome"|"xfce"  timezone = "UTC"  locale = "en_US.UTF-8"  keymap = "us"
    [disk]      (used by the installer only: disks, filesystem, raid, encryption, swap)
    [[users]]   name = "alice"  fullname = ""  admin = true  groups = ["docker"]  ssh_keys = ["ssh-ed25519 ..."]  password_hash = "$6$..."
    [hardware]  layers = "auto" | ["apple-t2", ...]   t2_firmware = "auto"|"none"|"bundle:firmware/firmware.tar"
    [[repos]]   name = "x"  sync_uri = "..."  sync_type = "git"  priority = 50  snapshot = "bundle:repos/x"  deploy_key = "bundle:keys/x"  source_ok = true
    [[binrepos]] name = "x"  uri = "https://..."  priority = 10
    [software]  packages = ["cat/pkg", ...]   sets = ["@x"]   allow_source = ["cat/pkg"]
                preinstall = ["cat/pkg-config"]   (installed before the rest is resolved: packages that own /etc/portage
                keyword/USE/licence files the other packages need)
    [portage]   use = { "cat/pkg" = "flag -flag" }   accept_keywords = { "cat/pkg" = "~amd64" }   license = { "cat/pkg" = "LIC" }
                mask = { ">=cat/pkg-2" = "" }   unmask = { "=cat/pkg-1" = "" }
    [services]  enable = ["sshd", "name:runlevel"]   disable = [...]
    [[wifi]]    ssid = "Net"  psk = "..."  (or psk_file = "bundle:secrets/wifi-net")  autoconnect = true
    [[files]]   path = "/etc/x"  source = "bundle:files/x" | content = "..."  mode = "0644"  owner = "root:root"

Generations: every apply records /var/lib/horcrux/profile/generations/<N>.json (profile hash, packages, files) and, on a ZFS root with
boot environments, takes a `horcrux-be snapshot profile-<N>` first; `horcrux-profile rollback <N>` re-activates that boot environment.
Removal is declarative too: a package dropped from the profile is deselected from @world (never depcleaned here), a file or
portage line dropped from the profile is removed (only files this program wrote are ever touched).
"""
import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

SCHEMA = 1
STATE = os.environ.get("HORCRUX_PROFILE_STATE", "/var/lib/horcrux/profile")
GEN_DIR = STATE + "/generations"
OWNED = "horcrux-profile"                       # file name used under /etc/portage/package.*/ and repos.conf/binrepos.conf
EDITIONS = ("server", "gnome-light", "gnome", "xfce")
# packages an edition adds on top of the ISO it is installed from (the full GNOME edition from the GNOME (light) ISO)
EDITION_PACKAGES = {"gnome": ["gnome-base/gnome", "gnome-extra/horcrux-gnome-extras"]}
# binary repositories an edition adds: desktop packages built under the GNOME ISO's own Portage profile, so their USE matches a desktop
# install (ctg-binhost lane amd64/gnome, OpenRC). Keyed by (edition, init system, arch); listed after the profile's own [[binrepos]].
# `[software] edition_binrepos = false` in a profile leaves them out.
_GNOME_OPENRC = [{"name": "ctg-binhost-gnome", "uri": "https://f002.backblazeb2.com/file/ctg-binhost/amd64/gnome", "priority": 8000}]
EDITION_BINREPOS = {("gnome", "openrc", "amd64"): _GNOME_OPENRC, ("gnome-light", "openrc", "amd64"): _GNOME_OPENRC}
ATOM_RE = re.compile(r"^(@[A-Za-z0-9_.+-]+|[<>=~]{0,2}[A-Za-z0-9+_.-]+/[A-Za-z0-9+_-]+(-[0-9][A-Za-z0-9._*-]*)?(:[A-Za-z0-9._/*-]+)?(::[A-Za-z0-9_-]+)?)$")
# package.* files also take wildcards: */*, */*::repo, cat/*, */pkg (never in software.packages)
PORTAGE_KEY_RE = re.compile(r"^(\*|[A-Za-z0-9+_.-]+)/(\*|[A-Za-z0-9+_-]+)(::[A-Za-z0-9_-]+)?$")
NAME_RE = re.compile(r"^[a-z_][a-z0-9_-]{0,31}$")
HOST_RE = re.compile(r"^(?!-)[A-Za-z0-9-]{1,63}(?<!-)$")
REPO_RE = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_-]*$")
DEFAULT_SOURCE_OK_REPOS = ("horcrux",)          # our own script/config packages; everything else must be a binary package
NO_COMPILE_CATEGORIES = ("acct-group", "acct-user", "virtual")   # never compile anything: fine from the ebuild on any machine
ADMIN_GROUPS = ("wheel", "portage")
SELF_ATOM = "sys-apps/horcrux-profile"


# ------------------------------------------------------------------------------------------------ loading / validation (pure)
def load(path):
    import tomllib
    with open(path, "rb") as f:
        return tomllib.load(f)


def bundle_path(bundle, ref):
    """'bundle:rel/path' -> absolute path inside the bundle dir (no escape); None for anything else"""
    if not isinstance(ref, str) or not ref.startswith("bundle:"):
        return None
    rel = os.path.normpath(ref[len("bundle:"):])
    if rel.startswith(("/", "..")) or not bundle:
        raise ValueError(f"bad bundle reference {ref!r}")
    return os.path.join(bundle, rel)


def problems(mp, bundle=None):
    """list of human-readable errors; empty = valid. Never echoes secret values."""
    errs = []
    if mp.get("schema") != SCHEMA:
        errs.append(f"schema must be {SCHEMA}")
    m = mp.get("machine", {})
    if not HOST_RE.match(str(m.get("hostname", ""))):
        errs.append("machine.hostname is missing or not a valid host name")
    if m.get("edition", "server") not in EDITIONS:
        errs.append(f"machine.edition must be one of {', '.join(EDITIONS)}")
    names = set()
    for u in mp.get("users", []):
        n = u.get("name", "")
        if not NAME_RE.match(n) or n == "root":
            errs.append(f"users: invalid user name {n!r}")
        if n in names:
            errs.append(f"users: {n} listed twice")
        names.add(n)
        if "password" in u:
            errs.append(f"users.{n}: plain-text passwords are not allowed in a profile (use password_hash, or let the installer ask)")
        ph = u.get("password_hash", "")
        if ph and not re.match(r"^\$(y|gy|7|2[aby]|6|5)\$", ph):
            errs.append(f"users.{n}: password_hash is not a crypt(3) hash")
        for g in u.get("groups", []):
            if not NAME_RE.match(g):
                errs.append(f"users.{n}: invalid group {g!r}")
    sw = mp.get("software", {})
    for a in list(sw.get("packages", [])) + list(sw.get("sets", [])) + list(sw.get("allow_source", [])) + list(sw.get("preinstall", [])):
        if not ATOM_RE.match(str(a)):
            errs.append(f"software: {a!r} is not a package atom")
    for sect in mp.get("portage", {}):
        if sect not in PORTAGE_TABLES:
            errs.append(f"portage.{sect}: unknown table (known: {', '.join(PORTAGE_TABLES)})")
    for sect in PORTAGE_TABLES:
        for a, v in mp.get("portage", {}).get(sect, {}).items():
            if not (ATOM_RE.match(a) or PORTAGE_KEY_RE.match(a)) or "\n" in str(v):
                errs.append(f"portage.{sect}: bad entry for {a!r}")
    for r in mp.get("repos", []) + mp.get("binrepos", []):
        if not REPO_RE.match(str(r.get("name", ""))):
            errs.append(f"repos: invalid repository name {r.get('name')!r}")
    for r in mp.get("binrepos", []):
        if not str(r.get("uri", "")).startswith(("https://", "file://")):
            errs.append(f"binrepos.{r.get('name')}: uri must be https:// or file://")
    for w in mp.get("wifi", []):
        if not w.get("ssid") or len(w["ssid"].encode()) > 32:
            errs.append("wifi: every network needs an ssid (1-32 bytes)")
        if "psk" in w and not (8 <= len(str(w["psk"])) <= 63 or re.fullmatch(r"[0-9a-fA-F]{64}", str(w["psk"]))):
            errs.append(f"wifi.{w.get('ssid')}: psk must be 8-63 characters")
    for f in mp.get("files", []):
        p = str(f.get("path", ""))
        if not p.startswith("/") or ".." in p.split("/") or p.startswith(("/proc", "/sys", "/dev", "/boot")):
            errs.append(f"files: refusing path {p!r}")
        if ("source" in f) == ("content" in f):
            errs.append(f"files.{p}: exactly one of source / content")
    # every bundle: reference must exist (only checkable when the bundle is at hand)
    if bundle:
        for ref in _bundle_refs(mp):
            try:
                bp = bundle_path(bundle, ref)
            except ValueError as e:
                errs.append(str(e))
                continue
            if bp and not os.path.exists(bp):
                errs.append(f"{ref} is not in the profile bundle")
    return errs


def _bundle_refs(mp):
    out = []
    for r in mp.get("repos", []):
        out += [r[k] for k in ("snapshot", "deploy_key") if k in r]
    out += [w["psk_file"] for w in mp.get("wifi", []) if "psk_file" in w]
    out += [f["source"] for f in mp.get("files", []) if "source" in f]
    fw = mp.get("hardware", {}).get("t2_firmware")
    if isinstance(fw, str) and fw.startswith("bundle:"):
        out.append(fw)
    return [x for x in out if isinstance(x, str) and x.startswith("bundle:")]


def profile_hash(mp):
    return hashlib.sha256(json.dumps(mp, sort_keys=True, default=str).encode()).hexdigest()


# ------------------------------------------------------------------------------------------------ desired state (pure)
def desired_atoms(mp, layers=(), distdir="/var/cache/distfiles", installed=lambda atom: False):
    """packages that must be in @world: edition, hardware layers, then the profile's sets and packages (order kept, no duplicates).
    A layer's firmware_package only when its (machine-local) distfile was captured, or when it is already installed."""
    out = list(EDITION_PACKAGES.get(mp.get("machine", {}).get("edition", "server"), []))
    desktop = mp.get("machine", {}).get("edition", "server") != "server"
    for lay in layers:
        out += list(lay.get("packages", []))
        if desktop:
            out += list(lay.get("desktop_packages", []))    # e.g. audio profiles: need the desktop's PipeWire
        out += list(lay.get("kernel", {}).get("packages", []))
        fp = lay.get("firmware_package")
        if fp and (os.path.exists(os.path.join(distdir, fp.get("distfile", ""))) or installed(fp["atom"])):
            out.append(fp["atom"])
    sw = mp.get("software", {})
    out += list(sw.get("preinstall", [])) + list(sw.get("sets", [])) + list(sw.get("packages", []))
    seen, res = set(), []
    for a in out:
        if a not in seen:
            seen.add(a)
            res.append(a)
    return res


# [portage.<table>] -> /etc/portage/package.<table>/horcrux-profile ("atom" = "value"; mask/unmask take "" as the value)
PORTAGE_TABLES = ("use", "accept_keywords", "license", "mask", "unmask")


RESOLVE_FAILED = ("!!!", "masked by:", "there are no ebuilds", "Multiple package instances", "slot conflict")
# warnings emerge prints with "!!!" although the plan is fine
_RESOLVE_WARNINGS = ("!!! The following binary packages have been ignored due to changed dependencies",
                     "!!! The following binary packages have been ignored due to non matching USE")


def resolve_failed(text):
    """True when `emerge -p` output says the request can't be satisfied (its warning blocks about ignored binary packages don't count)"""
    lines, skip = [], False
    for l in (text or "").splitlines():
        if l.startswith(_RESOLVE_WARNINGS):
            skip = True
            continue
        if skip and (not l.strip() or l.startswith((" ", "\t", "NOTE:"))):
            continue
        skip = False
        lines.append(l)
    rest = "\n".join(lines)
    return any(m in rest for m in RESOLVE_FAILED)


_AU_SECTIONS = (("keyword changes", "accept_keywords"), ("USE changes", "use"), ("license changes", "license"),
                ("mask changes", "unmask"))
_AU_LINE = re.compile(r"^([<>=~]*)([A-Za-z0-9+_.-]+/[A-Za-z0-9+_.-]+?)(-[0-9][^\s:]*)?(:\S+)?(::\S+)?\s+(.*)$")


def autounmask_hints(text):
    """emerge --autounmask=y --autounmask-write=n output -> {table: {"cat/pkg": value}} (unversioned, so a later version still matches)"""
    out, table = {}, None
    for line in text.splitlines():
        hdr = [t for k, t in _AU_SECTIONS if f"The following {k} are necessary" in line]
        if hdr:
            table = hdr[0]
            continue
        if table is None or not line.strip() or line.lstrip().startswith(("#", "(")):
            continue
        m = _AU_LINE.match(line.strip())
        if not m:
            table = None
            continue
        val = m.group(6).strip()
        if table == "unmask":
            val = ""
        if table == "use":
            cur = out.setdefault(table, {}).get(m.group(2), "")
            val = " ".join(dict.fromkeys((cur + " " + val).split()))
        out.setdefault(table, {})[m.group(2)] = val
    return out


def format_hints(h):
    return "\n".join(f"[portage.{t}]\n" + "\n".join(f'"{a}" = "{v}"' for a, v in sorted(d.items())) for t, d in h.items())


def resolve_failure(text, limit=80):
    """the part of a failed `emerge -p` that explains it: everything except the merge-list lines and the progress noise.
    Slot conflicts and blockers are reported in free-form paragraphs, so a fixed prefix filter would drop the reason."""
    keep = []
    for l in text.splitlines():
        s = l.strip()
        if not s or re.match(r"^\[(binary|ebuild)", s) or s.startswith(("Calculating dependencies", "These are the packages", "Total:",
                                                                          "Local copy of remote index", "Fetching ")):
            continue
        keep.append(l.rstrip())
    return "\n".join(keep[-limit:]) if keep else text.strip()[-4000:]


def portage_files(mp, layers=(), arch_kw="amd64", init=None):
    """{relative /etc/portage path: text} owned by this program"""
    p = mp.get("portage", {})
    out = {}
    for d in PORTAGE_TABLES:
        sect = "package." + d
        lines = []
        for lay in layers:
            for a, v in lay.get("portage", {}).get(d, {}).items():
                lines.append(f"{a} {v}".strip())
        for a, v in p.get(d, {}).items():
            lines.append(f"{a} {v}".strip())
        if lines:
            out[f"/etc/portage/{sect}/{OWNED}"] = "# written by horcrux-profile from the machine profile: edit the profile, not this file\n" + "\n".join(lines) + "\n"
    repos = [r for r in mp.get("repos", [])]
    if repos:
        txt = ["# written by horcrux-profile from the machine profile"]
        for r in repos:
            txt.append(f"[{r['name']}]")
            txt.append(f"location = /var/db/repos/{r['name']}")
            txt.append(f"priority = {int(r.get('priority', 50))}")
            if r.get("sync_uri"):
                txt.append(f"sync-type = {r.get('sync_type', 'git')}")
                txt.append(f"sync-uri = {r['sync_uri']}")
                txt.append("auto-sync = yes")
                if r.get("deploy_key"):
                    txt.append(f"sync-git-env = GIT_SSH_COMMAND=\"ssh -i /etc/horcrux/profile-keys/{r['name']} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new\"")
            else:
                txt.append("auto-sync = no")
            txt.append("")
        out[f"/etc/portage/repos.conf/{OWNED}.conf"] = "\n".join(txt)
    bins = list(mp.get("binrepos", []))
    if mp.get("software", {}).get("edition_binrepos", True):
        key = (mp.get("machine", {}).get("edition", "server"), init or init_system(), arch_kw)
        bins += [b for b in EDITION_BINREPOS.get(key, []) if b["name"] not in [x.get("name") for x in bins]]
    for lay in layers:
        b = lay.get("kernel", {}).get("binrepo")
        if b:
            bins.append(b)
    if bins:
        txt = ["# written by horcrux-profile from the machine profile / hardware layers"]
        for b in bins:
            txt += [f"[{b['name']}]", f"sync-uri = {b['uri']}", f"priority = {int(b.get('priority', 10))}", ""]
        out[f"/etc/portage/binrepos.conf/{OWNED}.conf"] = "\n".join(txt)
    return out


def user_groups(mp):
    """{user: [groups]}: administrators get wheel + portage (owner rule: every admin on Gentoo is in portage)"""
    res = {}
    for u in mp.get("users", []):
        gs = list(u.get("groups", []))
        if u.get("admin", True):
            gs = list(ADMIN_GROUPS) + gs
        seen = []
        for g in gs:
            if g not in seen:
                seen.append(g)
        res[u["name"]] = seen
    return res


def source_builds(pretend_out):
    """[(cpv, repo)] that `emerge -p` would build from source (lines starting with [ebuild)"""
    out = []
    for l in pretend_out.splitlines():
        m = re.match(r"^\[ebuild[^\]]*\]\s+(\S+?)(?:::(\S+))?\s", l + " ")
        if m:
            out.append((m.group(1), m.group(2) or "gentoo"))
    return out


def forbidden_source_builds(builds, mp):
    ok_repos = set(DEFAULT_SOURCE_OK_REPOS) | {r["name"] for r in mp.get("repos", []) if r.get("source_ok")}
    allow = mp.get("software", {}).get("allow_source", [])
    bad = []
    for cpv, repo in builds:
        if repo in ok_repos or repo == "local" or cpv.split("/")[0] in NO_COMPILE_CATEGORIES:
            continue
        cp = re.sub(r"-[0-9][^/]*$", "", cpv)
        if any(cp == re.sub(r"^[<>=~]+", "", a).split(":")[0] for a in allow):
            continue
        bad.append(f"{cpv}::{repo}")
    return bad


def plan(mp, layers, state):
    """the change list for `plan` (pure): state = {'world': set, 'files': {path: text}, 'groups': {user: set}, 'prev': gen json or {}}"""
    lines = []
    want = desired_atoms(mp, layers)
    for a in want:
        if a not in state.get("world", set()):
            lines.append(f"+ package {a}")
    prev = state.get("prev", {})
    for a in prev.get("packages", []):
        if a not in want and a != SELF_ATOM:          # this program stays (added by apply when the overlay offers it)
            lines.append(f"- package {a} (deselected from @world)")
    files = portage_files(mp, layers)
    files.update({f["path"]: f.get("content", "") for f in mp.get("files", []) if "content" in f})
    for p, t in files.items():
        if state.get("files", {}).get(p) != t:
            lines.append(f"~ file {p}")
    for p in prev.get("files", []):
        if p not in files and p not in [f["path"] for f in mp.get("files", [])]:
            lines.append(f"- file {p}")
    for u, gs in user_groups(mp).items():
        have = state.get("groups", {}).get(u, set())
        miss = [g for g in gs if g not in have]
        if miss:
            lines.append(f"~ user {u}: add groups {','.join(miss)}")
    for w in mp.get("wifi", []):
        lines.append(f"= wifi {w['ssid']}")
    return lines


# ------------------------------------------------------------------------------------------------ the system side
class Sys:
    def __init__(self, bundle=None, log=print, dry=False):
        self.bundle, self.log, self.dry = bundle, log, dry

    def run(self, cmd, check=True, capture=False, env=None):
        self.log("$ " + " ".join(cmd))
        if self.dry:
            return ""
        r = subprocess.run(cmd, text=True, capture_output=capture, env={**os.environ, **(env or {})})
        if check and r.returncode != 0:
            raise RuntimeError(f"command failed ({r.returncode}): {' '.join(cmd)}" + (("\n" + (r.stdout or "")[-3000:] + (r.stderr or "")[-2000:]) if capture else ""))
        return (r.stdout or "") + (r.stderr or "") if capture else r.returncode

    def write(self, path, text, mode=0o644):
        if self.dry:
            self.log(f"write {path}")
            return
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = path + ".hxp-new"
        with open(tmp, "w") as f:
            f.write(text)
        os.chmod(tmp, mode)
        os.replace(tmp, path)

    def world(self):
        out = set()
        for p, pre in (("/var/lib/portage/world", ""), ("/var/lib/portage/world_sets", "")):
            try:
                out |= {pre + l.strip() for l in open(p) if l.strip()}
            except OSError:
                pass
        return out

    def groups_of(self, user):
        out = set()
        try:
            for l in open("/etc/group"):
                p = l.rstrip("\n").split(":")
                if len(p) >= 4 and user in p[3].split(","):
                    out.add(p[0])
        except OSError:
            pass
        return out

    def user_exists(self, user):
        try:
            return any(l.split(":")[0] == user for l in open("/etc/passwd"))
        except OSError:
            return False

    def group_exists(self, g):
        try:
            return any(l.split(":")[0] == g for l in open("/etc/group"))
        except OSError:
            return False


def init_system():
    return "systemd" if os.path.isdir("/run/systemd/system") or (os.path.exists("/sbin/init") and "systemd" in os.path.realpath("/sbin/init")) else "openrc"


def service(sy, name, enable=True):
    lvl = "default"
    if ":" in name:
        name, lvl = name.split(":", 1)
    if init_system() == "systemd":
        sy.run(["systemctl", "enable" if enable else "disable", name], check=False)
    elif enable:
        if os.path.exists(f"/etc/init.d/{name}"):
            sy.run(["rc-update", "add", name, lvl], check=False)
        else:
            sy.log(f"service {name}: no OpenRC script installed (skipped)")
    else:
        sy.run(["rc-update", "del", name, lvl], check=False)


def wifi_files(w, psk):
    """(iwd known-network text, NetworkManager keyfile text) for one network; psk may be '' for an open network"""
    import uuid
    ssid = w["ssid"]
    auto = "true" if w.get("autoconnect", True) else "false"
    iwd = ("[Security]\nPassphrase=%s\n" % psk if psk else "") + "[Settings]\nAutoConnect=%s\n" % auto
    nm = ("[connection]\nid=%s\nuuid=%s\ntype=wifi\nautoconnect=%s\n\n[wifi]\nmode=infrastructure\nssid=%s\n\n" % (ssid, uuid.uuid5(uuid.NAMESPACE_DNS, "wifi:" + ssid), auto, ssid)
          + ("[wifi-security]\nkey-mgmt=wpa-psk\npsk=%s\n\n" % psk if psk else "") + "[ipv4]\nmethod=auto\n\n[ipv6]\nmethod=auto\n")
    return iwd, nm


def iwd_name(ssid, open_net):
    ok = re.fullmatch(r"[A-Za-z0-9 _-]+", ssid)
    base = ssid if ok else "=" + ssid.encode().hex()
    return base + (".open" if open_net else ".psk")


# ------------------------------------------------------------------------------------------------ apply
def load_layers(ids, layer_dirs):
    """layer definitions by id: the packaged directory first (updates), then the copies the installer stored"""
    out = []
    import tomllib
    for i in ids:
        for d in layer_dirs:
            p = os.path.join(d, f"{i}.toml")
            if os.path.exists(p):
                with open(p, "rb") as f:
                    lay = tomllib.load(f)
                lay.setdefault("id", i)
                if load_json(f"{STATE}/options.json").get("layer_kernels") is False:
                    lay.pop("kernel", None)     # this machine keeps the image's stock kernel (set by the installer; TEST ONLY in QEMU)
                out.append(lay)
                break
        else:
            raise RuntimeError(f"hardware layer {i!r} not found in {', '.join(layer_dirs)}")
    return out


def next_generation():
    os.makedirs(GEN_DIR, exist_ok=True)
    ns = [int(os.path.basename(p)[:-5]) for p in glob.glob(GEN_DIR + "/*.json") if os.path.basename(p)[:-5].isdigit()]
    return (max(ns) + 1) if ns else 1, (load_json(f"{GEN_DIR}/{max(ns)}.json") if ns else {})


def load_json(p):
    try:
        with open(p) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def zfs_root():
    try:
        for l in open("/proc/mounts"):
            p = l.split()
            if p[1] == "/" and p[2] == "zfs":
                return True
    except OSError:
        pass
    return False


def apply_accounts(mp, sy):
    """group memberships + ssh keys of the profile's users (accounts themselves are created by the installer or by hand)"""
    for u, gs in user_groups(mp).items():
        if not sy.user_exists(u):
            # the installer creates the first administrator (with the password typed at install); further profile users are created here,
            # with their password_hash or LOCKED (ssh key / an administrator sets the password) - a profile never carries a plain password
            spec = next(x for x in mp.get("users", []) if x["name"] == u)
            sy.run(["useradd", "-m", "-s", "/bin/bash", "-c", spec.get("fullname", u), u], check=False)
            if spec.get("password_hash"):
                if not sy.dry:
                    subprocess.run(["chpasswd", "-e"], input=f"{u}:{spec['password_hash']}\n", text=True, check=False)
            else:
                sy.run(["passwd", "-l", u], check=False)
            if not sy.dry and not sy.user_exists(u):
                sy.log(f"user {u}: could not be created")
                continue
        for g in gs:
            if not sy.group_exists(g):
                if g in ADMIN_GROUPS:
                    sy.run(["groupadd", "-f", "-r", g], check=False)
                else:
                    sy.log(f"user {u}: group {g} does not exist (its package is not installed): skipped")
                    continue
            if g not in sy.groups_of(u):
                sy.run(["usermod", "-aG", g, u])
        keys = [k for k in next((x for x in mp.get("users", []) if x["name"] == u), {}).get("ssh_keys", [])]
        if keys and not sy.dry:
            import pwd
            pw = pwd.getpwnam(u)
            d = os.path.join(pw.pw_dir, ".ssh")
            os.makedirs(d, mode=0o700, exist_ok=True)
            ak = os.path.join(d, "authorized_keys")
            have = open(ak).read().splitlines() if os.path.exists(ak) else []
            sy.write(ak, "\n".join(have + [k for k in keys if k not in have]) + "\n", 0o600)
            for p in (d, ak):
                os.chown(p, pw.pw_uid, pw.pw_gid)


def bzimage_release(path):
    """the release string a bzImage reports (boot protocol header: u16 kernel_version at 0x20E, relative to 0x200); '' when unreadable"""
    try:
        with open(path, "rb") as f:
            h = f.read(0x210)
            off = int.from_bytes(h[0x20E:0x210], "little") + 0x200
            f.seek(off)
            return f.read(256).split(b"\0")[0].decode("ascii", "replace").split(" ")[0]
    except (OSError, ValueError):
        return ""


DRACUT_KERNEL_TXN = "/etc/dracut.conf.d/00-horcrux-profile-kernel-install.conf"


def kernel_transaction(sy, cmd, reinstall=(), mods="/lib/modules"):
    """emerge a transaction that brings a new dist-kernel (a hardware layer's kernel binhost) together with its ZFS module.

    The kernel's postinst builds an initramfs through installkernel/dracut BEFORE zfs-kmod for that kernel is merged (zfs-kmod depends on
    the kernel), and dracut's `zfs` module (forced by the live image's dracut config, and by encrypted-ZFS installs) dies with
    "Failed to find module 'zfs'" on the new kernel, which fails the kernel's postinst and with it the whole transaction. So the zfs
    module is omitted for the duration of the transaction, and every kernel that appeared in /lib/modules is then given its real initramfs
    (all dracut config again, zfs included, now that zfs.ko exists for it); a missing module is a hard error, never a silent
    initramfs without it.

    `reinstall` = the layer's [kernel] reinstall list: module packages (sys-fs/zfs-kmod) that must be merged again from the kernel lane once the
    kernel is in. A kernel of the SAME version as the one already installed (the stock kernel-bin vs the layer's patched gentoo-kernel) does not
    trigger Portage's slot-operator rebuild, so the old zfs.ko stays under the old kernel and the new kernel has none. Binary-only: the lane
    publishes the module built against exactly that kernel, a source build here would be a bug."""
    before = set(os.listdir(mods)) if os.path.isdir(mods) else set()
    if not sy.dry:
        sy.write(DRACUT_KERNEL_TXN, "# horcrux-profile: only while a kernel layer's packages are merged (removed afterwards, the initramfs is rebuilt)\n"
                                    'omit_dracutmodules+=" zfs "\n')
    try:
        sy.run(cmd, env={"PORTAGE_NICENESS": "10"})
        if reinstall and not sy.dry:
            # zfs-kmod's postinst reinstalls the kernel through /usr/src/linux (dist-kernel_reinstall_initramfs): while that symlink still
            # names the stock kernel (the kernel's own postinst leaves it as-is when it points elsewhere) /boot/vmlinuz-<new release>,
            # System.map and config are overwritten with the STOCK kernel's files and the machine boots the unpatched kernel
            # against the new kernel's initramfs. Point it at the new kernel first.
            for kr in sorted(set(os.listdir(mods)) - before if os.path.isdir(mods) else ()):
                if os.path.isdir(f"/usr/src/linux-{kr}"):
                    sy.run(["eselect", "kernel", "set", f"linux-{kr}"], check=False)
        if reinstall:
            sy.run(["emerge", "--oneshot", "--usepkgonly", "--getbinpkg", "--binpkg-respect-user-patches=n", "--quiet-build=y", "--with-bdeps=n", *reinstall],
                   env={"PORTAGE_NICENESS": "10"})
    finally:
        if not sy.dry and os.path.exists(DRACUT_KERNEL_TXN):
            os.remove(DRACUT_KERNEL_TXN)
    if sy.dry:
        return
    chost = (sy.run(["portageq", "envvar", "CHOST"], check=False, capture=True) or "").strip()
    env = {"CHOST": chost} if chost else None
    for kr in sorted(set(os.listdir(mods)) - before if os.path.isdir(mods) else ()):
        if not os.path.isdir(os.path.join(mods, kr, "kernel")):
            continue
        sy.log(f"kernel {kr}: building its initramfs (dracut, zfs included)")
        sy.run(["dracut", "--force", "--no-hostonly", "--kver", kr, f"/boot/initramfs-{kr}.img", kr], env=env)
        rel = bzimage_release(f"/boot/vmlinuz-{kr}")
        if rel and rel != kr:
            raise RuntimeError(f"/boot/vmlinuz-{kr} is a {rel} image, not the {kr} kernel (a module package's reinstall copied another kernel's image): "
                               f"the machine would boot the wrong kernel against this initramfs")


def apply(mp, layers, sy, install=False, sync=True, emerge_args=()):
    """converge the running system (or the installer's chroot) to the profile; returns the generation record"""
    errs = problems(mp, sy.bundle)
    if errs:
        raise RuntimeError("profile is invalid: " + "; ".join(errs))
    gen, prev = next_generation() if not sy.dry else (0, {})
    t0 = time.time()
    rec = {"generation": gen, "time": time.strftime("%FT%TZ", time.gmtime()), "profile_sha256": profile_hash(mp),
           "hostname": mp["machine"]["hostname"], "layers": [l.get("id") for l in layers], "install": install}
    if not install and zfs_root() and shutil.which("horcrux-be"):
        sy.run(["horcrux-be", "snapshot", f"profile-{gen}"], check=False)
        rec["boot_environment_snapshot"] = f"profile-{gen}"
    # 1. repositories: deploy keys, bundled snapshots, repos.conf / binrepos.conf, portage config
    for r in mp.get("repos", []):
        if r.get("deploy_key"):
            k = bundle_path(sy.bundle, r["deploy_key"])
            sy.write(f"/etc/horcrux/profile-keys/{r['name']}", open(k).read(), 0o600)
        if r.get("snapshot"):
            src, dst = bundle_path(sy.bundle, r["snapshot"]), f"/var/db/repos/{r['name']}"
            if not sy.dry:
                if os.path.isdir(dst) and not os.path.isdir(dst + "/.git"):
                    shutil.rmtree(dst)
                if not os.path.isdir(dst):
                    shutil.copytree(src, dst, symlinks=True)
                    # the bundle is root-only (0700/0600); a repository must be readable by Portage's unprivileged metadata/build user
                    for dp, dn, fn in os.walk(dst):
                        os.chmod(dp, 0o755)
                        for f in fn:
                            fp = os.path.join(dp, f)
                            if not os.path.islink(fp):
                                os.chmod(fp, 0o644)
            sy.log(f"repo {r['name']}: snapshot from the profile bundle")
    owned_files = portage_files(mp, layers)
    for p, t in owned_files.items():
        sy.write(p, t)
    for p in prev.get("files", []):                    # dropped from the profile since the last generation: remove what we wrote
        if p not in owned_files and p not in [f["path"] for f in mp.get("files", [])] and os.path.exists(p) and not sy.dry:
            os.remove(p)
            sy.log(f"removed {p} (no longer in the profile)")
    # 2. trees: the image ships an empty ::gentoo; the overlays are synced from their sync-uri
    if sync:
        out = sy.run(["emerge-webrsync", "-q"], check=False, capture=True)
        if out and "local timestamp is newer" not in out and "error" in out.lower() and not os.path.isdir("/var/db/repos/gentoo/sys-apps"):
            raise RuntimeError("could not fetch the Gentoo tree snapshot (network?)")
        for r in ["horcrux"] + [r["name"] for r in mp.get("repos", []) if r.get("sync_uri") and not r.get("snapshot")]:
            sy.run(["emaint", "sync", "-r", r], check=False)
    # 3. packages: binary only (source only for trusted script/config repos and the profile's allow_source list)
    atoms = desired_atoms(mp, layers, installed=lambda a: sy.run(["portageq", "has_version", "/", a], check=False) == 0)
    if SELF_ATOM not in atoms and (sy.run(["portageq", "best_visible", "/", SELF_ATOM], check=False, capture=True) or "").strip().startswith(SELF_ATOM.split("/")[0]):
        atoms.append(SELF_ATOM)          # this program, so later `horcrux-profile apply` runs come from the package
    rec["packages"] = atoms
    base = ["emerge", "--update", "--newuse", "--getbinpkg", "--usepkg", "--select", "--noreplace", "--quiet-build=y", "--with-bdeps=n", *emerge_args]
    if any(lay.get("kernel", {}).get("binrepo") for lay in layers):
        # a hardware layer's kernel binhost lane (the Apple T2 kernel) builds gentoo-kernel with the layer's user patches (/etc/portage/patches), and
        # Portage ignores a binary package whose USER_PATCHES digest differs from the local one: the patch-set package installs in the same
        # transaction, so the digest never matches at resolve time. The lane's signed index is the trust anchor, not the local patch files.
        base.append("--binpkg-respect-user-patches=n")
    pre_atoms = [a for a in mp.get("software", {}).get("preinstall", []) if a in atoms]
    if pre_atoms:
        # packages owning /etc/portage files (keywords, USE, licences) that the rest of the profile needs: Portage only sees those
        # files once they are installed, so they go in first, under the same binary-only rule
        pre = sy.run(base + ["--pretend", "--verbose", "--color=n", *pre_atoms], check=False, capture=True)
        if resolve_failed(pre):
            raise RuntimeError("the preinstall packages of the machine profile can't be resolved:\n" + resolve_failure(pre or ""))
        bad = forbidden_source_builds(source_builds(pre or ""), mp)
        if bad:
            raise RuntimeError("these would be compiled on this machine (no binary package on the binhosts): " + ", ".join(bad)
                               + " - publish them on the binhost or list them in software.allow_source")
        sy.run(base + pre_atoms, env={"PORTAGE_NICENESS": "10"})
    if atoms:
        pre = sy.run(base + ["--pretend", "--verbose", "--color=n", *atoms], check=False, capture=True)
        if resolve_failed(pre):
            # name EVERY missing keyword/USE/licence/unmask at once, in profile syntax, instead of one error per attempt
            au = sy.run(base + ["--pretend", "--color=n", "--autounmask=y", "--autounmask-write=n", "--autounmask-continue=n", *atoms],
                        check=False, capture=True)
            hints = autounmask_hints(au or "")
            if hints:
                raise RuntimeError("the packages need these [portage.*] entries in the machine profile:\n" + format_hints(hints))
            raise RuntimeError("the packages of the machine profile can't be resolved:\n" + resolve_failure(pre or ""))
        bad = forbidden_source_builds(source_builds(pre or ""), mp)
        if bad:
            raise RuntimeError("these would be compiled on this machine (no binary package on the binhosts): " + ", ".join(bad)
                               + " - publish them on the binhost or list them in software.allow_source")
        kernel_layers = [lay for lay in layers if lay.get("kernel", {}).get("packages")]
        if kernel_layers:
            kernel_transaction(sy, base + atoms, sorted({a for lay in kernel_layers for a in lay["kernel"].get("reinstall", [])}))
        else:
            sy.run(base + atoms, env={"PORTAGE_NICENESS": "10"})
    for a in prev.get("packages", []):
        if a not in atoms and a != SELF_ATOM:
            sy.run(["emerge", "--deselect", a], check=False)
    for lay in layers:
        for u in lay.get("kernel", {}).get("unmerge", []):
            if sy.run(["portageq", "has_version", "/", u], check=False) == 0:
                sy.run(["emerge", "--unmerge", u], check=False)
    if not install:      # at install time the installer creates the users after this stage, then runs `apply --accounts-only`
        apply_accounts(mp, sy)
    # 5. Wi-Fi (iwd known networks when iwd is the backend, NetworkManager keyfiles otherwise), files, services
    for w in mp.get("wifi", []):
        psk = w.get("psk", "")
        if w.get("psk_file"):
            psk = open(bundle_path(sy.bundle, w["psk_file"])).read().strip()
        iwd, nm = wifi_files(w, psk)
        if os.path.isdir("/etc/iwd") or shutil.which("iwd") or os.path.exists("/usr/libexec/iwd"):
            sy.write(f"/var/lib/iwd/{iwd_name(w['ssid'], not psk)}", iwd, 0o600)
        sy.write(f"/etc/NetworkManager/system-connections/{re.sub(r'[^A-Za-z0-9_.-]', '_', w['ssid'])}.nmconnection", nm, 0o600)
        sy.log(f"wifi {w['ssid']}: written")
    for f in mp.get("files", []):
        mode = int(str(f.get("mode", "0644")), 8)
        data = f["content"] if "content" in f else open(bundle_path(sy.bundle, f["source"])).read()
        sy.write(f["path"], data, mode)
        if f.get("owner") and not sy.dry:
            sy.run(["chown", f["owner"], f["path"]])
    sv = mp.get("services", {})
    for lay in layers:
        for s in lay.get("services", {}).get("enable", []):
            service(sy, s, True)
    for s in sv.get("enable", []):
        service(sy, s, True)
    for s in sv.get("disable", []):
        service(sy, s, False)
    m = mp.get("machine", {})
    if m.get("timezone") and not install and not sy.dry:
        if os.path.exists(f"/usr/share/zoneinfo/{m['timezone']}"):
            if os.path.lexists("/etc/localtime"):
                os.remove("/etc/localtime")
            os.symlink(f"../usr/share/zoneinfo/{m['timezone']}", "/etc/localtime")
    rec["files"] = sorted(list(owned_files) + [f["path"] for f in mp.get("files", [])])
    rec["seconds"] = round(time.time() - t0, 1)
    if not sy.dry:
        sy.write(f"{GEN_DIR}/{gen}.json", json.dumps(rec, indent=1, sort_keys=True) + "\n")
        cur = f"{STATE}/current"
        if os.path.lexists(cur):
            os.remove(cur)
        os.symlink(f"generations/{gen}.json", cur)
        # the applied profile itself (secrets stay in the root-only bundle dir)
        sy.write(f"{STATE}/profile.toml.applied", json.dumps(mp, indent=1, sort_keys=True, default=str) + "\n", 0o600)
    sy.log(f"generation {gen} applied in {rec['seconds']} s: {len(atoms)} packages, {len(rec['files'])} files")
    return rec


# ------------------------------------------------------------------------------------------------ CLI
def default_profile():
    for p in (f"{STATE}/bundle/profile.toml", "/etc/horcrux/machine-profile.toml"):
        if os.path.exists(p):
            return p
    return f"{STATE}/bundle/profile.toml"


def main(argv=None):
    ap = argparse.ArgumentParser(prog="horcrux-profile", description="apply a declarative machine profile (plan / apply / generations / rollback)")
    ap.add_argument("--profile", default=None, help="profile.toml (default: the one the installer stored, /var/lib/horcrux/profile/bundle/profile.toml)")
    ap.add_argument("--layers", default=None, help="comma-separated hardware layer ids (default: the ones recorded at install)")
    ap.add_argument("--layer-dir", action="append", default=None, help="hardware layer directory (repeatable; default /usr/share/horcrux/hwlayers, then the install-time copies)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("validate")
    sub.add_parser("plan")
    a_apply = sub.add_parser("apply")
    a_apply.add_argument("--install", action="store_true", help="first apply, run by the live installer inside the new system")
    a_apply.add_argument("--no-sync", action="store_true")
    a_apply.add_argument("--dry-run", action="store_true")
    a_apply.add_argument("--accounts-only", action="store_true", help="only group memberships and ssh keys (the installer runs this once the users exist)")
    sub.add_parser("generations")
    rb = sub.add_parser("rollback")
    rb.add_argument("generation", type=int)
    a = ap.parse_args(argv)
    if a.cmd == "generations":
        for p in sorted(glob.glob(GEN_DIR + "/*.json"), key=lambda x: int(os.path.basename(x)[:-5]) if os.path.basename(x)[:-5].isdigit() else 0):
            g = load_json(p)
            print(f"{g.get('generation'):>4}  {g.get('time')}  {g.get('profile_sha256', '')[:12]}  {len(g.get('packages', []))} pkgs  "
                  f"{'(install) ' if g.get('install') else ''}{g.get('boot_environment_snapshot', '')}")
        return 0
    if a.cmd == "rollback":
        g = load_json(f"{GEN_DIR}/{a.generation}.json")
        snap = g.get("boot_environment_snapshot") or (load_json(f"{GEN_DIR}/{a.generation + 1}.json").get("boot_environment_snapshot"))
        if not snap or not shutil.which("horcrux-be"):
            print("no boot-environment snapshot recorded for that generation: edit the profile back and run `horcrux-profile apply`")
            return 2
        return subprocess.call(["horcrux-be", "rollback", snap])
    path = a.profile or default_profile()
    mp = load(path)
    bundle = os.path.dirname(os.path.abspath(path))
    errs = problems(mp, bundle)
    if a.cmd == "validate":
        for e in errs:
            print("ERROR", e)
        print("profile OK" if not errs else f"{len(errs)} problem(s)")
        return 1 if errs else 0
    ids = a.layers.split(",") if a.layers else load_json(f"{STATE}/current").get("layers", [])
    ids = [i for i in ids if i]
    layers = load_layers(ids, a.layer_dir or ["/usr/share/horcrux/hwlayers", f"{STATE}/layers"]) if ids else []
    if a.cmd == "plan":
        sy = Sys(bundle)
        prev = load_json(f"{STATE}/current")
        paths = list(portage_files(mp, layers)) + [f["path"] for f in mp.get("files", []) if "content" in f]
        state = {"world": sy.world(), "files": {p: (open(p, errors="replace").read() if os.path.exists(p) else None) for p in paths},
                 "groups": {u: sy.groups_of(u) for u in user_groups(mp)}, "prev": prev}
        for l in plan(mp, layers, state) or ["(nothing to change)"]:
            print(l)
        return 0
    sy = Sys(bundle, dry=a.dry_run)
    try:
        if a.accounts_only:
            apply_accounts(mp, sy)
            return 0
        apply(mp, layers, sy, install=a.install, sync=not a.no_sync)
    except RuntimeError as e:
        print("horcrux-profile: " + str(e), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
