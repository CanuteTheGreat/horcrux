"""Guided installer: screens, unattended runner and entry point."""
import curses
import os
import sys
import threading
import time

from . import system as S
from . import tui
from .tui import Field, Abort, run_form, run_disk_picker, message, progress_screen

ERASE_WORD = "ERASE"


class State:
    def __init__(self, profile):
        self.p = profile
        self.c = S.default_config(profile)
        self.nics = []
        self.dry = False


# --------------------------------------------------------------------------
# screens - each returns "next" or "back"
# --------------------------------------------------------------------------
def s_locale(t, st, step):
    c = st.c
    f = [Field("locale", "Language", "list", c["locale"], S.LOCALES, hint="Language of the installed system (the installer itself is English)"),
         Field("keymap", "Keyboard layout", "list", c["keymap"], [(k, k) for k in S.keymaps()]),
         Field("timezone", "Time zone", "list", c["timezone"], [(z, z) for z in S.timezones()], hint="Enter opens a list; type to filter")]
    b, v = run_form(t, step, "Language, keyboard and time zone",
                    "Choose how the installed system should speak and tell time.", f, buttons=("Abort", "Next"))
    if b == "Abort":
        raise Abort()
    c.update(v)
    S.apply_live_keymap(c["keymap"])
    return "next"


def s_fs(t, st, step):
    c, p = st.c, st.p
    opts = {"ext4": "ext4 (robust)", "xfs": "XFS", "zfs": "ZFS (checksums, snapshots, mirror)",
            "xfs-luks": "XFS in an encrypted LUKS2 volume (recommended for < 8 GB RAM)"}
    f = [Field("fs", "Root file system", "choice", c["fs"], [(x, opts.get(x, x)) for x in p["filesystems"]]),
         Field("zfs_raid", "ZFS layout", "choice", c["zfs_raid"], [("single", "single disk"), ("mirror", "mirror (RAID1, two disks)")],
               visible=lambda v: v["fs"] == "zfs"),
         Field("lpw", "Disk passphrase", "password", c.get("luks_passphrase", ""), width=40, visible=lambda v: v["fs"] == "xfs-luks",
               hint="Typed at every boot (the Type Cover / a USB keyboard works). Min 12 chars. Lose it = lose the data"),
         Field("lpw2", "Confirm passphrase", "password", c.get("luks_passphrase", ""), width=40, visible=lambda v: v["fs"] == "xfs-luks")]

    def check(a):
        if a["fs"] == "xfs-luks":
            if a["lpw"] != a["lpw2"]:
                return "The two passphrases do not match."
            if len(a["lpw"]) < 12:
                return "The disk passphrase needs at least 12 characters."
        return None
    b, v = run_form(t, step, "File system",
                    "ext4 and XFS use one disk. XFS+LUKS encrypts everything but /boot and uses zram instead of a swap partition. "
                    "ZFS uses the whole disk as a pool 'rpool'; a mirror needs two disks "
                    "(it survives one disk failing; /boot lives on the first disk only).", f, validate_all=check)
    if b == "Back":
        return "back"
    c["fs"], c["zfs_raid"] = v["fs"], v["zfs_raid"]
    c["luks_passphrase"] = v["lpw"] if c["fs"] == "xfs-luks" else ""
    if c["fs"] == "zfs" and not st.dry and S.sh("modprobe zfs && test -e /dev/zfs")[0] != 0:
        message(t, "ZFS not available", "The ZFS kernel module could not be loaded on this machine. Choose ext4 or XFS.", "err")
        return "again"
    return "next"


def s_desktop(t, st, step):
    """only on a machine with a hardware profile that ships a desktop (Surface): user + autologin"""
    c = st.c
    d = c["desktop"]
    f = [Field("user", "User name", "text", d.get("user") or "surface", width=30, validate=lambda v, a: None if S.re.fullmatch(r"[a-z_][a-z0-9_-]{0,30}", v or "") else "lower-case letters, digits, - and _"),
         Field("pw", "Password", "password", d.get("password", ""), width=40, hint="Min 12 chars. Needed to log in and for sudo"),
         Field("pw2", "Confirm password", "password", d.get("password", ""), width=40),
         Field("auto", "Log in automatically on the console", "check", d.get("autologin", False),
               hint="OFF (default): the Type Cover / on-screen keyboard login prompt is shown. ON: the desktop starts right after the disk passphrase")]

    def check(a):
        if a["pw"] != a["pw2"]:
            return "The two passwords do not match."
        return S.password_problem(a["pw"], c["hostname"])
    b, v = run_form(t, step, "Desktop user", "A touch-friendly desktop (sway, on-screen keyboard, pen) is installed for this machine.", f, validate_all=check)
    if b == "Back":
        return "back"
    d.update({"enable": True, "user": v["user"], "password": v["pw"], "autologin": bool(v["auto"])})
    return "next"


