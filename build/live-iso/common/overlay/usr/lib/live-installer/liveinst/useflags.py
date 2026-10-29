"""USE-flag model for the installer: descriptions, presets, Portage sanity check, binpkg-vs-source impact.

Data comes from /usr/share/live-installer/portage-meta.json.xz, generated at ISO build time by
mk-portage-meta (profiles/use.desc + use.local.desc + the installed-package database: IUSE, USE,
REQUIRED_USE and the USE-dependent dependencies/blockers of every installed package).  The Portage
tree itself is not shipped on the installed system.  No UI code here; GUI and TUI both call this.
"""
import json
import lzma
import os
import re

META_PATHS = ("/usr/share/live-installer/portage-meta.json.xz", "/usr/share/live-installer/portage-meta.json")

# --------------------------------------------------------------------------
# REQUIRED_USE
# --------------------------------------------------------------------------


def _tokens(s):
    return re.findall(r"\(|\)|[^\s()]+", s or "")


def parse_required_use(s):
    toks = _tokens(s)
    pos = [0]

    def group():
        out = []
        while pos[0] < len(toks):
            t = toks[pos[0]]
            if t == ")":
                pos[0] += 1
                return out
            pos[0] += 1
            if t == "(":
                out.append(("all", group()))
            elif t in ("||", "^^", "??"):
                if pos[0] < len(toks) and toks[pos[0]] == "(":
                    pos[0] += 1
                out.append(({"||": "any", "^^": "one", "??": "most"}[t], group()))
            elif t.endswith("?"):
                neg = t.startswith("!")
                name = t.lstrip("!")[:-1]
                if pos[0] < len(toks) and toks[pos[0]] == "(":
                    pos[0] += 1
                out.append(("cond", (name, neg), group()))
            else:
                neg = t.startswith("!")
                out.append(("flag", t.lstrip("!"), neg))
        return out
    return ("all", group())


def _names(children):
    return [n[1] if n[0] == "flag" else "(...)" for n in children]


def ru_eval(node, use):
    k = node[0]
    if k == "flag":
        return (node[1] in use) != node[2]
    if k == "cond":
        (name, neg) = node[1]
        return all(ru_eval(c, use) for c in node[2]) if ((name in use) != neg) else True
    if k == "all":
        return all(ru_eval(c, use) for c in node[1])
    vals = [ru_eval(c, use) for c in node[1]]
    if k == "any":
        return any(vals)
    if k == "one":
        return sum(vals) == 1
    return sum(vals) <= 1       # most


def ru_explain(node, use, because=""):
    """Plain-language reasons why `node` is violated under `use` ([] when satisfied)."""
    if ru_eval(node, use):
        return []
    k = node[0]
    if k == "flag":
        return [f"{because}'{node[1]}' must be {'off' if node[2] else 'on'}"]
    if k == "cond":
        (name, neg) = node[1]
        why = f"because '{name}' is {'off' if neg else 'on'}, "
        out = []
        for c in node[2]:
            out += ru_explain(c, use, because + why)
        return out
    if k == "all":
        out = []
        for c in node[1]:
            out += ru_explain(c, use, because)
        return out
    names = ", ".join(f"'{n}'" for n in _names(node[1]))
    word = {"any": "at least one of", "one": "exactly one of", "most": "at most one of"}[k]
    return [f"{because}{word} {names} must be on"]


# --------------------------------------------------------------------------
# groups / texts
# --------------------------------------------------------------------------
GROUPS = ["Security and cryptography", "Networking", "Virtualization and containers", "Storage and file systems",
          "Graphics and desktop", "Audio and video", "Hardware and drivers", "Compression", "Languages and bindings",
          "Documentation and development", "Internationalization", "Other"]
