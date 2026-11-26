"""Post-install console menu (OPNsense/pfSense style) for the installed system.

Plain line-oriented text: identical on the VGA console and on a serial line. Started from
/etc/profile.d/console-menu.sh for root logins on tty1/ttyS0/hvc0 (or autologin when the
installer was told to open the console menu), `console-menu` runs it by hand.
"""
import datetime
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

from . import initsys
from . import system as S

BACKUP_DIR_FMT = "/var/backups/{name}"
DHCPCD = "/etc/dhcpcd.conf"
HOST_RE = re.compile(r"^[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?$|^[0-9a-fA-F:]+$")


# --------------------------------------------------------------------------
# pure helpers (unit tested)
# --------------------------------------------------------------------------
def strip_iface_stanza(text, iface):
    """Remove every `interface <iface>` stanza (up to the next blank line / `interface`) and our managed block."""
    out, skip = [], False
    managed = re.compile(r"^# BEGIN console-menu %s$" % re.escape(iface))
    managed_end = re.compile(r"^# END console-menu %s$" % re.escape(iface))
    in_block = False
    for line in text.splitlines():
        if managed.match(line):
            in_block = True
            continue
        if in_block:
            if managed_end.match(line):
                in_block = False
            continue
        if re.match(r"^interface\s+%s\s*$" % re.escape(iface), line):
            skip = True
            continue
        if skip:
            if not line.strip() or line.startswith("interface "):
                skip = False
                if line.startswith("interface "):
                    out.append(line)
            continue
        out.append(line)
    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out) + ("\n" if out else "")


IPV6_MODES = ("auto", "static", "none")      # auto = router advertisements / DHCPv6 (dhcpcd's default)


def ipv6_problem(addr):
    """an IPv6 address with prefix (2001:db8::2/64), or None"""
    import ipaddress
    try:
        i = ipaddress.ip_interface(addr)
    except ValueError:
        return f"{addr!r} is not an IPv6 address/prefix (e.g. 2001:db8::2/64)"
    if i.version != 6 or "/" not in addr:
        return f"{addr!r} is not an IPv6 address/prefix (e.g. 2001:db8::2/64)"
    return None


def dhcpcd_block(iface, mode, address="", gateway="", dns="", v6="auto", v6_address="", v6_gateway=""):
    if v6 not in IPV6_MODES:
        raise ValueError(f"IPv6 mode {v6!r} (one of {', '.join(IPV6_MODES)})")
    if mode != "static" and v6 == "auto":
        return f"# BEGIN console-menu {iface}\n# {iface}: DHCP (set from the console menu)\n# END console-menu {iface}\n"
    lines = [f"# BEGIN console-menu {iface}", f"interface {iface}"]
    if mode == "static":
        lines.append(f"static ip_address={address}")
        if gateway:
            lines.append(f"static routers={gateway}")
        if dns:
            lines.append("static domain_name_servers=" + " ".join(re.split(r"[\s,]+", dns.strip())))
    if v6 == "static":
        lines += [f"static ip6_address={v6_address}", "noipv6rs"]          # fixed address: no SLAAC/DHCPv6 address next to it
    elif v6 == "none":
        lines.append("noipv6")
    lines.append(f"# END console-menu {iface}")
    return "\n".join(lines) + "\n"


def apply_iface_config(text, iface, mode, address="", gateway="", dns="", v6="auto", v6_address="", v6_gateway=""):
    return strip_iface_stanza(text, iface) + "\n" + dhcpcd_block(iface, mode, address, gateway, dns, v6, v6_address, v6_gateway)


def apply_roles(text, wan, lan, opts=None):
    """Set wan_interface / lan_interface (and opt_interfaces = OPT1, OPT2 ... in that order; None keeps the current list) in the
    [network] section of patronus.toml text."""
    if opts is None:
        opts = read_opts(text)
    lines = [l for l in text.splitlines() if not re.match(r"\s*(wan_interfaces?|lan_interfaces?|opt_interfaces)\s*=", l)
             and l.strip() not in ("# written by live-installer", "# written by live-installer / console menu")]
    roles = ["# written by live-installer / console menu"]
    if wan:
        roles.append(f'wan_interface = "{wan}"')
    if lan:
        roles.append(f'lan_interface = "{lan}"')
    if opts:
        roles.append("opt_interfaces = [" + ", ".join(f'"{o}"' for o in opts) + "]")
    out, placed = [], False
    for l in lines:
        out.append(l)
        if l.strip() == "[network]" and not placed:
            out += roles
            placed = True
    if not placed:
        out += ["", "[network]"] + roles
    return "\n".join(out) + "\n"


def read_roles(text):
    m = lambda k: (re.search(rf'(?m)^\s*{k}\s*=\s*"([^"]*)"', text) or [None, ""])[1]
    return m("wan_interface"), m("lan_interface")