def s_disk(t, st, step):
    c = st.c
    disks = S.list_disks()
    if not disks:
        message(t, "No disk found", "No usable internal disk (>= 2 GB) was found. If the disk is behind a RAID/VMD "
                "controller, switch the firmware to AHCI mode. The installer will exit to a shell.", "err")
        raise Abort()
    mirror = c["fs"] == "zfs" and c["zfs_raid"] == "mirror"
    intro = ("Select the TWO disks for the mirror." if mirror else "Select the disk to install to.") + \
            " Everything on a selected disk will be ERASED. The live USB stick is not listed."
    sel = [d for d in c["disks"] if any(x["path"] == d for x in disks)]
    while True:
        b, sel = run_disk_picker(t, step, "Target disk" + ("s" if mirror else ""), intro, disks, sel, mirror, S.fmt_size)
        if b == "Back":
            return "back"
        need = 2 if mirror else 1
        if len(sel) != need:
            message(t, "Disk selection", f"Select exactly {need} disk{'s' if need > 1 else ''} (selected: {len(sel)}).", "err")
            continue
        if mirror:
            sizes = [next(d["size"] for d in disks if d["path"] == x) for x in sel]
            if abs(sizes[0] - sizes[1]) > 0.1 * max(sizes):
                if message(t, "Different disk sizes", "The disks differ in size by more than 10%; the mirror is limited to the smaller one.",
                           "warn", ("Back", "Continue")) != "Continue":
                    continue
        small = [d for d in disks if d["path"] in sel and d["size"] < 8 * 10**9]
        if small:
            message(t, "Disk too small", f"{small[0]['path']} is smaller than 8 GB; the installed system needs more room.", "err")
            continue
        c["disks"] = sel
        return "next"


def nic_options(st, wired_only=False):
    out = []
    for n in st.nics:
        if wired_only and n["wifi"]:
            continue
        out.append((n["name"], S.nic_line(n)))
    return out


def default_gateway():
    rc, out = S.sh("ip -4 route show default")
    import re
    m = re.search(r"default via (\S+)", out)
    return m.group(1) if m else ""


def default_dns():
    for l in S.read("/etc/resolv.conf").splitlines():
        if l.startswith("nameserver"):
            return l.split()[1]
    return ""


def s_mgmt(t, st, step):
    c = st.c
    n = c["mgmt"]
    nics = nic_options(st)
    if not nics:
        message(t, "No network interface", "No network interface was found. You can still install; the web UI will be reachable once "
                "you add a NIC (USB Ethernet works) or Wi-Fi. Continue without networking?", "warn", ("Continue",))
        n["iface"] = ""
        return "next"
    if not n["iface"]:
        # prefer a wired NIC with a link, then any wired, then Wi-Fi
        wired = [x for x in st.nics if not x["wifi"]]
        up = [x for x in wired if S.carrier(x["name"])]
        n["iface"] = (up or wired or st.nics)[0]["name"]
    cur_ip = S.nic_ip(n["iface"])
    f = [Field("iface", "Management NIC", "list", n["iface"], nics, width=60, hint="The interface that carries the web UI and SSH"),
         Field("hostname", "Hostname (FQDN)", "text", c["hostname"], validate=lambda v, a: S.hostname_problem(v), width=40),
         Field("mode", "IP configuration", "choice", n["mode"], [("dhcp", "DHCP (automatic)"), ("static", "Static address")]),
         Field("address", "IP address / prefix", "text", n["address"] or cur_ip, validate=lambda v, a: S.cidr_problem(v),
               visible=lambda v: v["mode"] == "static", hint="e.g. 192.168.1.50/24"),
         Field("gateway", "Gateway", "text", n["gateway"] or default_gateway(), validate=lambda v, a: S.ip_problem(v, a.get("address")),
               visible=lambda v: v["mode"] == "static"),
         Field("dns", "DNS server(s)", "text", n["dns"] or default_dns(), validate=lambda v, a: S.dns_problem(v),
               visible=lambda v: v["mode"] == "static", hint="space separated")]
    b, v = run_form(t, step, "Management network",
                    "Pick the network interface for the management web UI and SSH, and set the host name.", f)
    if b == "Back":
        return "back"
    c["hostname"] = v["hostname"].strip().lower()
    n.update({k: v[k].strip() if isinstance(v[k], str) else v[k] for k in ("iface", "mode", "address", "gateway", "dns")})
    if next(x for x in st.nics if x["name"] == n["iface"])["wifi"]:
        r = s_wifi(t, st, step, n["iface"])
        if r != "next":
            return "again"
        n["mode"] = "dhcp"
    else:
        c["wifi"] = None
    return "next"


