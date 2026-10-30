"""bsdinstall-style install stages shared by the TUI, the GUI and the answer-file runner.

Every stage is a small unit with
  * fields(ctx)  - declarative form description (rendered by curses AND GTK; no UI code here)
  * commit(ctx, values) - copy the form values into the config
  * plan(ctx)    - PURE: list of human readable actions that apply() will perform
  * apply(inst)  - perform them on the target (Installer, see system.py)

Stage ids (UI order): keymap, hostname, disk, extract, rootpass, desktop (only with a hardware profile
that ships a desktop), netconfig, services, hardening, use_flags, finish.  `extract` and `finish` have no form.  Disk and extract run first when installing
(APPLY_ORDER); everything else writes into the extracted system.

The install type (Server by default; Workstation is reserved for a future desktop profile) is part of
the first stage and selects stage defaults through TYPE_DEFAULTS: a Workstation will keep the GUI
stack, skip the server sysctl lock-down and use the desktop USE preset.
"""
import re

from . import portableswap
from . import system as S

ERASE_WORD = "ERASE"

INSTALL_TYPES = [("server", "Server (default)"), ("workstation", "Workstation (coming soon)")]
AVAILABLE_TYPES = ("server",)

# what differs per install type; a future Workstation only has to fill this in
TYPE_DEFAULTS = {
    "server": {"sysctl_hardening": True, "keep_gui": False, "use_preset": "recommended"},
    "workstation": {"sysctl_hardening": False, "keep_gui": True, "use_preset": "recommended"},
}


class Field:
    """kind: text | password | choice | list | check | label | info | multi

    advanced=True fields live under 'Advanced options' (GUI expander, TUI toggle)."""

    def __init__(self, key, label, kind="text", value="", options=None, validate=None, hint="", width=38,
                 visible=None, optional=False, advanced=False, disabled=()):
        self.key, self.label, self.kind = key, label, kind
        self.value = value
        self.options = options or []      # choice/list/multi: [(value, text)]
        self.validate = validate
        self.hint = hint
        self.width = width
        self.visible = visible            # callable(values)->bool
        self.optional = optional
        self.advanced = advanced
        self.disabled = tuple(disabled)   # option values that are shown but cannot be picked

    def text_of(self):
        if self.kind == "password":
            return "*" * len(self.value)
        if self.kind == "check":
            return "[x]" if self.value else "[ ]"
        if self.kind in ("choice", "list"):
            for v, t in self.options:
                if v == self.value:
                    return t
            return str(self.value)
        if self.kind == "multi":
            return ", ".join(t for v, t in self.options if v in self.value) or "(none)"
        return str(self.value)


class Ctx:
    """Everything a stage may look at: profile, config and (injectable) hardware probes."""

    def __init__(self, profile, cfg, nics=None, disks=None, dry=False, efi=None, ram_mb=None):
        self.p, self.c = profile, cfg
        self.nics = nics if nics is not None else []
        self.disks = disks if disks is not None else []
        self.dry = dry
        self.efi = S.efi_boot() if efi is None else efi
        self.ram_mb = S.mem_mb() if ram_mb is None else ram_mb
        self.use_meta = None              # useflags.Meta, loaded lazily by the use_flags stage

    @property
    def wanlan(self):
        return self.p["network_mode"] == "wan-lan"


# --------------------------------------------------------------------------
# pure helpers shared by plan(), the GUI layout preview and Installer.partition()
# --------------------------------------------------------------------------
def swap_plan(cfg, profile, ram_mb):
    """what portableswap would do on this machine: {mode: none|hibernate, mb, why}; servers get no swap, portables a hibernate swap"""
    try:
        hp, _hm = S.hwprofile.detect()
    except Exception:  # noqa
        hp = None
    p = portableswap.plan(cfg, ram_mb, hp)
    if p["mode"] == "hibernate" and p.get("requested") == "auto" and profile.get("portable_swap") is False:
        p = dict(p, mode="none", mb=0, why="this product profile sets portable_swap = false (appliance)")
    return p


