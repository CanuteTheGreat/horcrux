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


def dhcpcd_block(iface, mode, address="", gateway="", dns=""):
    if mode != "static":
        return f"# BEGIN console-menu {iface}\n# {iface}: DHCP (set from the console menu)\n# END console-menu {iface}\n"
    lines = [f"# BEGIN console-menu {iface}", f"interface {iface}", f"static ip_address={address}"]
    if gateway:
        lines.append(f"static routers={gateway}")
    if dns:
        lines.append("static domain_name_servers=" + " ".join(re.split(r"[\s,]+", dns.strip())))
    lines.append(f"# END console-menu {iface}")
    return "\n".join(lines) + "\n"


def apply_iface_config(text, iface, mode, address="", gateway="", dns=""):
    return strip_iface_stanza(text, iface) + "\n" + dhcpcd_block(iface, mode, address, gateway, dns)


def apply_roles(text, wan, lan):
    """Set wan_interface / lan_interface in the [network] section of patronus.toml text."""
    lines = [l for l in text.splitlines() if not re.match(r"\s*(wan_interfaces?|lan_interfaces?)\s*=", l)
             and l.strip() != "# written by live-installer"]
    roles = ["# written by live-installer / console menu"]
    if wan:
        roles.append(f'wan_interface = "{wan}"')
    if lan:
        roles.append(f'lan_interface = "{lan}"')
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

    def header(self):
        wan, lan = self.roles()
        self.say("\033[H\033[2J" if os.environ.get("TERM", "dumb") != "dumb" else "")
        self.say(f"*** {self.p['title']} - console menu ***   {S.read('/etc/hostname')}")
        self.say("")
        for n in S.list_nics():
            role = ("WAN" if n["name"] == wan else "LAN" if n["name"] == lan else "")
            ip = S.nic_ip(n["name"]) or "-"
            link = "wifi" if n["wifi"] else ("up" if S.carrier(n["name"]) else "no link")
            self.say(f" {role:<4}{n['name']:<10}{link:<8}{ip}")
        self.say("")
        self.say(f" Web UI: {self.web_url()}   (user: admin)")
        self.say("")

    MENU = [
        ("1", "Assign interfaces"), ("2", "Set interface IP address (DHCP / static)"), ("3", "Reset the admin password"),
        ("4", "Reset to factory defaults"), ("5", "Power off"), ("6", "Reboot"), ("7", "Ping host"), ("8", "Shell"),
        ("9", "Show web UI URL and status"), ("10", "Restore a configuration backup"), ("11", "Create a configuration backup"),
        ("0", "Log out"),
    ]

    def show_menu(self):
        self.header()
        for k, t in self.MENU:
            self.say(f"  {k:>2}) {t}")
        self.say("")

    def loop(self):
        acts = {"1": self.assign, "2": self.set_ip, "3": self.reset_password, "4": self.factory_reset, "5": self.poweroff,
                "6": self.reboot, "7": self.ping, "8": self.shell, "9": self.status, "10": self.restore, "11": self.backup_cmd}
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
            if c not in ("5", "6", "8"):
                self.pause()

    # -- 9 status ---------------------------------------------------------------
    def status(self):
        self.say(f"Web UI : {self.web_url()}   login: admin")
        self.say(f"Host   : {S.read('/etc/hostname')}   up: {S.sh('uptime -p')[1].strip()}   kernel {os.uname().release}")
        for svc in self.p.get("services", []):
            rc, o = S.sh(["rc-service", svc, "status"])
            self.say(f"  {svc:<16}{'running' if 'started' in o else o.strip().replace(chr(10), ' ')[:40]}")
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
            path = self.root + "etc/patronus/patronus.toml"
            txt = S.read(path)
            open(path, "w").write(apply_roles(txt, wan, lan))
            S.sh(["chgrp", "patronus", path])
            S.sh(["chmod", "0640", path])
            self.say(f"WAN={wan}  LAN={lan} written to /etc/patronus/patronus.toml")
            if self.yes("Set the addresses now (option 2)?", True):
                for nm in (wan, lan):
                    if nm:
                        self.set_ip(nm)
            self.restart_services(("patronus-firewall", "patronus-applyd"))
        else:
            nm = self.pick_nic("Management interface (number, name or d)")
            if nm:
                self.set_ip(nm)

    def restart_services(self, svcs):
        for s in svcs:
            if os.path.exists(f"/etc/init.d/{s}"):
                rc, o = S.sh(["rc-service", s, "restart"], timeout=120)
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
        path = self.root + DHCPCD.lstrip("/")
        txt = open(path).read() if os.path.exists(path) else ""
        open(path, "w").write(apply_iface_config(txt, nm, *cfg))
        S.sh(["ip", "-4", "addr", "flush", "dev", nm])
        S.sh(["ip", "link", "set", nm, "up"])
        rc, o = S.sh(["rc-service", "dhcpcd", "restart"], timeout=90)
        if cfg[0] == "static":
            S.sh(["ip", "addr", "replace", cfg[1], "dev", nm])          # apply right now even if dhcpcd is slow
            if cfg[2]:
                S.sh(["ip", "route", "replace", "default", "via", cfg[2], "dev", nm])
        else:
            end = time.time() + 25
            while time.time() < end and not S.nic_ip(nm):
                time.sleep(1)
        self.say(f"{nm}: {S.nic_ip(nm) or 'no address (yet)'}")

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

    def reset_password(self):
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
        pw = self.new_password()
        gen = False
        if not pw:
            import secrets
            pw = secrets.token_urlsafe(18) + "9!"
            gen = True
        if self.web:
            S.sh(["rc-service", self.web, "stop"], timeout=60)
        for f in self.reset_files():
            try:
                os.remove(f)
            except OSError:
                pass
        self.reapply()
        self.write_firstboot(pw)
        if self.web:
            S.sh(["rc-service", self.web, "start"], timeout=120)
        self.remove_firstboot_later()
        self.say("Factory defaults restored; the web service was restarted.")
        self.say("Web admin password: " + (pw + "   (generated - note it now, it is not shown again)" if gen else "the one you just entered"))

    def reapply(self):
        """re-seed / re-apply what depends on the config store (Patronus: firewall + applyd), then fix ownership"""
        for svc in self.p.get("reapply_services", []):
            if os.path.exists(f"/etc/init.d/{svc}"):
                rc, o = S.sh(["rc-service", svc, "restart"], timeout=120)
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
            S.sh(["rc-service", self.web, "stop"], timeout=60)
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
            S.sh(["rc-service", self.web, "start"], timeout=120)
        self.umount_usb()
        self.say("Configuration restored; web service restarted.")


def main(argv=None):
    if os.geteuid() != 0:
        print("The console menu must run as root.")
        return 1
    os.environ["CONSOLE_MENU_INNER"] = "1"
    p = S.load_profile()
    return Console(p).loop()
