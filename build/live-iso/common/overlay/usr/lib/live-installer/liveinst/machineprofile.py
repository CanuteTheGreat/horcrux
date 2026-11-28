"""Machine profiles in the live installer: ONE stock ISO, any machine.

A machine profile is a small declarative TOML file (schema in /usr/lib/live-installer/horcrux_profile.py) kept in the owner's
private config repository, optionally with bundle files next to it (repository snapshot or deploy key, Wi-Fi secrets, firmware
tarball). The installer finds it, fills its own configuration from it (host name, disk layout and encryption, administrator,
locale/time zone, edition), installs the base system from the ISO as always, and then runs horcrux-profile INSIDE the new system:
detected hardware layers + the profile's packages, all from binary packages. Later edits of the profile apply on the installed
system with `horcrux-profile apply` (generations, rollback), so a reinstall and an edited profile converge to the same result.

Where a profile comes from (first match):
  1. kernel argument <proj>.profile=<https URL | path>   (a profile.toml, a bundle directory or a bundle .tar/.tar.gz/.tgz),
     optional <proj>.profile_token=<token> sent as "Authorization: Bearer <token>" (a QR code can carry the URL)
  2. a volume labelled <PROJ>-PROFILE holding profile.toml (+ bundle files) or bundle.tar(.gz)
  3. the answer file: [profile] source = "<URL | path>"
Secrets never come from the profile: the disk passphrase and login passwords are typed (or come from the answer file used by
automated tests); a profile may carry password HASHES only, and secretcheck still refuses reuse of the disk passphrase.
"""
import json
import os
import re
import shutil
import tarfile
import time
import urllib.request

BUNDLE = "/run/live-installer-profile"
ENGINE = "/usr/lib/live-installer/horcrux_profile.py"
DESKTOP_EDITIONS = ("gnome-light", "gnome", "xfce")


def _engine():
    import importlib.util
    here = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "horcrux_profile.py")
    spec = importlib.util.spec_from_file_location("horcrux_profile", here if os.path.exists(here) else ENGINE)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _unpack(src, dest):
    """src = profile.toml | bundle dir | bundle tarball -> dest dir holding profile.toml (bundle members cannot escape dest)"""
    if os.path.isdir(dest):
        shutil.rmtree(dest)
    os.makedirs(dest, mode=0o700)
    if os.path.isdir(src):
        shutil.copytree(src, dest, dirs_exist_ok=True, symlinks=False)
    elif src.endswith(".toml"):
        shutil.copy(src, os.path.join(dest, "profile.toml"))
    else:
        with tarfile.open(src) as t:
            for m in t.getmembers():
                n = os.path.normpath(m.name)
                if n.startswith(("/", "..")) or not (m.isfile() or m.isdir()):
                    continue
                t.extract(m, dest, filter="data") if hasattr(tarfile, "data_filter") else t.extract(m, dest)
        # a tarball with one top directory: use it as the bundle root
        top = [x for x in os.listdir(dest)]
        if not os.path.exists(os.path.join(dest, "profile.toml")) and len(top) == 1 and os.path.isdir(os.path.join(dest, top[0])):
            inner = os.path.join(dest, top[0])
            for x in os.listdir(inner):
                shutil.move(os.path.join(inner, x), os.path.join(dest, x))
            os.rmdir(inner)
    for dp, dn, fn in os.walk(dest):
        os.chmod(dp, 0o700)
        for f in fn:
            os.chmod(os.path.join(dp, f), 0o600)
    if not os.path.exists(os.path.join(dest, "profile.toml")):
        raise ValueError("the profile bundle has no profile.toml")
    return dest


def fetch(src, dest=None, token=None, timeout=60):
    dest = dest or BUNDLE
    if src.startswith("http://"):
        raise ValueError("machine profiles are fetched over https only")
    if src.startswith("https://"):
        req = urllib.request.Request(src, headers={"User-Agent": "live-installer/1"})
        if token:
            req.add_header("Authorization", "Bearer " + token)
        name = "bundle.tar.gz" if not src.split("?")[0].endswith(".toml") else "profile.toml"
        tmp = os.path.join("/run", "live-installer-profile-" + name)
        with urllib.request.urlopen(req, timeout=timeout) as r, open(tmp, "wb") as o:
            shutil.copyfileobj(r, o)
        try:
            return _unpack(tmp, dest)
        finally:
            os.remove(tmp)
    if not os.path.exists(src):
        raise ValueError(f"machine profile {src} not found")
    return _unpack(src, dest)