def swap_field(cfg, profile, ram_mb):
    """the swap choice, shown with what 'auto' would do on THIS machine (battery / DMI chassis type)"""
    auto = swap_plan(dict(cfg, swap="auto"), profile, ram_mb)
    opts = [("auto", "Automatic: " + portableswap.describe(auto)),
            ("hibernate", f"Hibernate swap, {portableswap.fmt_gib(portableswap.swap_mb(ram_mb))} (laptop, tablet, desktop)"),
            ("none", "No swap (servers)")]
    return Field("swap", "Swap / hibernate", "choice", cfg.get("swap", "auto"), opts, visible=lambda v: v.get("fs") != "zfs" or v.get("enc"), advanced=True,
                 hint="Portables get an encrypted hibernate swap (RAM x 1.125) so sleep survives a flat battery; servers get none. zram stays the first swap")


def disk_layout(cfg, profile, size_bytes, efi, ram_mb):
    """Partition table the installer will write, as [(name, bytes, fs, mountpoint)]; root takes the rest."""
    MB = 1024 * 1024
    segs = []
    zfs = cfg["fs"] == "zfs"
    segs.append(("ESP", (1024 if zfs else 512) * MB, "vfat", "/boot/efi") if efi else ("BIOS boot", 1 * MB, "-", ""))
    segs.append(("boot", (2048 if zfs else 1024) * MB, "ext4", "/boot"))
    if cfg["fs"] == "xfs-luks":                        # encrypted root takes the rest, zram instead of a swap partition
        used = sum(s[1] for s in segs)
        segs.append(("root (LUKS2)", max(0, size_bytes - used - 2 * MB), "xfs in LUKS2", "/"))
        return segs
    sp = swap_plan(cfg, profile, ram_mb) if (cfg["fs"] != "zfs" or cfg.get("zfs_encrypt")) else {"mode": "none", "mb": 0}
    if sp["mode"] == "hibernate":
        segs.append(("swap", sp["mb"] * MB, "swap", "[swap]"))
    used = sum(s[1] for s in segs)
    segs.append(("root", max(0, size_bytes - used - 2 * MB), cfg["fs"], "/"))
    return segs


def kernel_cmdline_arg(cmdline, name, default=""):
    m = re.search(rf"(?:^|\s){re.escape(name)}=(\S+)", cmdline)
    return m.group(1) if m else default


# --------------------------------------------------------------------------
# the stages
# --------------------------------------------------------------------------
class Stage:
    id = ""
    title = ""
    short = ""            # sidebar label
    intro = ""
    ui = "form"           # form | custom | none

    def applies(self, ctx):
        return True

    def fields(self, ctx):
        return []

    def commit(self, ctx, v):
        pass

    def validate(self, ctx, v):
        return None

    def plan(self, ctx):
        return []

    def apply(self, inst):
        getattr(inst, "stage_" + self.id)()


class KeymapStage(Stage):
    id, title, short = "keymap", "Language, keyboard and time zone", "Keyboard"
    intro = "Choose the install type, then how the installed system should speak and tell time."

    def fields(self, ctx):
        c = ctx.c
        return [
            Field("install_type", "Install type", "choice", c["install_type"], INSTALL_TYPES,
                  disabled=[v for v, _t in INSTALL_TYPES if v not in AVAILABLE_TYPES],
                  hint="Server: headless appliance, hardened defaults. Workstation (desktop) is not available yet."),
            Field("locale", "Language", "list", c["locale"], S.LOCALES,
                  hint="Language of the installed system (the installer itself is English)"),
            Field("keymap", "Keyboard layout", "list", c["keymap"], [(k, k) for k in S.keymaps()]),
            Field("timezone", "Time zone", "list", c["timezone"], [(z, z) for z in S.timezones()],
                  hint="Enter opens a list; type to filter"),
        ]

    def validate(self, ctx, v):
        if v["install_type"] not in AVAILABLE_TYPES:
            return "The Workstation install type is not available yet; choose Server."
        return None

    def commit(self, ctx, v):
        ctx.c.update({k: v[k] for k in ("install_type", "locale", "keymap", "timezone")})
        from . import desktop
        if desktop.available():          # the Desktop image variant installs the desktop profile (user page follows)
            ctx.c["install_type"] = "desktop"

    def plan(self, ctx):
        c = ctx.c
        return [f"Install type: {c['install_type']}; locale {c['locale']}, keyboard {c['keymap']}, time zone {c['timezone']}"]


