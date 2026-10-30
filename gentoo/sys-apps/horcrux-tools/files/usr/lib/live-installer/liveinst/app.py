"""Guided installer, text front-end: renders the shared stages (stages.py) with curses, plus the unattended runner."""
import curses
import os
import sys
import threading
import time

from . import stages as ST
from . import system as S
from . import portableswap
from . import tui
from . import useflags as UF
from .stages import ERASE_WORD, Field
from .tui import Abort, run_form, run_disk_picker, message, progress_screen


class State:
    def __init__(self, profile):
        self.p = profile
        self.c = S.default_config(profile)
        self.nics = []
        self.dry = False
        self.ctx = ST.Ctx(profile, self.c)

    def sync(self):
        self.ctx.nics = self.nics


# --------------------------------------------------------------------------
# generic form stage - the same declarative Field list the GUI renders
# --------------------------------------------------------------------------
def tui_stage(t, st, stage, step, fields=None, buttons=("Back", "Next")):
    ctx = st.ctx
    f = fields if fields is not None else stage.fields(ctx)
    adv = [x for x in f if x.advanced]
    if adv:
        f.append(Field("__adv", "Show advanced options", "check", os.environ.get("LIVE_INSTALLER_EXPERT") == "1", hint="Space toggles the advanced options of this screen"))
        for x in adv:
            old = x.visible
            x.visible = (lambda v, old=old: bool(v.get("__adv")) and (old is None or old(v)))
    b, v = run_form(t, step, stage.title, stage.intro, f, buttons=buttons, validate_all=lambda a: stage.validate(ctx, a))
    return b, v


def s_keymap(t, st, step):
    b, v = tui_stage(t, st, ST.BY_ID["keymap"], step, buttons=("Abort", "Next"))
    if b == "Abort":
        raise Abort()
    ST.BY_ID["keymap"].commit(st.ctx, v)
    S.apply_live_keymap(st.c["keymap"])
    return "next"


def s_hostname(t, st, step):
    b, v = tui_stage(t, st, ST.BY_ID["hostname"], step)
    if b == "Back":
        return "back"
    ST.BY_ID["hostname"].commit(st.ctx, v)
    return "next"


def s_disk(t, st, step):
    c = st.c
    stage = ST.BY_ID["disk"]
    if len(st.p["filesystems"]) > 1:
        b, v = tui_stage(t, st, stage, step)
        if b == "Back":
            return "back"
        stage.commit(st.ctx, v)
        if c["fs"] == "zfs" and not st.dry and S.sh("modprobe zfs && test -e /dev/zfs")[0] != 0:
            message(t, "ZFS not available", "The ZFS kernel module could not be loaded on this machine. Choose XFS.", "err")
            return "again"
    disks = st.ctx.disks if st.dry else S.list_disks()
    st.ctx.disks = disks
    if not disks:
        message(t, "No disk found", "No usable internal disk (>= 2 GB) was found. If the disk is behind a RAID/VMD "
                "controller, switch the firmware to AHCI mode. The installer will exit to a shell.", "err")
        raise Abort()
    mirror = stage.need_disks(c) == 2
    intro = (stage.intro.split(".")[0] + ". ") + ("Select the TWO disks for the mirror." if mirror else "Select the disk to install to.") + \
            " Everything on a selected disk will be ERASED. The live USB stick is not listed."
    sel = [d for d in c["disks"] if any(x["path"] == d for x in disks)]
    while True:
        b, sel = run_disk_picker(t, step, "Target disk" + ("s" if mirror else ""), intro, disks, sel, mirror, S.fmt_size)
        if b == "Back":
            return "back"
        err, warn = stage.check_selection(st.ctx, sel)
        if err:
            message(t, "Disk selection", err, "err")
            continue
        if warn and message(t, "Different disk sizes", warn, "warn", ("Back", "Continue")) != "Continue":
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
         Field("mode", "IP configuration", "choice", n["mode"], [("dhcp", "DHCP (automatic)"), ("static", "Static address")]),
         Field("address", "IP address / prefix", "text", n["address"] or cur_ip, validate=lambda v, a: S.cidr_problem(v),
               visible=lambda v: v["mode"] == "static", hint="e.g. 192.168.1.50/24"),
         Field("gateway", "Gateway", "text", n["gateway"] or default_gateway(), validate=lambda v, a: S.ip_problem(v, a.get("address")),
               visible=lambda v: v["mode"] == "static"),
         Field("dns", "DNS server(s)", "text", n["dns"] or default_dns(), validate=lambda v, a: S.dns_problem(v),
               visible=lambda v: v["mode"] == "static", hint="space separated")]
    b, v = run_form(t, step, "Management network",
                    "Pick the network interface for the management web UI and SSH.", f)
    if b == "Back":
        return "back"
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
    f = [Field("wmode", "WAN addressing", "choice", w["mode"], [("dhcp", "DHCP (from the ISP / upstream)"), ("static", "Static address")]),
         Field("waddr", "WAN address/prefix", "text", w["address"], validate=lambda v, a: S.cidr_problem(v), visible=lambda v: v["wmode"] == "static"),
         Field("wgw", "WAN gateway", "text", w["gateway"], validate=lambda v, a: S.ip_problem(v, a.get("waddr")), visible=lambda v: v["wmode"] == "static"),
         Field("wdns", "DNS server(s)", "text", w["dns"], validate=lambda v, a: S.dns_problem(v), visible=lambda v: v["wmode"] == "static")]
    if l["iface"]:
        f.append(Field("laddr", "LAN address/prefix", "text", l["address"], validate=lambda v, a: S.cidr_problem(v),
                       hint="The firewall's own LAN address; also the web UI address"))
    b, v = run_form(t, step, "WAN / LAN addressing",
                    "Set how the WAN gets its address. The LAN is static (a DHCP server for LAN clients can be enabled later in the web UI)."
                    + (" WAN is Wi-Fi (DHCP)." if wifi else ""), f)
    if b == "Back":
        return "back"
    w.update({"mode": "dhcp" if wifi else v["wmode"], "address": v.get("waddr", ""), "gateway": v.get("wgw", ""), "dns": v.get("wdns", "")})
    if l["iface"]:
        l["address"] = v["laddr"].strip()
    return "next"