def find(proj, cmdline, answers=None, wait=6, blkid=None, mount=None):
    """bundle dir (copied to /run) or None. Never raises for 'no profile'; a profile that is given but broken raises ValueError."""
    m = re.search(rf"(?:^|\s){proj}\.profile=(\S+)", cmdline)
    tok = re.search(rf"(?:^|\s){proj}\.profile_token=(\S+)", cmdline)
    if m:
        return fetch(m.group(1), token=tok.group(1) if tok else None)
    if answers and answers.get("profile", {}).get("source"):
        return fetch(answers["profile"]["source"], token=answers["profile"].get("token"))
    from . import system as S
    label = f"{proj.upper()}-PROFILE"
    for _ in range(max(1, wait)):
        rc, dev = (blkid or (lambda: S.sh(["blkid", "-L", label])))()
        if rc == 0 and dev.strip():
            mnt = "/run/profile-media"
            os.makedirs(mnt, exist_ok=True)
            if os.path.ismount(mnt) or (mount or (lambda d: S.sh(["mount", "-o", "ro", d, mnt])[0]))(dev.strip()) == 0:
                for name in ("profile.toml", "bundle.tar.gz", "bundle.tgz", "bundle.tar"):
                    p = os.path.join(mnt, name)
                    if os.path.exists(p):
                        return fetch(mnt if name == "profile.toml" else p)
        time.sleep(1)
    return None


def load(bundle):
    E = _engine()
    mp = E.load(os.path.join(bundle, "profile.toml"))
    errs = E.problems(mp, bundle)
    if errs:
        raise ValueError("machine profile: " + "; ".join(errs))
    return mp


def iso_edition(root="/"):
    """the edition family this ISO installs, from the live system's os-release VARIANT_ID: server | gnome | xfce"""
    try:
        for l in open(os.path.join(root, "usr/lib/os-release")):
            if l.startswith("VARIANT_ID="):
                v = l.split("=", 1)[1].strip().strip('"').lower()
                return "gnome" if v.startswith("gnome") else v
    except OSError:
        pass
    return "server"


def family(edition):
    return "gnome" if edition in ("gnome", "gnome-light") else edition


def edition_problem(want, iso):
    """the stock ISO of an edition installs that edition family; inside the GNOME family the profile picks light or full (the full
    GNOME packages come from the binhost). A server ISO never becomes a desktop or the other way round."""
    if family(want) == family(iso):
        return None
    hint = {"gnome": "GNOME", "xfce": "XFCE", "server": "server"}.get(family(want), want)
    return f"the profile wants the {want} edition: boot the {hint} installer ISO (this one installs {iso})"


def to_config(mp, c, bundle, iso=None):
    """fill the installer config from the profile (pure apart from reading the ISO edition); returns a list of problems"""
    errs = []
    m = mp.get("machine", {})
    edition = m.get("edition", "server")
    ep = edition_problem(edition, iso or iso_edition())
    if ep:
        errs.append(ep)
    c["hostname"] = m["hostname"]
    for k_in, k_out in (("locale", "locale"), ("keymap", "keymap"), ("timezone", "timezone")):
        if m.get(k_in):
            c[k_out] = m[k_in]
    c["install_type"] = "desktop" if edition in DESKTOP_EDITIONS else "server"
    if m.get("hostid"):
        from . import zfsroot
        hp = zfsroot.hostid_problem(m["hostid"])
        if hp:
            errs.append(f"machine.{hp}")
        else:
            c["hostid"] = str(m["hostid"])      # keep an existing ZFS pool's hostid (e.g. imported `bulk`); written before the pool is created
    boot = mp.get("boot", {})
    if boot.get("kernel_cmdline"):
        kc = str(boot["kernel_cmdline"])
        if "\n" in kc:
            errs.append("boot.kernel_cmdline must be a single line")
        else:
            c["kernel_cmdline"] = kc             # extra kernel args -> be.conf CMDLINE / GRUB_CMDLINE_LINUX (e.g. intel_iommu=on iommu=pt)
    d = mp.get("disk", {})
    if d.get("disks"):
        c["disks"] = list(d["disks"])
    if d.get("filesystem"):
        c["fs"], c["fs_why"] = d["filesystem"], "machine profile"
    if d.get("raid"):
        c["zfs_raid"] = d["raid"]
    if "swap" in d:
        c["swap"] = str(d["swap"]).lower()
    if "encryption" in d:
        c["zfs_encrypt"] = d["encryption"] is True
    if d.get("test_machine_no_encryption"):
        c["test_machine_no_encryption"] = True
    users = mp.get("users", [])
    admin = next((u for u in users if u.get("admin", True)), None)
    if admin:
        c["user_name"] = admin["name"]
        c["user_fullname"] = admin.get("fullname", admin["name"])
        if admin.get("password_hash"):
            c["user_hash"], c["user_password"] = admin["password_hash"], ""
        if admin.get("ssh_keys"):
            c["ssh_keys"] = list(dict.fromkeys(list(c.get("ssh_keys", [])) + list(admin["ssh_keys"])))
    hw = mp.get("hardware", {})
    if hw.get("t2_firmware"):
        c["t2_firmware"] = hw["t2_firmware"]
    if "t2" in hw:
        c["t2"] = str(hw["t2"])
    c["machine_profile"], c["machine_profile_dir"] = mp, bundle
    return errs