class HostnameStage(Stage):
    id, title, short = "hostname", "Host name", "Host name"
    intro = "Name this machine. A fully qualified name (node1.example.com) is preferred."

    def fields(self, ctx):
        return [Field("hostname", "Hostname (FQDN)", "text", ctx.c["hostname"],
                      validate=lambda v, a: S.hostname_problem(v.strip().lower()), width=40)]

    def commit(self, ctx, v):
        ctx.c["hostname"] = v["hostname"].strip().lower()

    def plan(self, ctx):
        return [f"Set the host name to {ctx.c['hostname']}"]


class DiskStage(Stage):
    id, title, short = "disk", "File system and target disk", "Disk"
    ui = "custom"
    intro = "Choose the file system and the disk(s) to install to. Everything on a selected disk will be ERASED."

    def fields(self, ctx):
        """The file-system part (the disk list itself is a custom widget)."""
        c, p = ctx.c, ctx.p
        opts = {"ext4": "ext4 (manual choice)", "xfs": "XFS", "zfs": "ZFS (checksums, snapshots, boot environments, mirror)",
                "xfs-luks": "XFS in an encrypted LUKS2 volume (the fallback when ZFS is unsuitable)"}
        order = ("zfs", "xfs-luks", "xfs", "ext4")      # ext4 last: never preselected
        fss = sorted(p["filesystems"], key=lambda x: order.index(x) if x in order else 9)
        why = c.get("fs_why") or ""
        luks = lambda v: v.get("fs") == "xfs-luks"  # noqa: E731
        return [Field("fs", "Root file system", "choice", c["fs"], [(x, opts.get(x, x)) for x in fss],
                      hint=("Default: " + why + ". " if why else "") + "ext4 and XFS use one disk. ZFS uses the whole disk as a pool 'rpool'."),
                Field("zfs_raid", "ZFS layout", "choice", c["zfs_raid"],
                      [("single", "single disk"), ("mirror", "mirror (RAID1, two disks)")],
                      visible=lambda v: v.get("fs") == "zfs", advanced=True,
                      hint="A mirror needs two disks and survives one failing; /boot lives on the first disk only."),
                Field("enc", "Encrypt the disk (ZFS native, AES-256-GCM)", "check", c.get("zfs_encrypt", False), visible=lambda v: v.get("fs") == "zfs",
                      hint="Recommended for laptops. The passphrase is asked at every boot; /boot and the EFI partition stay unencrypted"),
                Field("pp", "Disk passphrase", "password", c.get("zfs_passphrase", ""), width=40, visible=lambda v: v.get("fs") == "zfs" and v.get("enc"),
                      hint="Min 12 characters. NOT recoverable: if you forget it the data is gone"),
                Field("pp2", "Confirm passphrase", "password", c.get("zfs_passphrase", ""), width=40, visible=lambda v: v.get("fs") == "zfs" and v.get("enc")),
                Field("lpw", "Disk passphrase", "password", c.get("luks_passphrase", ""), width=40, visible=luks,
                      hint="Typed at every boot (the Type Cover / a USB keyboard works). Min 12 chars. Lose it = lose the data"),
                Field("lpw2", "Confirm passphrase", "password", c.get("luks_passphrase", ""), width=40, visible=luks),
                swap_field(c, p, ctx.ram_mb)]

    def validate(self, ctx, v):
        if v.get("fs") == "xfs-luks":
            if v.get("lpw") != v.get("lpw2"):
                return "The two passphrases do not match."
            if len(v.get("lpw") or "") < 12:
                return "The disk passphrase needs at least 12 characters."
        if v.get("fs") == "zfs" and v.get("enc"):
            if v.get("pp") != v.get("pp2"):
                return "The two passphrases do not match."
            from . import zfsroot
            return zfsroot.passphrase_problem(v.get("pp") or "")
        return None

    def commit(self, ctx, v):
        ctx.c["fs"], ctx.c["zfs_raid"] = v["fs"], v["zfs_raid"]
        ctx.c["luks_passphrase"] = v.get("lpw", "") if v["fs"] == "xfs-luks" else ""
        ctx.c["zfs_encrypt"] = bool(v["fs"] == "zfs" and v.get("enc"))
        ctx.c["zfs_passphrase"] = v.get("pp", "") if ctx.c["zfs_encrypt"] else ""
        ctx.c["swap"] = v.get("swap", "auto") if (v["fs"] != "zfs" or ctx.c["zfs_encrypt"]) else "auto"

    def need_disks(self, c):
        return 2 if (c["fs"] == "zfs" and c["zfs_raid"] == "mirror") else 1

    def check_selection(self, ctx, sel):
        """Return (error, warning) for a disk selection."""
        need = self.need_disks(ctx.c)
        if len(sel) != need:
            return f"Select exactly {need} disk{'s' if need > 1 else ''} (selected: {len(sel)}).", None
        by = {d["path"]: d for d in ctx.disks}
        small = [by[x] for x in sel if x in by and by[x]["size"] < 8 * 10**9]
        if small:
            return f"{small[0]['path']} is smaller than 8 GB; the installed system needs more room.", None
        warn = None
        if need == 2 and all(x in by for x in sel):
            a, b = by[sel[0]]["size"], by[sel[1]]["size"]
            if abs(a - b) > 0.1 * max(a, b):
                warn = "The disks differ in size by more than 10%; the mirror is limited to the smaller one."
        return None, warn

    def plan(self, ctx):
        c = ctx.c
        out = []
        by = {d["path"]: d for d in ctx.disks}
        for disk in c["disks"]:
            size = by.get(disk, {}).get("size", 64 * 10**9)
            segs = disk_layout(c, ctx.p, size, ctx.efi, ctx.ram_mb)
            out.append(f"ERASE {disk} and create: " + ", ".join(f"{n} {S.fmt_size(b)}" for n, b, _f, _m in segs))
        if c["fs"] != "zfs" or c.get("zfs_encrypt"):
            out.append("Swap: " + portableswap.describe(swap_plan(c, ctx.p, ctx.ram_mb)))
        if c["fs"] == "xfs-luks":
            out.append("Encrypt the root partition with LUKS2 (argon2id) and create XFS inside it; zram instead of swap")
        elif c["fs"] == "zfs":
            out.append("Create ZFS pool rpool" + (" (mirror)" if c["zfs_raid"] == "mirror" else "") + " with lz4 compression"
                       + (", natively ENCRYPTED (AES-256-GCM, passphrase at every boot)" if c.get("zfs_encrypt") else ""))
        else:
            out.append(f"Create {c['fs']} file systems and mount them under /mnt/target")
        return out