def s_wifi(t, st, step, iface):
    c = st.c
    t.scr.clear()
    t.frame(step, "Wi-Fi")
    t.put(5, 2, f"Scanning for networks on {iface} ...")
    t.scr.refresh()
    nets = [] if st.dry else S.wifi_scan(iface)
    if not nets:
        r = message(t, "No Wi-Fi networks", "No networks were found (or the adapter has no firmware). Enter the SSID by hand?",
                    "warn", ("Back", "Enter manually"))
        if r == "Back":
            return "back"
    opts = [(s, f"{s}   [{sec}] {sig}") for s, sec, sig in nets]
    f = []
    if opts:
        f.append(Field("ssid", "Network", "list", opts[0][0], opts, width=50))
    else:
        f.append(Field("ssid", "SSID", "text", "", validate=lambda v, a: None if v else "Enter the network name", width=40))
    f.append(Field("psk", "Passphrase", "password", "", optional=True, hint="leave empty for an open network"))
    while True:
        b, v = run_form(t, step, "Wi-Fi",
                        "The device has no wired NIC (or you chose Wi-Fi). It is used as DHCP client; the passphrase is stored for the installed system.", f)
        if b == "Back":
            return "back"
        if st.dry:
            ok, info = True, "10.0.0.2"
        else:
            t.scr.clear()
            t.frame(step, "Wi-Fi")
            t.put(5, 2, f"Connecting to {v['ssid']} ...")
            t.scr.refresh()
            ok, info = S.wifi_connect(iface, v["ssid"], v["psk"])
        if ok:
            c["wifi"] = {"ssid": v["ssid"], "psk": v["psk"], "iface": iface}
            message(t, "Wi-Fi connected", f"Connected, address {info}.", "ok")
            return "next"
        message(t, "Wi-Fi failed", f"Could not connect: {info}", "err")