# ------------------------------------------------------------------------------------------------ install time
def layer_dir(project):
    return project.get("hwlayer_dir") or f"/usr/share/{project.get('name', 'horcrux')}/hwlayers"


def choose_layers(inst):
    """hardware layers for this install (detection + the profile's [hardware] layers + the T2 decision the installer already made)"""
    from . import hwlayers
    mp = inst.c.get("machine_profile") or {}
    allx = hwlayers.load_all(layer_dir(inst.p))
    force = ["apple-t2"] if inst.t2 else []
    spec = mp.get("hardware", {}).get("layers", "auto")
    sel = hwlayers.select(allx, spec, edition=mp.get("machine", {}).get("edition"), force=force)
    if not inst.t2:
        sel = [l for l in sel if l["id"] != "apple-t2"]       # the installer's T2 decision (answer file t2 = no) wins
    if inst.c.get("t2_kernel") == "skip":                    # TEST ONLY (QEMU): no T2 kernel, see system.config_from_answers
        sel = [{k: v for k, v in l.items() if k != "kernel"} if l["id"] == "apple-t2" else l for l in sel]
    return sel


def _wait_index(uri, wait_s, sleep=time.sleep, fetch=None):
    """(Packages text or None, last error): the autoinstall starts as soon as DHCP is *started*, with a clock that is still wrong (TLS
    certificates not yet valid), so an unreachable binhost is retried for wait_s seconds before it counts as unreachable"""
    from . import hwlayers
    end, err = time.time() + wait_s, ""
    while True:
        try:
            return (fetch or hwlayers.binrepo_index)(uri), ""
        except Exception as e:
            err = f"{type(e).__name__}: {e}"[:120]
        if time.time() >= end:
            return None, err
        sleep(5)


def kernel_preflight(inst, layers, has=None, wait_s=None, sleep=time.sleep, fetch=None):
    """before the disk is touched: a layer that brings its own kernel (T2) must find it on its binhost, unless the ISO already boots one.
    `has` (tests) answers per atom without waiting; otherwise the index is fetched, waiting up to answer key binhost_wait_s (default 300 s)
    for the network and the clock, and "cannot reach" is told apart from "not published yet"."""
    from . import hwlayers, t2
    out = []
    if wait_s is None:
        wait_s = int(getattr(inst, "c", {}).get("binhost_wait_s", 300))
    for lay in layers:
        k = lay.get("kernel", {})
        if not k.get("packages"):
            continue
        if lay["id"] == "apple-t2" and t2.image_has_t2_kernel("/"):
            continue
        b = k.get("binrepo", {})
        uri = b.get("uri", "")
        idx = None
        if has is None:
            idx, err = _wait_index(uri, wait_s, sleep, fetch)
            if idx is None:
                out.append(f"hardware layer {lay['id']}: cannot reach {uri or 'its binhost'} after {wait_s} s ({err}): is the network up and the clock set?")
                continue
        for a in k["packages"]:
            if not (has(uri, a) if has is not None else hwlayers.index_has(idx, a)):
                out.append(f"hardware layer {lay['id']}: {a} is not published on {uri or 'its binhost'} yet")
    return out