class ExtractStage(Stage):
    id, title, short = "extract", "Copy the system", "Install"
    ui = "none"

    def plan(self, ctx):
        return ["Copy the system image from the installation medium to the new root file system"]


class RootpassStage(Stage):
    id, title, short = "rootpass", "Administrator account", "Password"
    intro = ("Set the administrator password (REQUIRED - there is no default password). "
             "It is used for the console root login and the web admin user.")

    def fields(self, ctx):
        c = ctx.c
        return [
            Field("pw", "Password", "password", c["root_password"], width=40,
                  hint="At least 12 characters, mixed. Used for console root and web admin"),
            Field("pw2", "Confirm password", "password", c["root_password"], width=40),
            Field("strength", "Strength", "info", lambda a: S.password_strength(a["pw"]) if a["pw"] else "-"),
            Field("email", "E-mail (notifications)", "text", c["email"], validate=lambda v, a: S.email_problem(v), width=40,
                  hint="Alerts and notices are addressed here"),
            Field("key", "SSH public key", "text", "\n".join(c["ssh_keys"]), optional=True, width=40, advanced=True,
                  validate=lambda v, a: S.ssh_key_problem(v), hint="Optional: paste one ssh-ed25519/ssh-rsa line for root key login"),
        ]

    def validate(self, ctx, v):
        if v["pw"] != v["pw2"]:
            return "The two passwords do not match."
        return S.password_problem(v["pw"], ctx.c["hostname"])

    def commit(self, ctx, v):
        c = ctx.c
        c["root_password"], c["web_password"] = v["pw"], v["pw"]
        c["root_hash"] = ""
        c["email"] = v["email"].strip()
        c["ssh_keys"] = [v["key"].strip()] if v["key"].strip() else []

    def plan(self, ctx):
        c = ctx.c
        who = "password hash from the answer file" if c["root_hash"] else "the password you entered"
        return [f"Set the root and web admin password ({who}); no default password exists",
                "Install " + (f"{len(c['ssh_keys'])} SSH key(s) for root" if c["ssh_keys"] else "no SSH key (console login only)")]