def s_ifaces(t, st, step):
    """Patronus: WAN/LAN assignment with link detection (like OPNsense option 1)."""
    c = st.c
    nics = nic_options(st)
    if not nics:
        message(t, "No network interface", "No network interface found. A firewall needs at least one; plug in a NIC (USB Ethernet works) "
                "and restart the installer.", "err")
        raise Abort()
    wired = [x for x in st.nics if not x["wifi"]]
    S.bring_up(st.nics)
    time.sleep(1.5)
    auto = ("__auto__", "Auto-detect: unplug/replug that cable")
    none = ("", "(none)")
    wan_def = c["wan"]["iface"] or (wired[0]["name"] if wired else st.nics[0]["name"])
    lan_def = c["lan"]["iface"] or (wired[1]["name"] if len(wired) > 1 else "")
    f = [Field("wan", "WAN interface", "list", wan_def, nics + [auto], width=60, hint="Faces the internet / upstream. Wi-Fi works as WAN on NIC-less devices."),
         Field("lan", "LAN interface", "list", lan_def, [none] + nics + [auto], width=60, hint="Faces your network; the web UI is reachable here")]
    while True:
        b, v = run_form(t, step, "Interface assignment",
                        "Assign the WAN and LAN interfaces. Link state is read live; choose 'Auto-detect' and replug the cable to identify a port.",
                        f, validate_all=lambda a: "WAN and LAN must be different interfaces" if a["wan"] and a["wan"] == a["lan"] and a["wan"] != "__auto__" else None)
        if b == "Back":
            return "back"
        picked = {}
        for role in ("wan", "lan"):
            if v[role] == "__auto__":
                cands = [x["name"] for x in wired if x["name"] not in picked.values()]
                end = [None]

                def tick(left, role=role):
                    t.scr.clear()
                    t.frame(step, "Interface assignment")
                    t.put(5, 2, f"Plug the {role.upper()} cable in (or unplug and replug it) now ...  {left}s")
                    t.scr.refresh()
                tick(45)
                end[0] = S.detect_link_change(cands, 45, tick)
                if not end[0]:
                    message(t, "Nothing detected", "No link change seen. Pick the interface from the list instead.", "warn")
                    break
                picked[role] = end[0]
            else:
                picked[role] = v[role]
        else:
            if picked["wan"] == picked["lan"] and picked["wan"]:
                message(t, "Same interface", "WAN and LAN must be different interfaces.", "err")
                continue
            for k in ("wan", "lan"):
                f[0 if k == "wan" else 1].value = picked[k]
            c["wan"]["iface"], c["lan"]["iface"] = picked["wan"], picked["lan"]
            wan_nic = next(x for x in st.nics if x["name"] == picked["wan"])
            if wan_nic["wifi"]:
                if s_wifi(t, st, step, wan_nic["name"]) != "next":
                    continue
                c["wan"]["mode"] = "dhcp"
            else:
                c["wifi"] = None
            return "next"


def s_addr(t, st, step):
    c = st.c
    w, l = c["wan"], c["lan"]
    wifi = bool(c["wifi"])
    f = [Field("hostname", "Hostname (FQDN)", "text", c["hostname"], validate=lambda v, a: S.hostname_problem(v), width=40),
         Field("wmode", "WAN addressing", "choice", w["mode"], [("dhcp", "DHCP (from the ISP / upstream)"), ("static", "Static address")]),
         Field("waddr", "WAN address/prefix", "text", w["address"], validate=lambda v, a: S.cidr_problem(v), visible=lambda v: v["wmode"] == "static"),
         Field("wgw", "WAN gateway", "text", w["gateway"], validate=lambda v, a: S.ip_problem(v, a.get("waddr")), visible=lambda v: v["wmode"] == "static"),
         Field("wdns", "DNS server(s)", "text", w["dns"], validate=lambda v, a: S.dns_problem(v), visible=lambda v: v["wmode"] == "static")]
    if l["iface"]:
        f.append(Field("laddr", "LAN address/prefix", "text", l["address"], validate=lambda v, a: S.cidr_problem(v),
                       hint="The firewall's own LAN address; also the web UI address"))
    b, v = run_form(t, step, "WAN / LAN addressing",
                    "Set the host name and how the WAN gets its address. The LAN is static (a DHCP server for LAN clients can be enabled later in the web UI)."
                    + (" WAN is Wi-Fi (DHCP)." if wifi else ""), f)
    if b == "Back":
        return "back"
    c["hostname"] = v["hostname"].strip().lower()
    w.update({"mode": "dhcp" if wifi else v["wmode"], "address": v.get("waddr", ""), "gateway": v.get("wgw", ""), "dns": v.get("wdns", "")})
    if l["iface"]:
        l["address"] = v["laddr"].strip()
    return "next"