_G_NAMES = {
    "Security and cryptography": "ssl openssl gnutls pam seccomp caps hardened pie ssp selinux apparmor tpm tpm2 pkcs11 sasl kerberos gcrypt nettle ldap smartcard libressl",
    "Networking": "ipv6 dhcp dns http2 http3 curl nftables iptables netlink bridge vpn wifi wireless networkmanager samba nfs snmp sctp",
    "Virtualization and containers": "kvm qemu libvirt spice vnc usbredir virgl xen lxc docker virtual virt-network vhost-net vhost-user-fs numa",
    "Storage and file systems": "lvm raid zfs xfs btrfs device-mapper cryptsetup udev nvme iscsi fuse acl xattr ceph",
    "Graphics and desktop": "X wayland gtk gtk3 gtk4 qt5 qt6 opengl vulkan gui cairo png jpeg svg opengl egl gles2 xcb",
    "Audio and video": "alsa pulseaudio pipewire sound ffmpeg gstreamer v4l jack vaapi vdpau opus flac mp3 vorbis",
    "Hardware and drivers": "usb pci bluetooth firmware libusb uvc cpu_flags",
    "Compression": "zstd lzma lz4 lzo bzip2 zlib brotli xz gzip",
    "Languages and bindings": "python perl ruby lua tcl java php guile go rust",
    "Documentation and development": "doc man examples test debug static-libs gtk-doc valgrind verify-sig introspection vala doctool nls-tools tools",
    "Internationalization": "nls unicode icu l10n",
}
_G_NAME = {}
for _g, _ns in _G_NAMES.items():
    for _n in _ns.split():
        _G_NAME.setdefault(_n, _g)
_G_DESC = [
    (r"encrypt|crypto|\bTLS\b|\bSSL\b|authent|certificate|password|sandbox|security|signature|secure", "Security and cryptography"),
    (r"virtual|emulat|hypervisor|\bguest\b|container", "Virtualization and containers"),
    (r"filesystem|file system|storage|\bdisk\b|volume|\bRAID\b|\bLVM\b|block device", "Storage and file systems"),
    (r"network|protocol|IPv[46]|\bDNS\b|\bDHCP\b|\bVPN\b|socket|firewall|netfilter|wireless|\bhttp", "Networking"),
    (r"graphic|\bGUI\b|display|X11|OpenGL|\bimage\b|\bfont|render|toolkit|desktop|\bGTK|\bQt\b", "Graphics and desktop"),
    (r"audio|video|sound|media|codec|music|stream", "Audio and video"),
    (r"hardware|device|\bUSB\b|driver|bluetooth|sensor|\bCPU\b", "Hardware and drivers"),
    (r"compress|archive", "Compression"),
    (r"binding|interpreter|language|script|python|\bperl\b|\bruby\b|\blua\b|\bjava", "Languages and bindings"),
    (r"documentation|\bdebug|\btest|development|header|man page|example", "Documentation and development"),
    (r"locale|language support|translation|unicode|\bnative language", "Internationalization"),
]

SECURITY_KEEP = {   # disabling these weakens the system
    "pam": "PAM provides account authentication and lockout; without it logins use weaker built-in paths.",
    "ssl": "TLS/SSL support protects network traffic; disabling it can leave services speaking plain text.",
    "openssl": "OpenSSL backs TLS in many services; disabling it can force plain-text or weaker fallbacks.",
    "gnutls": "GnuTLS backs TLS in many services; disabling it can force plain-text or weaker fallbacks.",
    "seccomp": "seccomp filters syscalls of sandboxed services; disabling it removes a containment layer.",
    "caps": "Linux capabilities let services drop root privileges; disabling it makes them run with more rights.",
    "pie": "Position-independent executables make exploits harder (ASLR).",
    "ssp": "Stack protector detects stack smashing.",
    "hardened": "The hardened toolchain options raise exploit resistance.",
    "pkcs11": "Smartcard/HSM key storage.",
    "tpm": "TPM based key protection.",
    "tpm2": "TPM2 based key protection.",
    "kerberos": "Kerberos single sign-on.",
}
SECURITY_AVOID = {  # enabling these adds risk or attack surface
    "debug": "Debug builds can leak internals and are slower; do not use on production systems.",
    "test": "Test suites bloat the build and pull test-only dependencies.",
    "X": "Adds an X11 stack (more code, more attack surface) to a headless server.",
    "wayland": "Adds a desktop stack to a headless server.",
    "gui": "Adds graphical tooling to a headless server.",
    "pulseaudio": "Adds a sound server to a headless server.",
    "bluetooth": "Adds a wireless attack surface to a server.",
    "cups": "Adds a print server (network-listening) to the system.",
}