class NetconfigStage(Stage):
    id, title, short = "netconfig", "Network", "Network"
    ui = "custom"
    intro = "Pick the network interface(s) and addressing. Link state is read live."

    def plan(self, ctx):
        c = ctx.c
        out = []
        if ctx.wanlan:
            w, l = c["wan"], c["lan"]
            out.append(f"WAN {w['iface'] or '-'}: " + (w["address"] if w["mode"] == "static" else "DHCP") + (" (Wi-Fi)" if c["wifi"] else ""))
            out.append(f"LAN {l['iface'] or '-'}: {l['address'] if l['iface'] else '-'}")
        else:
            n = c["mgmt"]
            out.append(f"Management NIC {n['iface'] or '-'}: " + (f"{n['address']} gw {n['gateway']} dns {n['dns']}" if n["mode"] == "static" else "DHCP")
                       + (" (Wi-Fi)" if c["wifi"] else ""))
        return out


class ServicesStage(Stage):
    id, title, short = "services", "Services", "Services"
    intro = "Choose which optional services start at boot. Required services are always enabled."

    def optional(self, p):
        return [s for s in p.get("services_optional", []) if s in p["services"]]

    def required(self, p):
        return [s for s in p["services"] if s not in self.optional(p) and s != "sshd"]

    def fields(self, ctx):
        p, c = ctx.p, ctx.c
        info = p.get("service_info", {})
        opts = [(s, f"{s} - {info.get(s, '')}".rstrip(" -")) for s in self.optional(p)]
        f = [Field("req", "Always on", "info", ", ".join(self.required(p)) or "-")]
        if opts:
            f.append(Field("opt", "Optional services", "multi", [s for s in self.optional(p) if s not in c["services_off"]], opts,
                           hint="Space toggles the highlighted service (Left/Right moves)"))
        else:
            f.append(Field("none", "There are no optional services for this product.", "label"))
        return f

    def commit(self, ctx, v):
        if "opt" in v:
            ctx.c["services_off"] = [s for s in self.optional(ctx.p) if s not in v["opt"]]

    def plan(self, ctx):
        on = [s for s in ctx.p["services"] if s not in ctx.c["services_off"] and s != "sshd"]
        out = ["Enable at boot: " + ", ".join(on)]
        if ctx.c["services_off"]:
            out.append("Left disabled: " + ", ".join(ctx.c["services_off"]))
        return out