def s_account(t, st, step):
    c, p = st.c, st.p
    f = [Field("pw", "Password", "password", c["root_password"], width=40,
               hint="Min 12 chars, mixed. Used for console root and web admin"),
         Field("pw2", "Confirm password", "password", c["root_password"], width=40),
         Field("strength", "Strength", "info", lambda a: S.password_strength(a["pw"]) if a["pw"] else "-"),
         Field("email", "E-mail (notifications)", "text", c["email"], validate=lambda v, a: S.email_problem(v), width=40,
               hint="Alerts and notices are addressed here"),
         Field("key", "SSH public key", "text", "\n".join(c["ssh_keys"]), optional=True, width=40,
               validate=lambda v, a: S.ssh_key_problem(v), hint="Optional: paste one ssh-ed25519/ssh-rsa line for root key login"),
         Field("pwauth", "Allow SSH password login", "check", c["ssh_password_auth"],
               hint="OFF is recommended: root can then log in over SSH with a key only"),
         Field("conopen", "Open console menu (no login on the console)", "check", c["console_open"],
               hint="OFF (default): the console asks for the root password first. ON = OPNsense style: anyone at the console can use the menu"),
         Field("warn", "WARNING: password SSH login exposes the system to password guessing from the network.", "label", "", hint="warn",
               visible=lambda v: v["pwauth"])]

    def check(a):
        if a["pw"] != a["pw2"]:
            return "The two passwords do not match."
        e = S.password_problem(a["pw"], c["hostname"])
        if e:
            return e
        return None
    b, v = run_form(t, step, "Administrator account",
                    "Set the administrator password (REQUIRED - there is no default password). SSH is key-only unless you enable password login below.",
                    f, validate_all=check)
    if b == "Back":
        return "back"
    c["root_password"], c["web_password"] = v["pw"], v["pw"]
    c["root_hash"] = ""
    c["email"] = v["email"].strip()
    c["ssh_keys"] = [v["key"].strip()] if v["key"].strip() else []
    c["ssh_password_auth"] = bool(v["pwauth"])
    c["console_open"] = bool(v["conopen"])
    return "next"


def summary_lines(st):
    c, p = st.c, st.p
    out = [("Boot mode", "UEFI" if S.efi_boot() else "legacy BIOS"),
           ("Language / keyboard", f"{c['locale']} / {c['keymap']}"), ("Time zone", c["timezone"]),
           ("Target disk(s)", ", ".join(c["disks"])),
           ("File system", c["fs"] + (f" ({c['zfs_raid']})" if c["fs"] == "zfs" else "")),
           ("Hostname", c["hostname"])]
    if p["network_mode"] == "wan-lan":
        w, l = c["wan"], c["lan"]
        out.append(("WAN", f"{w['iface'] or '-'}: " + (w["address"] if w["mode"] == "static" else "DHCP") + (" (Wi-Fi)" if c["wifi"] else "")))
        out.append(("LAN", f"{l['iface'] or '-'}: {l['address'] if l['iface'] else '-'}"))
    else:
        n = c["mgmt"]
        out.append(("Management NIC", f"{n['iface'] or '-'}: " + (f"{n['address']} gw {n['gateway']} dns {n['dns']}" if n["mode"] == "static" else "DHCP")
                    + (" (Wi-Fi)" if c["wifi"] else "")))
    out.append(("E-mail", c["email"] or "-"))
    out.append(("Console menu", "OPEN (no login)" if c["console_open"] else "after root login"))
    out.append(("SSH", ("key" if c["ssh_keys"] else "no key") + (", PASSWORD login ON" if c["ssh_password_auth"] else ", password login off")))
    return out


def s_summary(t, st, step):
    f = [Field(k, k, "info", v) for k, v in summary_lines(st)]
    f.append(Field("w1", f"All data on {', '.join(st.c['disks'])} will be DESTROYED.", "label", "", hint="warn"))
    f.append(Field("erase", f"Type {ERASE_WORD} to confirm", "text", "", width=12))

    def check(a):
        return None if a["erase"].strip() == ERASE_WORD else f"Type {ERASE_WORD} (capital letters) to confirm the wipe."
    b, _v = run_form(t, step, "Summary - ready to install", "Review the settings. Nothing has been written yet.", f,
                     buttons=("Back", "Install"), validate_all=check)
    return "back" if b == "Back" else "next"