DEFAULT_MINIMAL_OFF = ["X", "wayland", "gtk", "gtk3", "qt5", "qt6", "gui", "pulseaudio", "pipewire", "alsa", "sound", "cups",
                       "bluetooth", "doc", "examples", "test", "debug", "gtk-doc", "gstreamer", "vulkan", "introspection",
                       "static-libs", "ffmpeg", "v4l", "jack"]
DEFAULT_FULL_ON = ["spice", "usbredir", "vnc", "sasl", "lzo", "zstd", "lz4", "brotli", "http2", "ipv6", "pam", "seccomp",
                   "caps", "acl", "xattr", "nls", "icu", "readline", "ncurses", "threads"]

BUILD_MIN = {"app-emulation/qemu": 28, "app-emulation/libvirt": 9, "llvm-core/llvm": 70, "media-libs/mesa": 35,
             "gui-libs/gtk": 14, "dev-lang/rust": 100, "dev-lang/python": 12, "sys-devel/gcc": 120,
             "sys-kernel/gentoo-kernel": 25, "sys-fs/zfs": 8, "sys-fs/zfs-kmod": 8, "net-libs/webkit-gtk": 150,
             "app-containers/docker": 20, "dev-lang/go": 20, "sys-libs/glibc": 25}

EXPAND_PREFIXES = ("python_targets_", "python_single_target_", "ruby_targets_", "video_cards_", "cpu_flags_", "abi_x86_",
                   "abi_", "llvm_slot_", "llvm_targets_", "input_devices_", "l10n_", "linguas_", "lua_targets_",
                   "lua_single_target_", "php_targets_", "qemu_softmmu_targets_", "qemu_user_targets_", "libreoffice_extensions_",
                   "kernel_", "elibc_", "userland_", "ruby_", "perl_features_", "grub_platforms_", "office_implementation_",
                   "ocaml_", "dist_kernel", "bootstrap", "ada_target_", "guile_targets_", "guile_single_target_",
                   "rust_cpu_targets_", "nginx_modules_", "apache2_modules_", "apache2_mpms_", "openmpi_", "sane_backends_",
                   "pypy3_", "rubyopt", "gpsd_protocols_", "curl_ssl_", "cameras_", "boost_", "cpu_", "alsa_cards_",
                   "icu_", "freetype_", "plugins_")


def is_expand(flag):
    return flag.startswith(EXPAND_PREFIXES)


def group_of(flag, desc=""):
    if flag in _G_NAME:
        return _G_NAME[flag]
    for rx, g in _G_DESC:
        if re.search(rx, desc, re.I):
            return g
    return "Other"


# --------------------------------------------------------------------------
# metadata
# --------------------------------------------------------------------------
def load_meta(path=None):
    for p in ([path] if path else ([os.environ["LIVEINST_META"]] if os.environ.get("LIVEINST_META") else []) + list(META_PATHS)):
        if p and os.path.exists(p):
            try:
                op = lzma.open if p.endswith(".xz") else open
                with op(p, "rt") as f:
                    return Meta(json.load(f))
            except (OSError, ValueError, lzma.LZMAError):
                continue
    return None