class HardeningStage(Stage):
    id, title, short = "hardening", "Security hardening", "Hardening"
    intro = ("Secure defaults are pre-selected. Anything that weakens security is OFF and marked as a risk. "
             "Nothing here locks you out of the console.")

    def fields(self, ctx):
        c, p = ctx.c, ctx.p
        return [
            Field("ssh_enabled", "Enable the SSH server", "check", c["ssh_enabled"],
                  hint="Key-only root login unless password login is enabled below"),
            Field("sysctl", "Kernel and network hardening (sysctl)", "check", c["sysctl_hardening"],
                  hint="Restricts kernel pointers/dmesg/BPF, disables ICMP redirects, enables SYN cookies"),
            Field("pwauth", "Allow SSH password login", "check", c["ssh_password_auth"], advanced=True,
                  hint="OFF is recommended: root can then log in over SSH with a key only",
                  visible=lambda v: v.get("ssh_enabled", True)),
            Field("warn", "WARNING: password SSH login exposes the system to password guessing from the network.", "label", "",
                  hint="warn", visible=lambda v: v.get("pwauth") and v.get("ssh_enabled", True)),
            Field("conopen", "Open console menu (no login on the console)", "check", c["console_open"], advanced=True,
                  hint="OFF (default): the console asks for the root password first. ON = OPNsense style: anyone at the console can use the menu"),
            Field("warn2", "WARNING: anyone with physical or VM-console access can then reset the admin password.", "label", "",
                  hint="warn", visible=lambda v: v.get("conopen")),
        ]

    def commit(self, ctx, v):
        c = ctx.c
        c["ssh_enabled"] = bool(v["ssh_enabled"])
        c["sysctl_hardening"] = bool(v["sysctl"])
        c["ssh_password_auth"] = bool(v["pwauth"]) and c["ssh_enabled"]
        c["console_open"] = bool(v["conopen"])

    def plan(self, ctx):
        c = ctx.c
        out = []
        if c["ssh_enabled"]:
            out.append("SSH on: " + ("PASSWORD login ON (risk)" if c["ssh_password_auth"] else "key-only, no password login"))
        else:
            out.append("SSH server disabled")
        out.append("Kernel/network sysctl hardening: " + ("on" if c["sysctl_hardening"] else "off"))
        out.append("Console menu: " + ("OPEN (no login, risk)" if c["console_open"] else "after root login"))
        return out


class UseFlagsStage(Stage):
    id, title, short = "use_flags", "Software (USE flags)", "Software"
    ui = "custom"
    intro = "Choose a preset or tune individual Portage USE flags. The Portage sanity check runs live."

    def plan(self, ctx):
        from . import useflags
        c = ctx.c
        u = c["use"]
        out = [f"USE flag preset: {u['preset']}"]
        if u["flags"]:
            out.append(f"{len(u['flags'])} flag override(s): " + " ".join(("" if on else "-") + f for f, on in sorted(u["flags"].items())[:12])
                       + (" ..." if len(u["flags"]) > 12 else ""))
            meta = ctx.use_meta or useflags.load_meta()
            if meta:
                imp = useflags.impact(meta, useflags.effective_choices(meta, ctx.p, u))
                out.append(f"Writes /etc/portage/package.use; {imp['packages']} installed package(s) need a source rebuild "
                           f"(~{imp['minutes']} min, ~{imp['disk_mb']} MB build space) - done later, the system boots from binaries now")
        else:
            out.append("Image defaults (no USE changes): every package stays a pre-built binary")
        return out


class FinishStage(Stage):
    id, title, short = "finish", "Boot loader and finish", "Finish"
    ui = "none"

    def plan(self, ctx):
        return ["Install the boot loader (" + ("UEFI" if ctx.efi else "BIOS") + ") and unmount"]


