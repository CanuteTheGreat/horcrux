"""Hardware layers: generic hardware enablement picked by detection, installed as normal binary packages.

A layer is a TOML file in the project's layer directory (live-installer profile.toml key `hwlayer_dir`, default
/usr/share/<project>/hwlayers), shipped in the stock ISO and on installed systems (package sys-apps/horcrux-profile):

    id = "apple-t2"   title = "Apple Mac with the T2 chip"
    [match]   pci = ["106b:1801"]        # any listed vendor:device present
              dmi_sys_vendor = ["..."]   dmi_product_prefix = ["..."]    cpu_vendor = ["GenuineIntel"]   (all given keys must match)
    packages = ["cat/pkg", ...]          # added to @world from the binhosts
    firmware = "apple-t2"                # install-time firmware import (non-redistributable blobs come from the machine itself)
    [kernel]  packages = [...]  unmerge = [...]  binrepo = { name = "...", uri = "https://...", priority = 9000 }
    [portage] use = {...}  accept_keywords = {...}  license = {...}
    [services] enable = ["svc", ...]
    editions = ["gnome", ...]            # optional: only for these editions

Nothing here is machine-specific: which machine it is comes from the machine profile; which hardware it is comes from detection.
"""
import glob
import os

DEFAULT_DIR = "/usr/share/horcrux/hwlayers"


def load_all(layer_dir):
    import tomllib
    out = []
    for p in sorted(glob.glob(os.path.join(layer_dir, "*.toml"))):
        try:
            with open(p, "rb") as f:
                d = tomllib.load(f)
            d.setdefault("id", os.path.basename(p)[:-5])
            out.append(d)
        except Exception:         # a broken layer file never breaks the installer
            continue
    return out


def _rd(p):
    try:
        with open(p) as f:
            return f.read().strip()
    except OSError:
        return ""


def pci_ids(sysfs_pci="/sys/bus/pci/devices"):
    ids = set()
    for d in glob.glob(os.path.join(sysfs_pci, "*")):
        v, dv = _rd(os.path.join(d, "vendor")).lower(), _rd(os.path.join(d, "device")).lower()
        if v and dv:
            ids.add(f"{v.replace('0x', '')}:{dv.replace('0x', '')}")
    return ids


def facts(sysfs_pci="/sys/bus/pci/devices", dmi="/sys/class/dmi/id", cpuinfo="/proc/cpuinfo"):
    cpu = ""
    try:
        for l in open(cpuinfo):
            if l.startswith("vendor_id"):
                cpu = l.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    return {"pci": pci_ids(sysfs_pci), "sys_vendor": _rd(os.path.join(dmi, "sys_vendor")),
            "product_name": _rd(os.path.join(dmi, "product_name")), "cpu_vendor": cpu}


def matches(layer, f, edition=None):
    m = layer.get("match", {})
    if not m:
        return False
    if layer.get("editions") and edition not in layer["editions"]:
        return False
    if "pci" in m and not (set(x.lower() for x in m["pci"]) & f["pci"]):
        return False
    if "dmi_sys_vendor" in m and f["sys_vendor"].lower() not in [x.lower() for x in m["dmi_sys_vendor"]]:
        return False
    if "dmi_product_prefix" in m and not any(f["product_name"].lower().startswith(x.lower()) for x in m["dmi_product_prefix"]):
        return False
    if "cpu_vendor" in m and f["cpu_vendor"] not in m["cpu_vendor"]:
        return False
    return True


def detect(layers, f=None, edition=None, force=()):
    """the layers that apply to this machine, in file order; `force` = ids added by the profile/answer file (e.g. a QEMU fake T2)"""
    f = f if f is not None else facts()
    return [l for l in layers if matches(l, f, edition) or l["id"] in force]


def select(layers, spec, f=None, edition=None, force=()):
    """profile [hardware] layers = "auto" (detect) | "none" | [ids] (exactly these, unknown ids are an error)"""
    if spec in (None, "auto"):
        return detect(layers, f, edition, force)
    if spec == "none":
        return []
    by = {l["id"]: l for l in layers}
    miss = [i for i in spec if i not in by]
    if miss:
        raise ValueError("unknown hardware layer(s): " + ", ".join(miss))
    return [by[i] for i in spec]


def binrepo_index(uri, timeout=20):
    """the text of the binhost's Packages index (network; raises when it cannot be read)"""
    import urllib.request
    with urllib.request.urlopen(uri.rstrip("/") + "/Packages", timeout=timeout) as r:
        return r.read().decode("utf-8", "replace")


def index_has(text, atom_cp):
    return any(l.startswith("CPV: " + atom_cp + "-") for l in text.splitlines())


def binrepo_has(uri, atom_cp, fetch=None, timeout=20):
    """does the binhost at `uri` publish cat/pkg? (reads its Packages index; network)"""
    try:
        txt = fetch(uri.rstrip("/") + "/Packages") if fetch else binrepo_index(uri, timeout)
    except Exception:
        return False
    return index_has(txt, atom_cp)