def s_stage(sid):
    def run(t, st, step):
        stage = ST.BY_ID[sid]
        b, v = tui_stage(t, st, stage, step)
        if b == "Back":
            return "back"
        stage.commit(st.ctx, v)
        return "next"
    return run


def s_rootpass(t, st, step):
    return s_stage("rootpass")(t, st, step)


def s_netconfig_mgmt(t, st, step):
    return s_mgmt(t, st, step)


# --------------------------------------------------------------------------
# USE flags (text front-end): preset bar, filter + flag list, live Portage sanity check
# --------------------------------------------------------------------------
def s_use(t, st, step):
    stage = ST.BY_ID["use_flags"]
    meta = UF.load_meta()
    if not meta:
        message(t, "Software (USE flags)", "This image carries no Portage metadata, so the USE flags cannot be edited. "
                "The tested image defaults are used.", "warn")
        return "next"
    st.ctx.use_meta = meta
    use = st.c["use"]
    rows = meta.flag_rows()
    group_names = ["All groups"] + [g for g in UF.GROUPS if any(r["group"] == g for r in rows)]
    gi, flt, cur, top = 0, "", 0, 0
    focus = "list"            # preset | list | buttons
    pi = [x[0] for x in UF.PRESETS].index(use["preset"]) if use["preset"] in [x[0] for x in UF.PRESETS] else 0
    bsel = 2
    msg = ""
    while True:
        choices = UF.effective_choices(meta, st.p, use)
        issues = UF.check(meta, st.p, choices)
        shown = [r for r in meta.search(flt, rows) if gi == 0 or r["group"] == group_names[gi]]
        h, w = t.frame(step, stage.title)
        pname = use["preset"]
        t.put(4, 2, "Preset: ", t.a("bold"))
        x = 10
        for k, label, _d in UF.PRESETS:
            lab = f"< {label} >" if k == pname else f"  {label}  "
            t.put(4, x, lab, t.a("sel") if (focus == "preset" and k == UF.PRESETS[pi][0]) else (t.a("title") if k == pname else 0))
            x += len(lab) + 1
        if pname == "custom":
            t.put(4, x, "(custom)", t.a("warn"))
        t.put(5, 2, "Group:  " + group_names[gi] + "   (Ctrl-G changes)", 0)
        t.put(6, 2, "Filter: " + flt + ("_" if focus == "list" else ""), t.a("bold") if focus == "list" else 0)
        rows_n = max(3, h - 17)
        cur = min(cur, max(0, len(shown) - 1))
        if cur < top:
            top = cur
        if cur >= top + rows_n:
            top = cur - rows_n + 1
        for i in range(top, min(len(shown), top + rows_n)):
            r = shown[i]
            f = r["flag"]
            want = choices.get(f)
            now = want if want is not None else r["state"]
            mark = "[x]" if now is True else ("[ ]" if now is False else "[~]")
            chg = "*" if f in choices else " "
            line = f"{mark}{chg}{f:<22} {r['desc']}"
            t.put(7 + i - top, 2, line[: w - 4], t.a("sel") if (focus == "list" and i == cur) else (t.a("bold") if f in choices else 0))
        y = 7 + rows_n
        sel = shown[cur] if shown else None
        if sel:
            pk = ", ".join(sel["packages"][:3]) + (" ..." if len(sel["packages"]) > 3 else "") if sel["packages"] else "no installed package uses it"
            t.put(y, 2, ("Used by: " + pk)[: w - 4])
        y += 1
        errs = [i for i in issues if i.level == "error"]
        warns = [i for i in issues if i.level == "warning"]
        t.put(y, 2, f"Portage check: {len(errs)} error(s), {len(warns)} warning(s)", t.a("err") if errs else (t.a("warn") if warns else t.a("ok")))
        y += 1
        for i in (errs + warns + [x for x in issues if x.level == "info"])[:2]:
            t.put(y, 3, (("ERROR " if i.level == "error" else "warn  " if i.level == "warning" else "note  ") + i.text)[: w - 5],
                  t.a("err") if i.level == "error" else 0)
            y += 1
        t.put(y, 2, UF.summary_line(meta, choices)[: w - 4])
        by = h - 3
        bx = 2
        for i, b in enumerate(("Back", "Details", "Next")):
            lab = f"< {b} >"
            t.put(by, bx, lab, t.a("sel") if (focus == "buttons" and i == bsel) else 0)
            bx += len(lab) + 2
        t.footer(msg or "Tab: preset/list/buttons  Space: toggle flag  type: filter  Ctrl-G: group  Esc: abort", t.a("err") if msg else 0)
        msg = ""
        t.scr.refresh()
        k = t.key()
        if k == tui.ESC:
            if message(t, "Abort installation?", "Leave the installer? Nothing has been changed on disk yet.", "warn", ("Continue", "Abort")) == "Abort":
                raise Abort()
        elif k == "\x07":
            gi = (gi + 1) % len(group_names)
            cur = top = 0
        elif k == tui.TAB:
            focus = {"preset": "list", "list": "buttons", "buttons": "preset"}[focus]
        elif k == tui.BTAB:
            focus = {"preset": "buttons", "list": "preset", "buttons": "list"}[focus]
        elif focus == "preset":
            if k in (tui.LEFT, tui.RIGHT):
                pi = (pi + (1 if k == tui.RIGHT else -1)) % len(UF.PRESETS)
            elif k in (tui.ENTER, " "):
                use["preset"] = UF.PRESETS[pi][0]
                use["flags"] = {}
            elif k in (tui.DOWN,):
                focus = "list"
        elif focus == "buttons":
            if k in (tui.LEFT, tui.RIGHT):
                bsel = (bsel + (1 if k == tui.RIGHT else -1)) % 3
            elif k == tui.UP:
                focus = "list"
            elif k == tui.ENTER:
                if bsel == 0:
                    return "back"
                if bsel == 1:
                    lines = [("ERROR: " if i.level == "error" else "warning: " if i.level == "warning" else "note: ") + i.text for i in issues] or ["No problems found."]
                    imp = UF.impact(meta, choices)
                    lines += ["", UF.summary_line(meta, choices)] + [f"  {p_}  ~{m} min" for p_, m in imp["list"][:8]]
                    message(t, "Portage sanity check and build impact", "\n".join(lines), "warn" if issues else "ok")
                else:
                    if errs:
                        msg = "Fix the error first: " + errs[0].text
                        focus = "list"
                        continue
                    if errs == [] and use["preset"] != "recommended" or use["flags"]:
                        pass
                    return "next"
        else:
            if k == tui.UP:
                cur = max(0, cur - 1)
            elif k == tui.DOWN:
                cur = min(len(shown) - 1, cur + 1)
            elif k == tui.PGUP:
                cur = max(0, cur - rows_n)
            elif k == tui.PGDN:
                cur = min(len(shown) - 1, cur + rows_n)
            elif k == tui.ENTER:
                focus = "buttons"
            elif k == " " and sel:
                f = sel["flag"]
                now = choices.get(f, sel["state"])
                flags = dict(choices)           # freeze preset into explicit choices, then edit
                nv = (not now) if now is not None else True
                if nv == sel["state"]:
                    flags.pop(f, None)
                else:
                    flags[f] = nv
                use["flags"] = flags
                use["preset"] = "custom"
            elif k == tui.BACKSPACE:
                flt = flt[:-1]
                cur = top = 0
            elif k == "CLEAR":
                flt = ""
            elif len(k) == 1 and k >= " ":
                flt += k
                cur = top = 0