class DesktopStage(Stage):
    id, title, short = "desktop", "Desktop user", "Desktop"
    intro = "A touch-friendly desktop (sway, on-screen keyboard, pen) is installed for this machine."

    def applies(self, ctx):
        """only on a machine with a hardware profile that ships a desktop (Surface): user + autologin"""
        try:
            hp, _hm = S.hwprofile.detect()
        except Exception:  # noqa
            return False
        return bool(hp and hp.get("defaults", {}).get("desktop"))

    def fields(self, ctx):
        d = ctx.c["desktop"]
        return [Field("user", "User name", "text", d.get("user") or "surface", width=30,
                      validate=lambda v, a: None if re.fullmatch(r"[a-z_][a-z0-9_-]{0,30}", v or "") else "lower-case letters, digits, - and _"),
                Field("pw", "Password", "password", d.get("password", ""), width=40, hint="Min 12 chars. Needed to log in and for sudo"),
                Field("pw2", "Confirm password", "password", d.get("password", ""), width=40),
                Field("auto", "Log in automatically on the console", "check", d.get("autologin", False),
                      hint="OFF (default): the Type Cover / on-screen keyboard login prompt is shown. ON: the desktop starts right after the disk passphrase")]

    def validate(self, ctx, v):
        if v["pw"] != v["pw2"]:
            return "The two passwords do not match."
        return S.password_problem(v["pw"], ctx.c["hostname"])

    def commit(self, ctx, v):
        ctx.c["desktop"].update({"enable": True, "user": v["user"], "password": v["pw"], "autologin": bool(v["auto"])})

    def plan(self, ctx):
        d = ctx.c["desktop"]
        return [f"Create the desktop user {d.get('user') or '(not set)'}" + (" with automatic login" if d.get("autologin") else "")]

    def apply(self, inst):
        pass            # written by the hardware profile (hwprofile.apply_profile) while the finish stage configures the system


class T2Stage(Stage):
    """Apple T2 Mac (PCI 106b:1801): what the profile does and where the Wi-Fi firmware comes from (backend: t2.py, keys t2 / t2_firmware)"""
    id, title, short = "t2", "Apple T2 hardware", "T2 Mac"
    intro = "This Mac has the Apple T2 security chip."

    def applies(self, ctx):
        from . import t2
        want = str(ctx.c.get("t2", "auto")).lower()
        return want != "no" and (want in ("yes", "1", "true", "on") or bool(t2.detect()))

    def fields(self, ctx):
        from . import t2
        c = ctx.c
        found = [] if getattr(ctx, "dry", False) else t2.find_firmware_sources(["/mnt", "/media", "/run/media", "/root", "/home"])
        opts = [("auto", "Import automatically (first source found)" if found else "No firmware found: skip (add it later)"),
                ("none", "Skip: no Wi-Fi/Bluetooth until I import it")] + [(f["path"], f"{f['kind']}: {f['path']} ({f['apple']} Apple files)") for f in found]
        return [Field("info1", "Apple T2 Mac detected" + (f" ({t2.model()})" if t2.model() else ""), "label", "", hint="warn"),
                Field("info2", "Installs the T2 kernel (apple-bce keyboard in the passphrase prompt), fan control, Touch Bar and pm_async=off.", "label"),
                Field("fw", "Wi-Fi / Bluetooth firmware", "choice", c.get("t2_firmware", "auto"), opts,
                      hint="Apple firmware is never on the ISO. Put firmware.tar on a USB stick or point the installer at an old install")]

    def commit(self, ctx, v):
        ctx.c["t2_firmware"] = v["fw"]

    def plan(self, ctx):
        return [f"Apple T2 profile: T2 kernel, fan control, Touch Bar; Wi-Fi firmware: {ctx.c.get('t2_firmware', 'auto')}"]

    def apply(self, inst):
        pass            # done by the backend (t2.preflight in go(), t2.apply in profile_extras)