def read_opts(text):
    """opt_interfaces = ["eth2", "eth3"] -> ["eth2", "eth3"] (OPT1, OPT2)"""
    m = re.search(r'(?m)^\s*opt_interfaces\s*=\s*\[([^\]]*)\]', text)
    return re.findall(r'"([^"]+)"', m.group(1)) if m else []


def console_items(profile, taken):
    """the product's own console-menu items (profile `console_items`, a list of tables: key, title, run = argv, optional
    show = argv run first (e.g. a list to choose from), ask = a prompt whose answer replaces {answer} in run/then,
    confirm = the question asked after run, then = argv (or a list of argv, run in order, stopping at the first failure)
    run after a yes).
    Returns (items, problems); an item whose key clashes with a built-in one or another item is dropped with a problem."""
    out, bad, seen = [], [], set(taken)
    for i, it in enumerate(profile.get("console_items", []) or []):
        k, t, run = str(it.get("key", "")).strip(), str(it.get("title", "")).strip(), it.get("run")
        argvs = [run] + ([it["show"]] if "show" in it else []) + then_list(it)
        if not k or not t or not all(isinstance(a, list) and a and all(isinstance(x, str) for x in a) for a in argvs):
            bad.append(f"console_items[{i}]: needs key, title and run = [argv] (show/then: argv too)")
            continue
        if k in seen:
            bad.append(f"console_items[{i}]: key {k!r} is already used")
            continue
        seen.add(k)
        out.append(dict(it, key=k, title=t))
    return out, bad


def then_list(it):
    """an item's follow-up commands: then = argv or [argv, argv ...]"""
    t = it.get("then")
    if not t:
        return []
    return [t] if all(isinstance(x, str) for x in t) else list(t)


def fill_argv(argv, answer):
    """{answer} placeholders of a console item's argv (one argv element each: never a shell)"""
    return [a.replace("{answer}", answer) for a in argv]


def host_ok(h):
    return bool(h) and len(h) <= 253 and bool(HOST_RE.match(h)) and not h.startswith("-")


def member_allowed(name, allowed_roots):
    """A backup member may only restore below the product's own config paths."""
    n = os.path.normpath("/" + name.lstrip("/"))
    if ".." in name.split("/"):
        return False
    if n == "/MANIFEST.json":
        return True
    return any(n == r or n.startswith(r.rstrip("/") + "/") for r in allowed_roots)


def argon2_encode(password, run=None):
    """PHC argon2id string (the products verify with the Rust argon2 crate, which reads params from the string)."""
    run = run or subprocess.run
    salt = os.urandom(12).hex()
    r = run(["argon2", salt, "-id", "-e", "-t", "2", "-m", "16", "-p", "1"], input=password, capture_output=True, text=True, timeout=60)
    h = (r.stdout or "").strip()
    if r.returncode != 0 or not h.startswith("$argon2id$"):
        raise RuntimeError("argon2 hashing failed: " + (r.stderr or h)[:100])
    return h