def swap_summary(c):
    hp, _hm = S.hwprofile.detect()
    return portableswap.describe(portableswap.plan(c, S.mem_mb(), hp))


def summary_lines(st):
    c, p = st.c, st.p
    out = [("Install type", c["install_type"].capitalize() + f"   ({'UEFI' if S.efi_boot() else 'legacy BIOS'})"),
           ("Language / keyboard", f"{c['locale']} / {c['keymap']} / {c['timezone']}"),
           ("Target disk(s)", ", ".join(c["disks"]) + f"   {c['fs']}" + (f" ({c['zfs_raid']})" if c["fs"] == "zfs" else "")),
           ("Hostname", c["hostname"])]
    for line in ST.BY_ID["netconfig"].plan(st.ctx):
        out.append(("Network", line))
    out.append(("E-mail", c["email"] or "-"))
    out.append(("Services", ", ".join(x for x in p["services"] if x not in c["services_off"] and x != "sshd") or "-"))
    out.append(("SSH", ("off" if not c["ssh_enabled"] else (("key" if c["ssh_keys"] else "no key") + (", PASSWORD login ON" if c["ssh_password_auth"] else ", password login off")))))
    out.append(("Hardening", "sysctl " + ("on" if c["sysctl_hardening"] else "off") + "; console menu " + ("OPEN (no login)" if c["console_open"] else "after root login")))
    out.append(("Software", ST.BY_ID["use_flags"].plan(st.ctx)[0]))
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
    inst._ctx = st.ctx

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
def flow(profile, st=None):
    scr = [s_keymap, s_hostname, s_disk]
    hp, _hm = S.hwprofile.detect()
    desk = [s_stage("desktop")] if hp and hp.get("defaults", {}).get("desktop") else []      # Surface: desktop user + autologin
    if st is not None:                                  # Apple T2 Mac / Desktop image: their own pages (backend keys t2*, user_*)
        desk += [s_stage(sid) for sid in ("t2", "gnome") if ST.BY_ID[sid].applies(st.ctx)]
    if profile["network_mode"] == "wan-lan":
        scr += [s_rootpass] + desk + [s_ifaces, s_addr]
    else:
        scr += [s_rootpass] + desk + [s_mgmt]
    scr += [s_stage("services"), s_stage("hardening"), s_use, s_summary, s_install, s_done]
    return scr