class GnomeStage(Stage):
    """the Desktop image variant: a normal user for the GNOME session (backend: desktop.py, keys user_name / user_fullname / user_password / user_autologin)"""
    id, title, short = "gnome", "Desktop user", "Desktop user"
    intro = "The desktop session needs a normal user account (GNOME does not log in as root)."

    def applies(self, ctx):
        from . import desktop
        return desktop.available()

    def fields(self, ctx):
        from . import desktop
        c = ctx.c
        return [Field("user", "User name", "text", c["user_name"], width=30, validate=lambda v, a: desktop.user_problem(v)),
                Field("full", "Full name", "text", c["user_fullname"], width=30, optional=True),
                Field("pw", "Password", "password", c["user_password"], width=40, hint="Min 12 chars, mixed"),
                Field("pw2", "Confirm password", "password", c["user_password"], width=40),
                Field("auto", "Log in automatically", "check", c["user_autologin"], hint="Not recommended on an encrypted laptop that you carry around")]

    def validate(self, ctx, v):
        if v["pw"] != v["pw2"]:
            return "The two passwords do not match."
        return S.password_problem(v["pw"], ctx.c["hostname"])

    def commit(self, ctx, v):
        c = ctx.c
        c["install_type"] = "desktop"
        c["user_name"], c["user_fullname"], c["user_password"] = v["user"], v["full"] or v["user"], v["pw"]
        c["user_autologin"] = bool(v["auto"])

    def plan(self, ctx):
        c = ctx.c
        return [f"Desktop image: GNOME session for the user {c.get('user_name') or '(not set)'}" + (" with automatic login" if c.get("user_autologin") else "")]

    def apply(self, inst):
        pass            # desktop.apply runs from Installer.profile_extras


STAGES = [KeymapStage(), HostnameStage(), DiskStage(), ExtractStage(), RootpassStage(), DesktopStage(), T2Stage(), GnomeStage(), NetconfigStage(),
          ServicesStage(), HardeningStage(), UseFlagsStage(), FinishStage()]
BY_ID = {s.id: s for s in STAGES}
UI_ORDER = [s.id for s in STAGES]
# disks must exist and be extracted before anything is written into the new system
APPLY_ORDER = ["disk", "extract", "keymap", "hostname", "rootpass", "desktop", "t2", "gnome", "netconfig", "services", "hardening", "use_flags", "finish"]


def stage_list(ctx, ui_only=False):
    out = []
    for s in STAGES:
        if not s.applies(ctx):
            continue
        if ui_only and s.ui == "none":
            continue
        out.append(s)
    return out


def full_plan(ctx):
    """[(stage id, [actions])] in apply order: the same data the review page, `--plan` and the tests use."""
    return [(i, BY_ID[i].plan(ctx)) for i in APPLY_ORDER if BY_ID[i].applies(ctx)]


def run_stages(inst, only=None):
    """Apply every stage in sequence (Installer calls this; TUI, GUI and answer file all go through it)."""
    done = []
    for i in APPLY_ORDER:
        if only and i not in only:
            continue
        st = BY_ID[i]
        if not st.applies(inst.ctx()):
            continue
        inst.log(f"== stage: {st.id} ({st.title})")
        st.apply(inst)
        done.append(i)
    return done


def form_values(fields):
    return {f.key: f.value for f in fields}


def visible_fields(fields, values=None):
    vals = values if values is not None else form_values(fields)
    return [f for f in fields if f.visible is None or f.visible(vals)]


def first_problem(stage, ctx, fields):
    """Run every field validator + the stage validator exactly like the 'Next' button does. -> (field|None, message)|None"""
    vals = form_values(fields)
    vis = visible_fields(fields, vals)
    for f in vis:
        if f.kind in ("label", "info"):
            continue
        if f.validate and not (f.optional and not f.value):
            e = f.validate(f.value, {x.key: x.value for x in vis})
            if e:
                return f, e
    e = stage.validate(ctx, {x.key: x.value for x in vis})
    return (None, e) if e else None