def s_install(t, st, step):
    state = {"pct": 0, "note": "Starting", "lines": [], "err": None, "done": False}

    def log(line):
        state["lines"].append(line)

    def prog(pct, note):
        state["pct"], state["note"] = pct, note

    inst = S.Installer(st.p, st.c, log, prog)

    def work():
        try:
            if st.dry:
                for i in range(0, 101, 5):
                    prog(i, "dry run")
                    log(f"fake step {i}")
                    time.sleep(0.05)
            else:
                inst.go()
        except Exception as e:  # noqa
            state["err"] = e
            log("ERROR: " + str(e))
        state["done"] = True
    th = threading.Thread(target=work, daemon=True)
    th.start()
    t.scr.nodelay(True)            # never block in a curses read while the worker runs
    last, last_draw = None, 0.0
    while not state["done"]:
        snap = (state["pct"], state["note"], len(state["lines"]))
        # repaint only on change (a serial line is slow) and at most ~3x per second; the heartbeat
        # repaint every 5 s also un-sticks a terminal that missed a frame
        if (snap != last and time.time() - last_draw > 0.3) or time.time() - last_draw > 5:
            progress_screen(t, step, "Installing", state["pct"], state["lines"], state["note"])
            last, last_draw = snap, time.time()
        try:
            t.scr.get_wch()        # drain stray key presses
        except curses.error:
            pass
        time.sleep(0.15)
    t.scr.nodelay(False)
    t.scr.timeout(-1)
    progress_screen(t, step, "Installing", state["pct"], state["lines"], state["note"])
    if state["err"]:
        message(t, "Installation FAILED", f"{state['err']}\n\nThe log is in /var/log/live-installer.log. The installer will exit to a shell.", "err")
        raise Abort()
    return "next"


def web_url(st):
    c, p = st.c, st.p
    if p["network_mode"] == "wan-lan":
        ip = c["lan"]["address"].split("/")[0] if c["lan"]["iface"] else (c["wan"]["address"].split("/")[0] if c["wan"]["mode"] == "static" else "<WAN address from DHCP>")
    else:
        n = c["mgmt"]
        ip = n["address"].split("/")[0] if n["mode"] == "static" else (S.nic_ip(n["iface"]).split("/")[0] or "<address from DHCP>") if n["iface"] else "<IP address>"
    return f"{p['web_scheme']}://{ip}:{p['web_port']}/"


def s_done(t, st, step):
    c, p = st.c, st.p
    url = web_url(st)
    lines = [("Web UI", url), ("Login", "admin  /  the password you just set"),
             ("SSH", "root with " + ("your key" if c["ssh_keys"] else "NO key (console only)") + (" or password" if c["ssh_password_auth"] else "")),
             ("Console", "root  /  the same password")]
    f = [Field(k, k, "info", v) for k, v in lines]
    f.append(Field("m", "Installation successful. Remove the installation medium, then reboot.", "label", "", hint="warn"))
    if "<" in url:
        f.append(Field("m2", "The address comes from DHCP: it is shown on the console after boot and in your DHCP server's lease list.", "label"))
    b, _v = run_form(t, step, "Installation complete", f"{p['title']} has been installed.", f, buttons=("Shell", "Reboot"))
    return "reboot" if b == "Reboot" else "shell"


# --------------------------------------------------------------------------
# flow
# --------------------------------------------------------------------------
def flow(profile):
    scr = [s_locale]
    if len(profile["filesystems"]) > 1:
        scr.append(s_fs)
    scr.append(s_disk)
    hp, _hm = S.hwprofile.detect()
    if hp and hp.get("defaults", {}).get("desktop"):
        scr.append(s_desktop)
    if profile["network_mode"] == "wan-lan":
        scr += [s_ifaces, s_addr]
    else:
        scr.append(s_mgmt)
    scr += [s_account, s_summary, s_install, s_done]
    return scr


def interactive(profile, dry=False):
    st = State(profile)
    st.dry = dry

    def run(stdscr):
        t = tui.Term(stdscr, f"{profile['title']} installer", "UEFI" if S.efi_boot() else "BIOS")
        st.nics = S.list_nics() if not dry else [{"name": "eth0", "mac": "52:54:00:12:34:56", "wifi": False, "driver": "virtio_net", "model": "Virtio"},
                                                    {"name": "eth1", "mac": "52:54:00:12:34:57", "wifi": False, "driver": "virtio_net", "model": "Virtio"}]
        S.bring_up(st.nics)
        if not dry and S.large_screen():
            S.sh("setfont ter-v32n", timeout=10)
        screens = flow(profile)
        i = 0
        total = len(screens)
        while 0 <= i < total:
            step = f"{i + 1}/{total}"
            r = screens[i](t, st, step)
            if r == "next":
                i += 1
            elif r == "back":
                if i == 0:
                    raise Abort()
                i -= 1
            elif r in ("reboot", "shell"):
                return r
            # "again": stay
        return "shell"
    try:
        os.environ.setdefault("TERM", "linux")
        return curses.wrapper(run)
    except Abort:
        return "abort"