def interactive(profile, dry=False):
    st = State(profile)
    st.dry = dry

    def run(stdscr):
        t = tui.Term(stdscr, f"{profile['title']} installer", "UEFI" if S.efi_boot() else "BIOS")
        if dry:
            st.ctx.disks = [{"path": "/dev/vda", "name": "vda", "size": 64 * 10**9, "model": "QEMU HARDDISK", "kind": "SSD", "removable": False, "parts": []},
                            {"path": "/dev/vdb", "name": "vdb", "size": 64 * 10**9, "model": "QEMU HARDDISK", "kind": "SSD", "removable": False, "parts": []}]
        st.nics = S.list_nics() if not dry else [{"name": "eth0", "mac": "52:54:00:12:34:56", "wifi": False, "driver": "virtio_net", "model": "Virtio"},
                                                    {"name": "eth1", "mac": "52:54:00:12:34:57", "wifi": False, "driver": "virtio_net", "model": "Virtio"}]
        S.bring_up(st.nics)
        if not dry and S.large_screen():
            S.sh("setfont ter-v32n", timeout=10)
        st.sync()
        screens = flow(profile, st)
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
        c["ssh_enabled"] = c["ssh_enabled"] and True
        errs = S.validate_config(profile, c, unattended=True)
        if errs:
            for e in errs:
                say("ANSWER-FILE-ERROR " + e)
            raise S.InstallError("invalid answer file")
        say(f"AUTOINSTALL-PLAN disks={','.join(c['disks'])} fs={c['fs']} swap={c.get('swap', 'auto')} host={c['hostname']} net={c['mgmt']['iface'] or (c['wan']['iface'] + '/' + c['lan']['iface'])}")
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
    ap.add_argument("--ui", choices=["auto", "gui", "tui"], default="tui",
                    help="gui = GTK4 front-end (needs a Wayland/X display; exits 75 when it cannot start), tui = curses")
    ap.add_argument("--plan", action="store_true", help="print the stage plan for the answer file (or the defaults) and exit")
    a = ap.parse_args(argv)
    if os.geteuid() != 0 and not a.dry_run:
        print("The installer must run as root.")
        return 1
    profile = S.load_profile(a.profile)
    if a.plan:
        c = S.config_from_answers(profile, a.answers) if a.answers and a.answers != "auto" else S.default_config(profile)
        ctx = ST.Ctx(profile, c, disks=S.list_disks())
        for sid, lines in ST.full_plan(ctx):
            print(f"[{sid}]")
            for l in lines:
                print("  " + l)
        return 0
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
    if a.ui in ("gui", "auto"):
        try:
            from . import gui
        except Exception as e:  # noqa - no GTK/PyGObject on this image
            print("graphical installer unavailable:", e, file=sys.stderr)
            return 75
        r = gui.run(profile, dry=a.dry_run)
        res = os.environ.get("LIVEINST_RESULT")
        if res:
            try:
                with open(res, "w") as f:
                    f.write(r + "\n")
            except OSError:
                pass
        if r == "xkb":
            return 0
        if r == "fallback":
            return 75
    else:
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