def _copy_engine(inst):
    T = inst.target
    os.makedirs(f"{T}/var/tmp/horcrux-profile", exist_ok=True)
    eng = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "horcrux_profile.py")
    shutil.copy(eng if os.path.exists(eng) else ENGINE, f"{T}/var/tmp/horcrux-profile/horcrux_profile.py")


def accounts(inst):
    """after the installer created the users (finish stage): the profile's group memberships and ssh keys"""
    if not inst.c.get("machine_profile"):
        return
    _copy_engine(inst)
    try:
        inst.run(["python3", "/var/tmp/horcrux-profile/horcrux_profile.py", "--profile", "/var/lib/horcrux/profile/bundle/profile.toml",
                  "apply", "--accounts-only"], chroot=True)
    finally:
        shutil.rmtree(f"{inst.target}/var/tmp/horcrux-profile", ignore_errors=True)


def stage(inst):
    """run horcrux-profile inside the installed system (network needed; the live system's resolv.conf is copied in)"""
    c = inst.c
    mp, bundle = c.get("machine_profile"), c.get("machine_profile_dir")
    layers = getattr(inst, "hw_layers", None)
    if layers is None:
        layers = choose_layers(inst)
    T = inst.target
    st = f"{T}/var/lib/horcrux/profile"
    os.makedirs(st, mode=0o700, exist_ok=True)
    if mp:
        dst = f"{st}/bundle"
        if os.path.isdir(dst):
            shutil.rmtree(dst)
        shutil.copytree(bundle, dst)
        for dp, dn, fn in os.walk(dst):
            os.chmod(dp, 0o700)
            for f in fn:
                os.chmod(os.path.join(dp, f), 0o600)
        prof = "/var/lib/horcrux/profile/bundle/profile.toml"
    else:
        # no machine profile: the hardware layers alone, through a generated minimal profile
        if not layers:
            return None
        txt = f'schema = 1\n[machine]\nhostname = "{c["hostname"]}"\nedition = "{"server" if c.get("install_type") != "desktop" else "gnome-light" if iso_edition() == "gnome" else iso_edition()}"\n'
        os.makedirs(f"{st}/bundle", mode=0o700, exist_ok=True)
        with open(f"{st}/bundle/profile.toml", "w") as f:
            f.write(txt)
        prof = "/var/lib/horcrux/profile/bundle/profile.toml"
    os.makedirs(f"{st}/layers", exist_ok=True)
    if c.get("t2_kernel") == "skip":          # TEST ONLY (QEMU): every later `horcrux-profile apply` keeps the stock kernel too
        with open(f"{st}/options.json", "w") as f:
            json.dump({"layer_kernels": False, "why": "answer file t2_kernel = skip (QEMU test: no T2 hardware)"}, f)
    src = layer_dir(inst.p)
    for lay in layers:
        shutil.copy(os.path.join(src, f"{lay['id']}.toml"), f"{st}/layers/{lay['id']}.toml")
    _copy_engine(inst)
    if os.path.exists("/etc/resolv.conf"):
        shutil.copy("/etc/resolv.conf", f"{T}/etc/resolv.conf")
    fw = getattr(inst, "apple_fw_tar", None)
    if fw:
        os.makedirs(f"{T}/var/cache/distfiles", exist_ok=True)
        shutil.copy(fw, f"{T}/var/cache/distfiles/apple-t2-firmware-local.tar")
    ids = ",".join(l["id"] for l in layers)
    t0 = time.time()
    cmd = ["python3", "/var/tmp/horcrux-profile/horcrux_profile.py", "--profile", prof, "--layer-dir", "/var/lib/horcrux/profile/layers"]
    if ids:
        cmd += ["--layers", ids]
    cmd += ["apply", "--install"]
    try:
        inst.run(cmd, chroot=True)
    finally:
        shutil.rmtree(f"{T}/var/tmp/horcrux-profile", ignore_errors=True)
    inst.log(f"machine profile applied in {time.time() - t0:.0f} s (layers: {ids or 'none'})")
    return time.time() - t0