# --------------------------------------------------------------------------
# unattended
# --------------------------------------------------------------------------
def pick_first_disk():
    for d in S.list_disks():
        if not d["removable"] and d["size"] >= 8 * 10**9 and d["kind"] != "USB":
            return d["path"]
    return None


def unattended(profile, answers_path, halt=True):
    def say(s):
        print(s, flush=True)
    say("AUTOINSTALL-START answer file " + answers_path)
    try:
        c = S.config_from_answers(profile, answers_path)
        c["disks"] = [pick_first_disk() if d == "first" else d for d in c["disks"]]
        if None in c["disks"]:
            raise S.InstallError("no suitable internal disk for 'first'")
        nics = S.list_nics()
        wired = [n for n in nics if not n["wifi"]]
        S.bring_up(nics)
        time.sleep(2)

        def resolve(sect, which=0):
            n = c[sect]
            if n.get("iface") in ("", "auto", None):
                n["iface"] = ""
                if wired:
                    up = [x for x in wired if S.carrier(x["name"])]
                    pool = up or wired
                    if len(pool) > which:
                        n["iface"] = pool[which]["name"]
        if profile["network_mode"] == "wan-lan":
            resolve("wan", 0)
            lan_pool = [x for x in wired if x["name"] != c["wan"]["iface"]]
            if c["lan"].get("iface") in ("", "auto", None):
                c["lan"]["iface"] = lan_pool[0]["name"] if lan_pool else ""
        else:
            resolve("mgmt", 0)
        errs = S.validate_config(profile, c, unattended=True)
        if errs:
            for e in errs:
                say("ANSWER-FILE-ERROR " + e)
            raise S.InstallError("invalid answer file")
        say(f"AUTOINSTALL-PLAN disks={','.join(c['disks'])} fs={c['fs']} host={c['hostname']} net={c['mgmt']['iface'] or (c['wan']['iface'] + '/' + c['lan']['iface'])}")
        last = {"n": -1}

        def prog(pct, note):
            if pct // 10 != last["n"]:
                last["n"] = pct // 10
                say(f"AUTOINSTALL-PROGRESS {pct}% {note}")
        inst = S.Installer(profile, c, lambda l: say("  " + l), prog)
        inst.go()
    except Exception as e:  # noqa
        say(f"AUTOINSTALL-FAILED {e}")
        return 1
    say("AUTOINSTALL-OK")
    return 0


def main(argv=None):
    import argparse
    ap = argparse.ArgumentParser(prog="live-installer")
    ap.add_argument("--profile", default="/etc/live-installer/profile.toml")
    ap.add_argument("--answers", help="answer file path, or 'auto' to look for one (kernel arg / labelled volume)")
    ap.add_argument("--no-halt", action="store_true")
    ap.add_argument("--dry-run", action="store_true", help="walk the screens without touching disks")
    a = ap.parse_args(argv)
    if os.geteuid() != 0 and not a.dry_run:
        print("The installer must run as root.")
        return 1
    profile = S.load_profile(a.profile)
    if a.answers:
        path = a.answers
        if path == "auto":
            path = S.find_answer_file(profile["name"], S.read("/proc/cmdline"))
            if not path:
                print("No answer file found (kernel arg %s.answers=... or a volume labelled %s-ANSWERS with answer.toml)." %
                      (profile["name"], profile["name"].upper()), flush=True)
                return 3
        rc = unattended(profile, path)
        if rc == 0 and not a.no_halt:
            os.sync()
            time.sleep(2)
            S.sh("poweroff")
        return rc
    r = interactive(profile, dry=a.dry_run)
    if r == "reboot":
        print("\nRebooting - remove the installation medium now.", flush=True)
        os.sync()
        if not a.dry_run:
            S.sh("reboot")
        return 0
    if r == "abort":
        print("\nInstaller aborted. You are on the live system; run 'live-installer' to start again.")
    return 0