class Meta:
    def __init__(self, d):
        self.d = d
        self.pkgs = d.get("pkgs", {})
        self.desc = d.get("desc", {})
        self.local = d.get("local", {})
        self.flag_pkgs = {}        # flag -> [pkg]
        self.state = {}            # flag -> True / False / None (mixed)
        for pk, info in self.pkgs.items():
            use = set(info.get("use", []))
            for f in info.get("iuse", []):
                if is_expand(f):
                    continue
                self.flag_pkgs.setdefault(f, []).append(pk)
        for f, pks in self.flag_pkgs.items():
            on = [f in self.pkgs[pk].get("use", []) for pk in pks]
            self.state[f] = True if all(on) else (False if not any(on) else None)
        self.all_flags = sorted(set(self.flag_pkgs) | {f for f in self.desc if not is_expand(f)})
        self._ru = {}

    # -- listing -----------------------------------------------------------
    def describe(self, flag):
        if flag in self.desc:
            return self.desc[flag]
        for pk in self.flag_pkgs.get(flag, []):
            t = self.local.get(pk, {}).get(flag)
            if t:
                return t
        return "(no description in the Portage tree)"

    def flag_rows(self):
        rows = []
        for f in self.all_flags:
            d = self.describe(f)
            pks = self.flag_pkgs.get(f, [])
            rows.append({"flag": f, "desc": d, "group": group_of(f, d), "state": self.state.get(f), "packages": pks,
                         "used": bool(pks)})
        return rows

    def search(self, q, rows=None):
        q = (q or "").lower().strip()
        rows = rows if rows is not None else self.flag_rows()
        if not q:
            return rows
        out = []
        for r in rows:
            if q in r["flag"].lower() or q in r["desc"].lower() or any(q in p.lower() for p in r["packages"]):
                out.append(r)
        return out

    def ru(self, pk):
        if pk not in self._ru:
            self._ru[pk] = parse_required_use(self.pkgs[pk].get("ru", ""))
        return self._ru[pk]

    # -- effective USE --------------------------------------------------------
    def effective(self, pk, choices):
        info = self.pkgs[pk]
        use = set(info.get("use", []))
        for f in info.get("iuse", []):
            v = choices.get(f"{pk}:{f}", choices.get(f))
            if v is True:
                use.add(f)
            elif v is False:
                use.discard(f)
        return use

    def changed_pkgs(self, choices):
        out = []
        for pk, info in self.pkgs.items():
            if self.effective(pk, choices) != set(info.get("use", [])):
                out.append(pk)
        return out


def effective_choices(meta, profile, use):
    """preset overrides + the user's own per-flag choices (what really gets written)"""
    flags = dict(preset_flags(meta, profile, use.get("preset", "recommended")))
    flags.update(use.get("flags", {}))
    return flags


# --------------------------------------------------------------------------
# presets
# --------------------------------------------------------------------------
def critical_entries(profile):
    out = []
    for e in profile.get("use_critical", []):
        pk, _, fl = e.rpartition(":")
        out.append((pk, fl))
    return out


def is_critical(profile, flag):
    return any(fl == flag for _pk, fl in critical_entries(profile))


def preset_flags(meta, profile, name):
    """Overrides (flag -> bool) a preset applies on top of the image's own USE set."""
    if name == "minimal":
        want = [(f, False) for f in profile.get("use_preset_minimal_off", DEFAULT_MINIMAL_OFF)]
    elif name == "full":
        want = [(f, True) for f in profile.get("use_preset_full_on", DEFAULT_FULL_ON)]
    else:
        return {}
    out = {}
    for f, val in want:
        if f not in meta.flag_pkgs or is_critical(profile, f):
            continue
        st = meta.state.get(f)
        if st is val:
            continue
        out[f] = val
    return prune_preset(meta, profile, out)


def prune_preset(meta, profile, out):
    """A preset must never ship a Portage error: drop the flags the sanity check blames (e.g. 'introspection' off breaks
    gobject-introspection's USE dependency on glib) until the preset on its own is clean."""
    out = dict(out)
    for _ in range(40):
        errs = [i for i in check(meta, profile, out) if i.level == "error"]
        if not errs:
            break
        blamed = {i.flag for i in errs if i.flag in out}
        if not blamed:
            break
        for f in blamed:
            out.pop(f, None)
    return out


PRESETS = [("recommended", "Recommended", "The tested image defaults: every package stays a pre-built binary."),
           ("minimal", "Minimal", "Switches off desktop, sound, docs and debug features; smaller and a smaller attack surface."),
           ("full", "Full", "Switches on extra protocols, compression and hardware features; more is built from source.")]


# --------------------------------------------------------------------------
# dependency constraints
# --------------------------------------------------------------------------
def _atom_pkg(atom):
    a = re.sub(r"\[.*\]$", "", atom).lstrip("!~<>=")
    a = re.sub(r":.*$", "", a)
    a = re.sub(r"-\d.*$", "", a)
    return a.rstrip("*")