# --------------------------------------------------------------------------
# the menu
# --------------------------------------------------------------------------
class Console:
    def __init__(self, profile, inp=input, out=None, root="/"):
        self.p = profile
        self.inp = inp
        self.out = out or sys.stdout
        self.root = root
        self.name = profile["name"]
        self.web = profile.get("web_service") or next((s for s in profile.get("services", []) if "web" in s or "api" in s), "")
        self.db = profile.get("db_path", "")
        self.table = profile.get("db_table", "users")
        self.cfg_paths = list(profile.get("config_paths", [f"/etc/{self.name}"]))
        self.backup_dir = BACKUP_DIR_FMT.format(name=self.name)
        self.init = initsys.running_init()

    def svc_cmd(self, action, svc):
        """argv for a service action on this (running) system: rc-service on OpenRC, systemctl on systemd"""
        return initsys.control_cmd(self.init, action, svc)

    # -- io ---------------------------------------------------------------
    def say(self, s=""):
        print(s, file=self.out, flush=True)

    def ask(self, prompt, default=""):
        try:
            v = self.inp(prompt + (f" [{default}]" if default else "") + ": ")
        except EOFError:
            raise
        v = (v or "").strip()
        return v or default

    def yes(self, prompt, default=False):
        v = self.ask(prompt + (" [Y/n]" if default else " [y/N]")).lower()
        return default if not v else v[0] == "y"

    def pause(self):
        try:
            self.inp("\nPress Enter to return to the menu ")
        except EOFError:
            pass

    def secret(self, prompt):
        import getpass
        try:
            return getpass.getpass(prompt)
        except (EOFError, OSError):
            return self.inp(prompt)

    # -- status ---------------------------------------------------------------
    def web_url(self):
        ip = ""
        roles = self.roles()
        for n in ([roles[1], roles[0]] if self.p["network_mode"] == "wan-lan" else [None]):
            if n:
                ip = S.nic_ip(n).split("/")[0]
                if ip:
                    break
        if not ip:
            for n in S.list_nics():
                ip = S.nic_ip(n["name"]).split("/")[0]
                if ip:
                    break
        return f"{self.p['web_scheme']}://{ip or '<no address yet>'}:{self.p['web_port']}/"

    def roles(self):
        if self.p["network_mode"] != "wan-lan":
            return "", ""
        return read_roles(S.read(self.root + "etc/patronus/patronus.toml"))

    def opts(self):
        if self.p["network_mode"] != "wan-lan":
            return []
        return read_opts(S.read(self.root + "etc/patronus/patronus.toml"))

    def header(self):
        wan, lan = self.roles()
        opts = self.opts()
        self.say("\033[H\033[2J" if os.environ.get("TERM", "dumb") != "dumb" else "")
        self.say(f"*** {self.p['title']} - console menu ***   {S.read('/etc/hostname')}")
        self.say("")
        for n in S.list_nics():
            role = ("WAN" if n["name"] == wan else "LAN" if n["name"] == lan else
                    f"OPT{opts.index(n['name']) + 1}" if n["name"] in opts else "")
            ip = S.nic_ip(n["name"]) or "-"
            link = "wifi" if n["wifi"] else ("up" if S.carrier(n["name"]) else "no link")
            self.say(f" {role:<5}{n['name']:<10}{link:<8}{ip}")
        self.say("")
        self.say(f" Web UI: {self.web_url()}   (user: " + ("a system user in wheel/" + self.p.get("web_admin_group", "horcrux-admin") if self.p.get("web_auth") == "pam" else "admin") + ")")
        self.say("")

    MENU = [
        ("1", "Assign interfaces"), ("2", "Set interface IP address (DHCP / static)"), ("3", "Reset the admin password"),
        ("4", "Reset to factory defaults"), ("5", "Power off"), ("6", "Reboot"), ("7", "Ping host"), ("8", "Shell"),
        ("9", "Show web UI URL and status"), ("10", "Restore a configuration backup"), ("11", "Create a configuration backup"),
        ("12", "Restart the web UI"), ("13", "Turn SSH on or off"), ("14", "Live system view (top, q quits)"),
        ("0", "Log out"),
    ]

    def items(self):
        """the product's console items (profile console_items) after the built-in ones"""
        it, bad = console_items(self.p, [k for k, _ in self.MENU])
        return it

    def show_menu(self):
        self.header()
        for k, t in self.MENU[:-1] + [(i["key"], i["title"]) for i in self.items()] + self.MENU[-1:]:
            if k == "3" and self.p.get("web_auth") == "pam":
                t = "Reset a user password"          # the web UI signs in system users: this is chpasswd, not a web database
            self.say(f"  {k:>2}) {t}")
        self.say("")

    def loop(self):
        acts = {"1": self.assign, "2": self.set_ip, "3": self.reset_password, "4": self.factory_reset, "5": self.poweroff,
                "6": self.reboot, "7": self.ping, "8": self.shell, "9": self.status, "10": self.restore, "11": self.backup_cmd,
                "12": self.restart_web, "13": self.ssh_toggle, "14": self.top}
        for it in self.items():
            acts[it["key"]] = (lambda it=it: self.run_item(it))
        while True:
            self.show_menu()
            try:
                c = self.ask("Enter an option")
            except EOFError:
                return 0
            if c in ("0", "exit", "logout", "q"):
                return 0
            fn = acts.get(c)
            if not fn:
                if c:
                    self.say(f"'{c}' is not a menu option.")
                    time.sleep(1)
                continue
            try:
                fn()
            except EOFError:
                return 0
            except KeyboardInterrupt:
                self.say("\n(interrupted)")
            except Exception as e:  # noqa - never kill the console on a bug
                self.say(f"Error: {e}")
            if c not in ("5", "6", "8", "14"):
                self.pause()

    # -- 12 / 13 / 14 + the product's items ------------------------------------------
    def restart_web(self):
        if not self.web:
            self.say("This system has no web UI service.")
            return
        self.say(f"Restarting {self.web} ...")
        rc, o = self.rc_restart(self.web)
        up = self.wait_listening(60)
        self.say(f"  {self.web}: " + ("running, " + self.web_url() if up else "NOT listening: " + (o.strip()[-160:] or "see the service log")))

    def ssh_svc(self):
        return self.p.get("ssh_service", "sshd")

    def ssh_running(self):
        rc, o = S.sh(self.svc_cmd("status", self.ssh_svc()))
        return ("started" in o) if self.init == "openrc" else rc == 0

    def ssh_toggle(self):
        svc, on = self.ssh_svc(), self.ssh_running()
        self.say(f"SSH ({svc}) is {'ON' if on else 'OFF'}.")
        if on:
            if os.environ.get("SSH_CONNECTION"):
                self.say("You are connected over SSH: turning it off ends this session.")
            if not self.yes("Turn SSH OFF (stopped now and at boot)?"):
                return
            for cmd in [self.svc_cmd("stop", svc)] + initsys.disable_cmds(self.init, svc):
                S.sh(cmd)
        else:
            if not self.yes("Turn SSH ON (started now and at boot)?", True):
                return
            for cmd in initsys.enable_cmds(self.init, svc) + [self.svc_cmd("start", svc)]:
                S.sh(cmd)
        self.say(f"SSH is now {'ON' if self.ssh_running() else 'OFF'}.")

    def top(self):
        subprocess.run(["top"])

    def run_item(self, it):
        """a profile console item: optional question, the command in the foreground, optional confirm + follow-up command"""
        answer = ""
        if it.get("show"):
            subprocess.run(it["show"])
        if it.get("ask"):
            answer = self.ask(it["ask"], str(it.get("default", "")))
            if not answer:
                return
        r = subprocess.run(fill_argv(it["run"], answer))
        steps = then_list(it)
        if steps and getattr(r, "returncode", 0) == 0 and self.yes(it.get("confirm", "Apply it now?")):
            for argv in steps:
                if getattr(subprocess.run(fill_argv(argv, answer)), "returncode", 0) != 0:
                    self.say("Stopped: that step failed.")
                    break

    # -- 9 status ---------------------------------------------------------------
    def status(self):
        self.say(f"Web UI : {self.web_url()}   login: admin")
        self.say(f"Host   : {S.read('/etc/hostname')}   up: {S.sh('uptime -p')[1].strip()}   kernel {os.uname().release}")
        for svc in self.p.get("services", []):
            rc, o = S.sh(self.svc_cmd("status", svc))
            up = ("started" in o) if self.init == "openrc" else rc == 0
            self.say(f"  {svc:<16}{'running' if up else (o.strip().replace(chr(10), ' ')[:40] or 'not running')}")
        rc, o = S.sh("df -h / | tail -1")
        self.say("Root fs: " + re.sub(r"\s+", " ", o.strip()))
        rc, o = S.sh("sshd -T 2>/dev/null | grep -Ei '^(permitrootlogin|passwordauthentication) '")
        self.say("SSH    : " + o.strip().replace("\n", ", "))
        self.say("Backups: " + (", ".join(sorted(os.listdir(self.backup_dir))[-3:]) if os.path.isdir(self.backup_dir) else "none yet"))

    # -- 1 / 2 interfaces ----------------------------------------------------------
    def pick_nic(self, prompt, allow_none=False, allow_detect=True):
        nics = S.list_nics()
        if not nics:
            self.say("No network interface found. Plug in a NIC (USB Ethernet works) or check the firmware message:")
            self.say("  dmesg | grep -i firmware")
            return None
        S.bring_up(nics)
        time.sleep(1)
        for i, n in enumerate(nics, 1):
            self.say(f"  {i}) {S.nic_line(n)}")
        extra = "  d) detect by unplug/replug" if allow_detect else ""
        if allow_none:
            extra += "  n) none"
        self.say(extra.strip())
        v = self.ask(prompt).lower()
        if v == "n" and allow_none:
            return ""
        if v == "d" and allow_detect:
            wired = [n["name"] for n in nics if not n["wifi"]]
            self.say("Unplug and replug the cable of the port you want (45 s) ...")
            got = S.detect_link_change(wired, 45)
            self.say(f"detected: {got}" if got else "no link change seen")
            return got
        if v.isdigit() and 1 <= int(v) <= len(nics):
            return nics[int(v) - 1]["name"]
        if any(n["name"] == v for n in nics):
            return v
        self.say("Not a valid choice.")
        return None

    def assign(self):
        if self.p["network_mode"] == "wan-lan":
            wan0, lan0 = self.roles()
            self.say(f"Current: WAN={wan0 or '-'}  LAN={lan0 or '-'}")
            wan = self.pick_nic("WAN interface (number, name or d)")
            if wan is None:
                return
            lan = self.pick_nic("LAN interface (number, name, d or n for none)", allow_none=True)
            if lan is None:
                return
            if wan == lan:
                self.say("WAN and LAN must be different.")
                return
            opts = []
            while lan and len(S.list_nics()) > 2 + len(opts):
                o = self.pick_nic(f"OPT{len(opts) + 1} interface (number, name, d, or n when done)", allow_none=True)
                if not o:
                    break
                if o in (wan, lan) or o in opts:
                    self.say(f"{o} already has a role.")
                    continue
                opts.append(o)
            path = self.root + "etc/patronus/patronus.toml"
            txt = S.read(path)
            open(path, "w").write(apply_roles(txt, wan, lan, opts))
            S.sh(["chgrp", "patronus", path])
            S.sh(["chmod", "0640", path])
            self.say(f"WAN={wan}  LAN={lan}" + "".join(f"  OPT{i}={o}" for i, o in enumerate(opts, 1)) + " written to /etc/patronus/patronus.toml")
            sync = self.p.get("roles_sync_cmd")            # the product's command that moves the roles from that file into its config store
            if sync:
                r = subprocess.run(list(sync), capture_output=True, text=True)
                out = ((r.stdout or "") + (r.stderr or "")).strip()
                self.say(("  " + out.replace("\n", "\n  ")) if out else "  roles stored")
                if r.returncode != 0:
                    self.say("  The roles were NOT stored (see above); the file keeps them for the next boot.")
            if self.yes("Set the addresses now (option 2)?", True):
                for nm in (wan, lan, *opts):
                    if nm:
                        self.set_ip(nm)
            self.restart_services(("patronus-firewall", "patronus-applyd"))
        else:
            nm = self.pick_nic("Management interface (number, name or d)")
            if nm:
                self.set_ip(nm)

    # A restart can re-apply the whole config store (Patronus: `patronus apply` runs from the firewall service). On a loaded guest that took
    # longer than the old 120 s, and S.sh then killed rc-service half way: ruleset half loaded, web port unreachable for minutes.
    RESTART_TIMEOUT = 300

    def rc_restart(self, svc):
        """rc-service <svc> restart; one retry when it fails (the service is left in a defined state first)"""
        rc, o = S.sh(self.svc_cmd("restart", svc), timeout=self.RESTART_TIMEOUT)
        if rc != 0:
            time.sleep(3)
            rc, o = S.sh(self.svc_cmd("restart", svc), timeout=self.RESTART_TIMEOUT)
        return rc, o

    def port_listening(self):
        port = self.p.get("web_port", 8443)
        return S.sh(f"ss -ltn 2>/dev/null | grep -q ':{port} '")[0] == 0

    def wait_listening(self, timeout, interval=2, clock=time.monotonic, nap=time.sleep):
        end = clock() + timeout
        while True:
            if self.port_listening():
                return True
            if clock() >= end:
                return False
            nap(interval)

    def start_web(self, wait=120):
        """start the web service and wait until its port listens; one restart when it does not (never report success on a dead web UI)"""
        if not self.web:
            return True
        S.sh(self.svc_cmd("start", self.web), timeout=self.RESTART_TIMEOUT)
        if self.wait_listening(wait):
            return True
        self.say(f"  {self.web}: not listening after {wait} s, restarting it")
        self.rc_restart(self.web)
        return self.wait_listening(wait)

    def restart_services(self, svcs):
        for s in svcs:
            if initsys.exists(s, "/", self.init):
                rc, o = self.rc_restart(s)
                self.say(f"  {s}: " + ("restarted" if rc == 0 else "restart failed: " + o.strip()[-80:]))

    def set_ip(self, nm=None):
        nm = nm or self.pick_nic("Interface to configure (number, name or d)")
        if not nm:
            return
        nic = next((n for n in S.list_nics() if n["name"] == nm), None)
        if nic and nic["wifi"] and self.yes(f"{nm} is Wi-Fi. (Re)connect to a network?", not S.nic_ip(nm)):
            self.wifi(nm)
        mode = self.ask("IP configuration: (d)hcp or (s)tatic", "d").lower()[:1]
        if mode == "s":
            while True:
                addr = self.ask("IP address/prefix (e.g. 192.168.1.50/24)")
                e = S.cidr_problem(addr)
                if not e:
                    break
                self.say(e)
            gw = ""
            if not (self.p["network_mode"] == "wan-lan" and nm == self.roles()[1]):
                while True:
                    gw = self.ask("Gateway (empty = none)")
                    e = S.ip_problem(gw, addr) if gw else None
                    if not e:
                        break
                    self.say(e)
            dns = ""
            while gw:
                dns = self.ask("DNS server(s), space separated", gw)
                e = S.dns_problem(dns)
                if not e:
                    break
                self.say(e)
            cfg = ("static", addr, gw, dns)
        else:
            cfg = ("dhcp", "", "", "")
        v6 = {"a": "auto", "s": "static", "n": "none"}.get(self.ask("IPv6: (a)uto (router advertisements / DHCPv6), (s)tatic or (n)one", "a").lower()[:1], "auto")
        v6a = ""
        while v6 == "static":
            v6a = self.ask("IPv6 address/prefix (e.g. 2001:db8::2/64)")
            e = ipv6_problem(v6a)
            if not e:
                break
            self.say(e)
        path = self.root + DHCPCD.lstrip("/")
        txt = open(path).read() if os.path.exists(path) else ""
        open(path, "w").write(apply_iface_config(txt, nm, *cfg, v6=v6, v6_address=v6a))
        S.sh(["ip", "-4", "addr", "flush", "dev", nm])
        S.sh(["ip", "link", "set", nm, "up"])
        rc, o = S.sh(self.svc_cmd("restart", "dhcpcd"), timeout=90)
        if cfg[0] == "static":
            S.sh(["ip", "addr", "replace", cfg[1], "dev", nm])          # apply right now even if dhcpcd is slow
            if cfg[2]:
                S.sh(["ip", "route", "replace", "default", "via", cfg[2], "dev", nm])
        else:
            end = time.time() + 25
            while time.time() < end and not S.nic_ip(nm):
                time.sleep(1)
        if v6 == "static":
            S.sh(["ip", "-6", "addr", "replace", v6a, "dev", nm])
        self.say(f"{nm}: {S.nic_ip(nm) or 'no address (yet)'}" + (f"   IPv6 {v6a}" if v6 == "static" else f"   IPv6 {v6}"))

    def wifi(self, iface):
        nets = S.wifi_scan(iface)
        for i, (ss, sec, sig) in enumerate(nets, 1):
            self.say(f"  {i}) {ss}  [{sec}] {sig}")
        v = self.ask("Network number, or the SSID typed in")
        ssid = nets[int(v) - 1][0] if v.isdigit() and 1 <= int(v) <= len(nets) else v
        if not ssid:
            return
        psk = self.secret("Passphrase (empty for an open network): ")
        ok, info = S.wifi_connect(iface, ssid, psk)
        self.say("connected, " + info if ok else "failed: " + info)
        if ok:
            S.write_wifi_profile("/", ssid, psk)

    # -- 3 reset password --------------------------------------------------------------
    def new_password(self):
        for _ in range(3):
            a = self.secret("New admin password: ")
            if a == "":
                return None
            e = S.password_problem(a, S.read("/etc/hostname"))
            if e:
                self.say(e)
                continue
            if a != self.secret("Confirm: "):
                self.say("The passwords differ.")
                continue
            return a
        return None

    def set_web_password(self, pw):
        """Update admin in the product DB; return 'db' or 'file' (first-boot file when no account exists yet)."""
        if self.db and os.path.exists(self.db):
            import sqlite3
            h = argon2_encode(pw)
            con = sqlite3.connect(self.db, timeout=20)
            try:
                cur = con.execute(f"UPDATE {self.table} SET password_hash=? WHERE username='admin'", (h,))
                con.commit()
                if cur.rowcount:
                    try:
                        con.execute("DELETE FROM sessions")             # log existing sessions out (Horcrux)
                        con.commit()
                    except sqlite3.Error:
                        pass
                    return "db"
            finally:
                con.close()
        self.write_firstboot(pw)
        return "file"

    def write_firstboot(self, pw):
        pf = self.p.get("admin_password_file")
        if not pf:
            raise RuntimeError("this product has no first-boot password file")
        os.makedirs(os.path.dirname(pf), exist_ok=True)
        fd = os.open(pf, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(pw + "\n")
        owner = self.p.get("admin_password_owner", "root")
        S.sh(["chown", f"{owner}:{owner}", pf])

    def remove_firstboot_later(self):
        """the first-boot password file must not stay on disk: remove it once the web port listens (detached)"""
        pf = self.p.get("admin_password_file")
        if not pf:
            return
        port = self.p.get("web_port", 8443)
        script = (f"n=0; while [ $n -lt 300 ]; do if ss -ltn 2>/dev/null | grep -q ':{port} '; then sleep 5; rm -f {pf}; exit 0; fi; "
                  f"sleep 2; n=$((n+2)); done")
        subprocess.Popen(["sh", "-c", script], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)

    def reset_system_password(self):
        """web_auth = pam: the web UI accounts ARE the system users, so a reset is a chpasswd (sessions of that user are dropped)"""
        import pwd
        users = [u.pw_name for u in pwd.getpwall() if 1000 <= u.pw_uid < 60000 and u.pw_shell not in ("/sbin/nologin", "/usr/sbin/nologin", "/bin/false")]
        self.say("Reset the password of a system user (the web UI and the console use it). Empty input cancels.")
        self.say("Users: " + (", ".join(users) or "(none)"))
        name = self.ask("User name").strip()
        if not name or name not in users:
            self.say("Cancelled." if not name else "No such login user.")
            return
        pw = self.new_password()
        if not pw:
            self.say("Cancelled.")
            return
        rc, o = S.sh(f"printf '%s:%s\\n' {S.shquote(name)} {S.shquote(pw)} | chpasswd")
        if rc == 0:
            self.clear_login_lock(name)
        self.say(f"Password of {name} changed." if rc == 0 else "could not change it: " + o)
        if rc == 0 and self.yes("Use the same password for the root console login?", False):
            rc, o = S.sh(f"printf 'root:%s\\n' {S.shquote(pw)} | chpasswd")
            if rc == 0:
                self.clear_login_lock("root")
            self.say("root console password changed." if rc == 0 else "could not change root: " + o)

    @staticmethod
    def clear_login_lock(name):
        """A reset password must work at once. system-auth's pam_faillock locks a user for 10 min after 3 failed logins in
        15 min (no faillock.conf = the defaults), and a locked user is refused even with the new password; an administrator
        resetting a password expects that lock to be gone too. So clear the user's failure records with pam_faillock's own
        tool (absent on a PAM-less tier, where there is nothing to clear)."""
        if shutil.which("faillock"):
            S.sh(["faillock", "--user", name, "--reset"])

    def reset_password(self):
        if self.p.get("web_auth") == "pam":
            return self.reset_system_password()
        self.say("Reset the web administrator ('admin') password. Empty input cancels.")
        pw = self.new_password()
        if not pw:
            self.say("Cancelled.")
            return
        how = self.set_web_password(pw)
        if how == "file":
            self.say("No admin account exists yet: the password is applied at the next start of the web service.")
            self.restart_services((self.web,))
            self.remove_firstboot_later()
        else:
            self.say("Web admin password changed (existing web sessions were dropped).")
        if self.yes("Use the same password for the root console login?", True):
            rc, o = S.sh(f"printf 'root:%s\\n' {S.shquote(pw)} | chpasswd")
            self.say("root console password changed." if rc == 0 else "could not change root: " + o)

    # -- 4 factory reset ------------------------------------------------------------------------
    def factory_reset(self):
        self.say("RESET TO FACTORY DEFAULTS removes the web UI configuration database (users, settings, rules).")
        self.say("A backup is written to " + self.backup_dir + " first. Network roles and addresses are kept.")
        if self.ask("Type RESET to continue").strip() != "RESET":
            self.say("Cancelled.")
            return
        path = self.make_backup("pre-factory-reset")
        self.say("Backup: " + path)
        pam = self.p.get("web_auth") == "pam"
        pw = None if pam else self.new_password()
        gen = False
        if not pw and not pam:
            import secrets
            pw = secrets.token_urlsafe(18) + "9!"
            gen = True
        if self.web:
            S.sh(self.svc_cmd("stop", self.web), timeout=60)
        for f in self.reset_files():
            try:
                os.remove(f)
            except OSError:
                pass
        self.reapply()
        if not pam:
            self.write_firstboot(pw)
        up = self.start_web()
        if not pam:
            self.remove_firstboot_later()
        self.say("Factory defaults restored; the web service was restarted." if up else
                 "Factory defaults restored, but the web service is NOT listening: check /var/log and run '%s'." % " ".join(self.svc_cmd("restart", self.web)))
        if pam:
            return self.say(f"Sign in to the web UI with your system user (wheel / {self.p.get('web_admin_group', 'horcrux-admin')}); there is no separate web account.")
        self.say("Web admin password: " + (pw + "   (generated - note it now, it is not shown again)" if gen else "the one you just entered"))

    def reapply(self):
        """re-seed / re-apply what depends on the config store (Patronus: firewall + applyd), then fix ownership"""
        for svc in self.p.get("reapply_services", []):
            if initsys.exists(svc, "/", self.init):
                rc, o = self.rc_restart(svc)
                self.say(f"  {svc}: " + ("restarted" if rc == 0 else "restart failed: " + o.strip()[-80:]))
        owner = self.p.get("admin_password_owner", "root")
        if owner != "root" and self.db:
            S.sh(["chown", "-R", f"{owner}:{owner}", os.path.dirname(self.db)])

    def reset_files(self):
        db = self.db
        fs = [db, db + "-wal", db + "-shm", db + "-journal"] if db else []
        return fs + list(self.p.get("factory_reset_files", []))

    # -- 5 / 6 / 8 / 7 -----------------------------------------------------------------------------
    def poweroff(self):
        if self.yes("Power off the system now?"):
            os.sync()
            S.sh(["poweroff"])
            time.sleep(60)

    def reboot(self):
        if self.yes("Reboot the system now?"):
            os.sync()
            S.sh(["reboot"])
            time.sleep(60)

    def ping(self):
        h = self.ask("Host name or IP address to ping")
        if not host_ok(h):
            self.say("Not a valid host name or address.")
            return
        subprocess.run(["ping", "-c", "4", "-W", "2", h])

    def shell(self):
        self.say("Root shell. Type 'exit' to return to the console menu.")
        env = dict(os.environ, CONSOLE_MENU_INNER="1")
        subprocess.run(["/bin/bash", "-l"], env=env)

    # -- backups ------------------------------------------------------------------------------------
    def make_backup(self, tag="manual", dest_dir=None):
        d = dest_dir or self.backup_dir
        os.makedirs(d, mode=0o700, exist_ok=True)
        ts = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        out = os.path.join(d, f"{self.name}-config-{tag}-{ts}.tar.gz")
        stage = tempfile.mkdtemp(prefix="cfgbak-")
        try:
            with open(os.path.join(stage, "MANIFEST.json"), "w") as f:
                json.dump({"project": self.name, "created": ts, "host": S.read("/etc/hostname"), "format": 1}, f)
            members = []
            for pth in self.cfg_paths:
                if self.db and pth == self.db and os.path.exists(pth):
                    dst = stage + pth
                    os.makedirs(os.path.dirname(dst), exist_ok=True)
                    rc, o = S.sh(["sqlite3", pth, f".backup '{dst}'"], timeout=120)       # consistent copy, service keeps running
                    if rc != 0:
                        shutil.copy2(pth, dst)
                    members.append(pth.lstrip("/"))
                elif os.path.exists(pth):
                    members.append(pth.lstrip("/"))
            with tarfile.open(out, "w:gz", dereference=True) as tf:
                tf.add(os.path.join(stage, "MANIFEST.json"), "MANIFEST.json")
                for m in members:
                    src = os.path.join(stage, m) if os.path.exists(os.path.join(stage, m)) else "/" + m
                    tf.add(src, m)
            os.chmod(out, 0o600)
        finally:
            shutil.rmtree(stage, ignore_errors=True)
        return out

    def backup_cmd(self):
        out = self.make_backup()
        self.say("Backup written: " + out)
        usb = self.usb_mounts()
        if usb and self.yes("Also copy it to a USB stick?", False):
            mnt = usb[0][0] if len(usb) == 1 else usb[int(self.ask("Which one (number)", "1")) - 1][0]
            shutil.copy(out, mnt)
            os.sync()
            self.say(f"copied to {mnt}")
            self.umount_usb()

    def usb_mounts(self):
        """Mount removable/unmounted partitions read-write under /mnt/usb-N; return [(mountpoint, label)]."""
        res = []
        rc, o = S.sh(["lsblk", "-rpno", "NAME,FSTYPE,RM,MOUNTPOINT,LABEL,TYPE"])
        i = 0
        for line in o.splitlines():
            f = line.split(" ")
            if len(f) < 6 or f[5] not in ("part", "disk") or not f[1] or f[1] in ("swap", "LVM2_member", "crypto_LUKS", "zfs_member", "iso9660", "squashfs"):
                continue
            if f[3] or f[2] != "1":
                continue
            i += 1
            mnt = f"/mnt/usb-{i}"
            os.makedirs(mnt, exist_ok=True)
            if S.sh(["mount", f[0], mnt])[0] == 0:
                res.append((mnt, f[4].replace("\\x20", " ") or f[0]))
        return res

    def umount_usb(self):
        for d in os.listdir("/mnt") if os.path.isdir("/mnt") else []:
            if d.startswith("usb-"):
                S.sh(["umount", "/mnt/" + d])

    def find_backups(self):
        found = []
        import glob
        for pat in (f"{self.backup_dir}/*.tar.gz",):
            found += [(f, "local") for f in sorted(glob.glob(pat))]
        for mnt, label in self.usb_mounts():
            for f in sorted(glob.glob(f"{mnt}/*.tar.gz") + glob.glob(f"{mnt}/{self.name}*/*.tar.gz")):
                found.append((f, f"USB {label}"))
        return found

    def check_backup(self, path):
        """Return (ok, message). Only <project>-manifested archives with allowed member paths."""
        try:
            with tarfile.open(path) as tf:
                names = tf.getnames()
                man = json.load(tf.extractfile("MANIFEST.json"))
                if man.get("project") != self.name:
                    return False, f"this backup is for '{man.get('project')}', not {self.name}"
                bad = [n for n in names if not member_allowed(n, self.cfg_paths)]
                if bad:
                    return False, "contains files outside the product's config paths: " + bad[0]
                for m in tf.getmembers():
                    if m.issym() or m.islnk() or m.isdev():
                        return False, "contains a link or device node: " + m.name
        except Exception as e:  # noqa
            return False, f"not a usable backup: {e}"
        return True, "ok"

    def restore(self):
        found = self.find_backups()
        if not found:
            self.say(f"No backups found in {self.backup_dir} or on an attached USB stick (*.tar.gz).")
            return
        for i, (f, src) in enumerate(found, 1):
            self.say(f"  {i}) {os.path.basename(f)}   [{src}]")
        v = self.ask("Backup to restore (number, empty cancels)")
        if not v.isdigit() or not 1 <= int(v) <= len(found):
            self.say("Cancelled.")
            return
        path = found[int(v) - 1][0]
        ok, msg = self.check_backup(path)
        if not ok:
            self.say("Refused: " + msg)
            return
        if self.ask("This REPLACES the current configuration (a backup of it is made first). Type RESTORE").strip() != "RESTORE":
            self.say("Cancelled.")
            return
        self.say("Saved the current configuration as " + self.make_backup("pre-restore"))
        if self.web:
            S.sh(self.svc_cmd("stop", self.web), timeout=60)
        for f in self.reset_files():
            try:
                os.remove(f)
            except OSError:
                pass
        with tarfile.open(path) as tf:
            members = [m for m in tf.getmembers() if m.name != "MANIFEST.json"]
            tf.extractall("/", members=members, filter="tar")
        self.reapply()
        if self.web:
            S.sh(self.svc_cmd("start", self.web), timeout=120)
        self.umount_usb()
        self.say("Configuration restored; web service restarted.")


def main(argv=None):
    if os.geteuid() != 0:
        print("The console menu must run as root.")
        return 1
    os.environ["CONSOLE_MENU_INNER"] = "1"
    p = S.load_profile()
    return Console(p).loop()