def _usedeps(atom):
    m = re.search(r"\[([^\]]*)\]$", atom)
    return [t.strip() for t in m.group(1).split(",") if t.strip()] if m else []


def _norm(tok):
    return re.sub(r"\([+-]\)", "", tok)


def _cond_ok(cond, use):
    for c in cond:
        neg = c.startswith("!")
        if ((c.lstrip("!") in use) == neg):
            return False
    return True


# --------------------------------------------------------------------------
# the Portage sanity check
# --------------------------------------------------------------------------
class Issue:
    def __init__(self, level, code, text, flag="", pkg=""):
        self.level, self.code, self.text, self.flag, self.pkg = level, code, text, flag, pkg

    def __repr__(self):
        return f"<{self.level} {self.code}: {self.text}>"


def check(meta, profile, choices):
    """Return a list of Issue. level: error (blocks 'Next'), warning, info."""
    issues = []
    prod = profile.get("title", "the product")
    why = profile.get("use_critical_why", {})
    # 1. product-critical flags: must keep the image value (they are what the product was built and tested with)
    for pk, fl in critical_entries(profile):
        want = None
        pks = [pk] if pk else meta.flag_pkgs.get(fl, [])
        for p_ in pks:
            if p_ not in meta.pkgs or fl not in meta.pkgs[p_].get("iuse", []):
                continue
            base = fl in meta.pkgs[p_].get("use", [])
            got = fl in meta.effective(p_, choices)
            if got != base:
                label = f"{p_}[{fl}]" if p_ else f"'{fl}'"
                reason = why.get(f"{p_}:{fl}", why.get(fl, f"{prod} needs it"))
                issues.append(Issue("error", "critical", f"{label} must stay {'ON' if base else 'OFF'}: {reason}. "
                                    f"{prod} would not work as installed.", fl, p_))
                break
    changed = meta.changed_pkgs(choices)
    base_choices = {}
    # 2. REQUIRED_USE of changed packages
    for pk in changed:
        node = meta.ru(pk)
        if not meta.pkgs[pk].get("ru"):
            continue
        base = set(meta.pkgs[pk].get("use", []))
        now = meta.effective(pk, choices)
        if ru_eval(node, now):
            continue
        reasons = ru_explain(node, now)
        flips = sorted(f for f in now ^ base if f in meta.pkgs[pk].get("iuse", []))
        issues.append(Issue("error", "required_use",
                            f"{pk}: REQUIRED_USE is not satisfied ({'; '.join(reasons) or meta.pkgs[pk]['ru']}). "
                            f"Portage would refuse to build it with {', '.join(('-' if f in base else '') + f for f in flips)}.",
                            flips[0] if flips else "", pk))
    # 3. USE dependencies and blockers
    for pk, info in meta.pkgs.items():
        base_use = set(info.get("use", []))
        now_use = meta.effective(pk, choices)
        for cond, atom in info.get("cons", []):
            dep = _atom_pkg(atom)
            if atom.startswith("!"):
                if dep in meta.pkgs and _cond_ok(cond, now_use) and not _cond_ok(cond, base_use):
                    issues.append(Issue("error", "blocker",
                                        f"{pk} would block {dep}: with these flags {pk} declares a blocker against the installed {dep}. "
                                        f"Portage cannot install both.", cond[0].lstrip("!") if cond else "", pk))
                continue
            if dep not in meta.pkgs or not _cond_ok(cond, now_use):
                continue
            tuse_now = meta.effective(dep, choices)
            tuse_base = set(meta.pkgs[dep].get("use", []))
            tiuse = set(meta.pkgs[dep].get("iuse", []))
            for tok in _usedeps(atom):
                bad = _usedep_violation(_norm(tok), now_use, tuse_now, tiuse)
                if bad is None:
                    continue
                if _usedep_violation(_norm(tok), base_use, tuse_base, tiuse) is None:      # newly broken
                    f = bad[0]
                    issues.append(Issue("error", "use_dep",
                                        f"{pk} needs {dep} built with '{f}' {'on' if bad[1] else 'off'}, "
                                        f"but your choices set it {'off' if bad[1] else 'on'} for {dep}. "
                                        f"Portage reports this as an unsatisfied USE dependency ({atom}).", f, dep))
    # 4. security warnings
    seen = set()
    for f, val in choices.items():
        fl = f.rpartition(":")[2]
        if fl in seen:
            continue
        pks = meta.flag_pkgs.get(fl, [])
        if fl in SECURITY_KEEP and val is False and any(fl in meta.pkgs[p_].get("use", []) for p_ in pks):
            issues.append(Issue("warning", "security", f"Disabling '{fl}' weakens security: {SECURITY_KEEP[fl]}", fl))
            seen.add(fl)
        elif fl in SECURITY_AVOID and val is True and not all(fl in meta.pkgs[p_].get("use", []) for p_ in pks):
            issues.append(Issue("warning", "security", f"Enabling '{fl}': {SECURITY_AVOID[fl]}", fl))
            seen.add(fl)
    # 5. flags that no installed package has
    for f in choices:
        fl = f.rpartition(":")[2]
        if fl not in meta.flag_pkgs:
            issues.append(Issue("info", "unused", f"'{fl}' is not used by any package in this image; it is stored for later builds only.", fl))
    # de-duplicate
    uniq, keys = [], set()
    for i in issues:
        k = (i.code, i.text)
        if k not in keys:
            keys.add(k)
            uniq.append(i)
    return uniq


def _usedep_violation(tok, parent_use, target_use, target_iuse):
    """Return (flag, must_be_on) when a [use dep] token is violated, else None."""
    neg_prefix = tok.startswith("!")
    t = tok.lstrip("!")
    if t.endswith("?"):
        f = t[:-1]
        if f not in target_iuse:
            return None
        if neg_prefix:                  # [!flag?] : target flag on unless parent flag on  -> if parent off target must be on
            return (f, True) if (f not in parent_use and f not in target_use) else None
        return (f, True) if (f in parent_use and f not in target_use) else None   # [flag?]
    if t.endswith("="):
        f = t[:-1]
        if f not in target_iuse:
            return None
        want = (f in parent_use) != neg_prefix
        return (f, want) if (f in target_use) != want else None
    want_on = not t.startswith("-")
    f = t.lstrip("-")
    if f not in target_iuse:
        return None
    return (f, want_on) if (f in target_use) != want_on else None


def has_errors(issues):
    return any(i.level == "error" for i in issues)


# --------------------------------------------------------------------------
# binpkg vs source
# --------------------------------------------------------------------------
def build_minutes(pk, size_bytes):
    for name, m in BUILD_MIN.items():
        if pk == name or pk.startswith(name):
            return m
    return max(1, round(size_bytes / 1e6 * 0.35))


def impact(meta, choices):
    changed = meta.changed_pkgs(choices)
    total = len(meta.pkgs)
    mins, disk, rows = 0, 0, []
    for pk in sorted(changed):
        size = meta.pkgs[pk].get("size", 0)
        m = build_minutes(pk, size)
        mins += m
        disk += max(200, int(size / 1e6 * 4))
        rows.append((pk, m))
    return {"packages": len(changed), "binary": total - len(changed), "total": total, "minutes": mins,
            "disk_mb": disk, "list": rows}


def package_use_text(choices):
    """Content of /etc/portage/package.use/00-live-installer for the target."""
    lines = ["# written by live-installer (USE page); remove to return to the image defaults"]
    for k in sorted(choices):
        pk, _, fl = k.rpartition(":")
        lines.append(f"{pk or '*/*'} {'' if choices[k] else '-'}{fl}")
    return "\n".join(lines) + "\n"


def summary_line(meta, choices):
    imp = impact(meta, choices)
    if not imp["packages"]:
        return "No rebuild needed: every package stays a pre-built binary."
    return (f"{imp['packages']} of {imp['total']} packages must be rebuilt from source "
            f"(about {imp['minutes']} min on 4 cores, ~{imp['disk_mb']} MB of build space); "
            f"{imp['binary']} stay binary.")
