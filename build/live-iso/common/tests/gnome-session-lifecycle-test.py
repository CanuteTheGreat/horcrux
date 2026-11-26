#!/usr/bin/env python3
"""GNOME-on-OpenRC session lifecycle test: logins/logouts at the real GDM greeter while ssh sessions of the same user come and go.

Why: on OpenRC, gnome-session-openrc runs gnome-shell & co. as OpenRC USER services. The user runlevel (service user.<name>, started by
pam_openrc) is shared by every login of the user and only stops with the LAST one. An ssh login of the desktop user (agents, scripts) kept it
alive across a GNOME logout, supervise-daemon respawned gnome-shell into the closed session, that shell held the seat, and the next login got
mutter "Failed to take control of the session: ... EBUSY" and no mouse/keyboard (owner-ws, 2026-11-13). The fix lives in
gnome-base/gnome-session-openrc-50.0-r2::horcrux (session-scoped user services) plus sys-auth/horcrux-ssh-pam (ssh logins start no OpenRC user
services). This test reproduces the owner's sequence on an installed GNOME VM:

    cycle N:  GNOME login at the greeter (QMP key events)  ->  20 parallel `ssh USER@vm true` + one ssh session of USER held open
              ->  GNOME logout (gnome-session-quit, i.e. the menu)  ->  checks  ->  next cycle logs in again

Per cycle it asserts:
  * after the logout no gnome-shell of the user survives or gets respawned (the stale compositor that held the seat);
  * after the login the new session is seat0's active session, and exactly one user gnome-shell runs, in THAT session;
  * no "Failed to take control of the session" in syslog since the cycle started (mutter EBUSY);
  * the input devices (/dev/input/event*) are open in the active session's compositor and in no other gnome-shell of the user;
  * no duplicated user supervisors (two `supervise-daemon dbus` of the user, two `supervise-daemon user.USER`, ...);
  * screen sharing works: xdg-desktop-portal answers org.freedesktop.portal.ScreenCast AvailableSourceTypes with a non-zero value (it only
    has the GNOME backend when the session's XDG_CURRENT_DESKTOP & co. reached the user bus's D-Bus activation environment);
  * autostarted apps get the session's desktop identity: an autostart probe (written by the test) has XDG_CURRENT_DESKTOP=GNOME-ish,
    XDG_SESSION_DESKTOP and DESKTOP_SESSION in its environment, an entry with NotShowIn=GNOME does NOT start (nm-applet's case), and
    neither does an installed /etc/xdg/autostart/nm-applet.desktop.

Attach mode only: the VM must already run an installed GNOME edition (e.g. installed with qemu-test.py --flow auto and
tests/data/gnome-session-answers.toml), reachable over ssh as root with --key, with a QMP socket for key events. The test sets the login user's
password (test-only value) and authorises --key for that user.

    gnome-session-lifecycle-test.py --qmp /w/qmp.sock --port 22790 --key k [--cycles 3] [--user owner] [--out DIR]

Exit 0 = PASS (every check of every cycle), 1 = FAIL. A JSON summary goes to OUT/result.json, screenshots on failure to OUT/.
"""
import argparse
import collections
import json
import math
import os
import re
import shlex
import socket
import subprocess
import sys
import time

shq = shlex.quote

LOGIN_PASSWORD = "Gsl-Login-Test-4711"  # test-only, never a real secret; differs from every other harness password
_SHIFT = {c: k for c, k in zip('!@#$%^&*()_+{}|:"<>?~', "1234567890-=[]\\;',./`")}
_PLAIN = {" ": "spc", "-": "minus", "=": "equal", "[": "bracket_left", "]": "bracket_right", "\\": "backslash", ";": "semicolon",
          "'": "apostrophe", ",": "comma", ".": "dot", "/": "slash", "`": "grave_accent", "\n": "ret", "\t": "tab"}


class VM:
    def __init__(self, a):
        self.a = a
        self.base = ["ssh", "-i", a.key, "-p", str(a.port), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
                     "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=15", "-o", "BatchMode=yes"]

    def ssh_argv(self, user, cmd, loglevel="ERROR"):
        return [x if x != "LogLevel=ERROR" else f"LogLevel={loglevel}" for x in self.base] + [f"{user}@{self.a.host}", cmd]

    def sh(self, cmd, user="root", timeout=120, inp=None):
        r = subprocess.run(self.ssh_argv(user, cmd), capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout, input=inp)
        return r.returncode, (r.stdout + r.stderr).strip()

    def out(self, cmd, **kw):
        return self.sh(cmd, **kw)[1]

    def qmp(self, cmd, **kw):
        s = socket.socket(socket.AF_UNIX)
        s.connect(self.a.qmp)
        f = s.makefile("rw")
        f.readline()
        f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n"); f.flush(); f.readline()
        f.write(json.dumps({"execute": cmd, "arguments": kw}) + "\n"); f.flush()
        while True:  # skip async events
            r = json.loads(f.readline())
            if "return" in r or "error" in r:
                break
        s.close()
        return r

    def key(self, *names):
        self.qmp("send-key", keys=[{"type": "qcode", "data": n} for n in names])

    def type(self, text):
        for ch in text:
            if ch.isalpha():
                self.key(*((["shift"] if ch.isupper() else []) + [ch.lower()]))
            elif ch.isdigit():
                self.key(ch)
            elif ch in _PLAIN:
                self.key(_PLAIN[ch])
            else:
                b = _SHIFT[ch]
                self.key("shift", b if b.isdigit() else _PLAIN[b])
            time.sleep(0.06)

    def shot(self, name):
        try:
            os.makedirs(self.a.out, exist_ok=True)
            self.qmp("screendump", filename=os.path.join(self.a.vm_out or self.a.out, name), format="png")
        except Exception as e:  # noqa: BLE001 - a screenshot is evidence, never a reason to abort
            print(f"  (screenshot {name} failed: {e})")


# ---------------------------------------------------------------- state readers (all run as root in the VM)
def uid_of(vm, user):
    return int(vm.out(f"id -u {user}"))


def seat_sessions(vm, uid):
    """[(id, state, class, type)] of the user's sessions on seat0"""
    res = []
    for line in vm.out("loginctl list-sessions --no-legend").splitlines():
        f = line.split()
        if len(f) >= 4 and f[1] == str(uid) and f[3] == "seat0":
            p = dict(kv.split("=", 1) for kv in vm.out(f"loginctl show-session {f[0]} -p State -p Class -p Type").splitlines() if "=" in kv)
            res.append((f[0], p.get("State"), p.get("Class"), p.get("Type")))
    return res


def active_session(vm):
    return vm.out("loginctl show-seat seat0 -p ActiveSession --value")


def user_shells(vm, uid):
    """{pid: session id from its environment} for the user's gnome-shell processes (the greeter runs as gdm-greeter, never counted)"""
    shells = {}
    for pid in vm.out(f"pgrep -u {uid} -x gnome-shell || true").split():
        env = vm.out(f"tr '\\0' '\\n' </proc/{pid}/environ 2>/dev/null | sed -n 's/^XDG_SESSION_ID=//p' | head -n1")
        shells[pid] = env or "?"
    return shells


def input_holders(vm):
    """{pid: [event devices]} of every process holding /dev/input/event*"""
    o = vm.out("for p in /proc/[0-9]*; do l=$(ls -l $p/fd 2>/dev/null | grep -o '/dev/input/event[0-9]*' | sort -u | tr '\\n' ' '); "
               "[ -n \"$l\" ] && echo \"${p#/proc/} $(cat $p/comm) $l\"; done; true")
    res = {}
    for line in o.splitlines():
        f = line.split()
        if len(f) >= 3:
            res[f[0]] = (f[1], f[2:])
    return res


def supervisors(vm, uid, user):
    """duplicated supervise-daemon instances: the user's own (dbus, gnome-shell-*, gsd-*, ...) and root's user.USER"""
    dup = {}
    c = collections.Counter(re.findall(r"^supervise-daemon (\S+)", vm.out(f"ps -u {uid} -o args= || true"), re.M))
    dup.update({k: v for k, v in c.items() if v > 1})
    n = len(re.findall(rf"^supervise-daemon user\.{re.escape(user)} ", vm.out("ps -u 0 -o args= || true"), re.M))
    if n > 1:
        dup[f"user.{user}"] = n
    return dup


def syslog_since(vm, marker):
    # sysklogd rotates /var/log/messages by size (-r): read the previous file too
    return vm.out(f"{{ cat /var/log/messages.0 /var/log/messages 2>/dev/null; journalctl --no-pager -o short 2>/dev/null; }} | sed -n '/{marker}/,$p'")


# ---------------------------------------------------------------- the test
class Run:
    def __init__(self, a):
        self.a, self.vm = a, VM(a)
        self.results, self.failed = [], False

    def check(self, cycle, name, ok, detail=""):
        self.results.append({"cycle": cycle, "check": name, "ok": bool(ok), "detail": detail})
        print(f"[{'PASS' if ok else 'FAIL'}] c{cycle} {name} {detail}", flush=True)
        if not ok:
            self.failed = True
            self.vm.shot(f"fail-c{cycle}-{name}.png")
        return ok

    def setup(self):
        vm, a = self.vm, self.a
        rc, o = vm.sh(f"chpasswd", inp=f"{a.user}:{LOGIN_PASSWORD}\n")
        if rc:
            sys.exit(f"chpasswd failed: {o}")
        pub = subprocess.run(["ssh-keygen", "-y", "-f", a.key], capture_output=True, text=True, check=True).stdout.strip()
        vm.sh(f"install -d -m700 -o {a.user} -g {a.user} ~{a.user}/.ssh; grep -qxF '{pub}' ~{a.user}/.ssh/authorized_keys 2>/dev/null || "
              f"echo '{pub}' >> ~{a.user}/.ssh/authorized_keys; chown {a.user}: ~{a.user}/.ssh/authorized_keys; chmod 600 ~{a.user}/.ssh/authorized_keys")
        self.uid = uid_of(vm, a.user)
        # init system of the installed image: the OpenRC-only parts (session-scoped OpenRC user services, the gnome-session-openrc helper,
        # sysklogd) are checked only there; systemd gets the equivalent checks through logind/journald/processes
        self.systemd = vm.out("test -d /run/systemd/system && echo yes || echo no").strip() == "yes"
        print(f"init: {'systemd' if self.systemd else 'openrc'}", flush=True)
        # autostart probes: the first dumps its environment, the second must never start in GNOME (NotShowIn=GNOME, like nm-applet)
        probes = {"gsl-probe": "env > /tmp/gsl-probe.env; exec sleep 100001", "gsl-notshowin": "touch /tmp/gsl-notshowin.ran; exec sleep 100001"}
        vm.sh(f"install -d -o {a.user} -g {a.user} ~{a.user}/.config ~{a.user}/.config/autostart")
        for name, cmd in probes.items():
            extra = "NotShowIn=KDE;GNOME;\n" if "notshowin" in name else ""
            vm.sh(f"cat > ~{a.user}/.config/autostart/{name}.desktop; chown {a.user}: ~{a.user}/.config/autostart/{name}.desktop",
                  inp=f"[Desktop Entry]\nType=Application\nName={name}\n{extra}Exec=sh -c '{cmd}'\n")
        self.versions = vm.out("qlist -Iv gnome-base/gnome-session-openrc sys-auth/horcrux-ssh-pam sys-apps/openrc gnome-base/gdm 2>/dev/null; "
                               "sshd -T 2>/dev/null | grep -i '^pamservicename'")
        print("versions:\n  " + self.versions.replace("\n", "\n  "), flush=True)

    def wait(self, pred, timeout, step=2):
        end = time.time() + timeout
        while time.time() < end:
            v = pred()
            if v:
                return v
            time.sleep(step)
        return pred()

    def greeter_up(self):
        o = self.vm.out("loginctl list-sessions --no-legend")
        return any(l.split()[2:4] == ["gdm-greeter", "seat0"] for l in o.splitlines() if len(l.split()) >= 4) and \
            active_session(self.vm) in [l.split()[0] for l in o.splitlines() if "gdm-greeter" in l]

    def new_user_session(self, before):
        for sid, state, cls, typ in seat_sessions(self.vm, self.uid):
            if sid not in before and state == "active" and cls == "user":
                return sid
        return None

    def login(self, cycle):
        vm = self.vm
        if not self.wait(self.greeter_up, 120):
            self.check(cycle, "greeter-up", False, "no active gdm-greeter session on seat0 within 120 s")
            return None
        before = {s[0] for s in seat_sessions(vm, self.uid)}
        for attempt in range(3):
            vm.key("shift"); time.sleep(2)       # wake a blanked greeter
            vm.key("ret"); time.sleep(3)         # select the (only) listed user
            vm.type(LOGIN_PASSWORD); time.sleep(0.5)
            vm.key("ret")
            sid = self.wait(lambda: self.new_user_session(before), 45)
            if sid:
                return sid
            vm.shot(f"login-c{cycle}-try{attempt}.png")
            vm.key("esc"); time.sleep(3)
        self.check(cycle, "login", False, "no new active seat0 session of the user after 3 tries at the greeter")
        return None

    def ssh_burst(self):
        """20 parallel `ssh USER@vm true`. sshd's MaxStartups (default 10:30:100) randomly drops connections while more than 10 are
        unauthenticated: such a drop ("Connection closed"/"reset" before auth) is retried, every other failure counts."""
        procs = [subprocess.Popen(self.vm.ssh_argv(self.a.user, "true", loglevel="INFO"), stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                  text=True, encoding="utf-8", errors="replace") for _ in range(20)]
        ok, retried, errors = 0, 0, []
        for p in procs:
            err = p.communicate(timeout=120)[1].strip()
            rc = p.returncode
            tries = 0
            while rc != 0 and tries < 3 and re.search(r"Connection (closed|reset)|kex_exchange_identification", err):
                tries += 1
                retried += 1
                time.sleep(1)
                rc, err = self.vm.sh("true", user=self.a.user)
            if rc == 0:
                ok += 1
            else:
                errors.append(f"rc={rc} {err[-160:]}")
        return ok, retried, errors

    def logout(self, sid):
        vm, u = self.vm, self.a.user
        vm.sh(f"su - {u} -c 'DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{self.uid}/bus timeout 20 gnome-session-quit --logout --no-prompt' "
              f">/dev/null 2>&1 &", timeout=60)
        gone = lambda: vm.out(f"loginctl show-session {sid} -p State --value 2>/dev/null || echo gone") in ("gone", "closing", "")  # noqa: E731
        if self.wait(gone, 45):
            return "gnome-session-quit"
        # the menu path did not end the session: end it the way GDM/logind does
        vm.sh(f"loginctl terminate-session {sid}")
        return "terminate-session" if self.wait(gone, 30) else "still-open"

    def cycle(self, n):
        vm, a = self.vm, self.a
        marker = f"GSLTEST-{os.getpid()}-{n}"
        vm.sh(f"logger -t gsl-test {marker}; rm -f /tmp/gsl-probe.env /tmp/gsl-notshowin.ran; pkill -f 'sleep 10000[1]' || true")
        sid = self.login(n)
        if not sid:
            return
        print(f"  c{n}: logged in, session {sid}", flush=True)
        time.sleep(20)  # let the session's services come up
        self.verify_session(n, sid, marker)
        ok, retried, errors = self.ssh_burst()
        self.check(n, "ssh-burst", ok == 20, f"{ok}/20 parallel `ssh {a.user}@vm true` succeeded ({retried} retried after sshd MaxStartups drops)"
                   + (f"; errors: {errors[:3]}" if errors else ""))
        # an agent/script session of the same user that stays open across the GNOME logout (what kept the runlevel alive on owner-ws)
        held = subprocess.Popen(vm.ssh_argv(a.user, "sleep 100000"), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(3)
        self.check(n, "ssh-held-open", held.poll() is None, "a held ssh session of the user is open during the logout")
        lmark = f"GSLTEST-{os.getpid()}-{n}-logout"
        vm.sh(f"logger -t gsl-test {lmark}")
        how = self.logout(sid)
        self.check(n, "logout", how != "still-open", f"session {sid} ended via {how}")
        time.sleep(15)  # supervise-daemon respawns within seconds
        respawn = [l for l in syslog_since(vm, lmark).splitlines() if re.search(rf"\b{re.escape(a.user)}: .*Running GNOME Shell", l)]
        self.check(n, "no-shell-respawn-after-logout", not respawn,
                   f"{len(respawn)} gnome-shell (re)starts of {a.user} after the logout" + (f", e.g. {respawn[-1][:160]}" if respawn else ""))
        shells = user_shells(vm, self.uid)
        self.check(n, "no-stale-shell-after-logout", not shells, f"user gnome-shell after logout: {shells or 'none'}")
        self.check(n, "no-duplicate-supervisors-after-logout", not supervisors(vm, self.uid, a.user),
                   f"{supervisors(vm, self.uid, a.user) or 'none'}")
        self.held.append(held)

    def verify_session(self, n, sid, marker):
        vm, a = self.vm, self.a
        act = active_session(vm)
        self.check(n, "session-is-active-on-seat0", act == sid, f"seat0 ActiveSession={act}, new session={sid}")
        shells = self.wait(lambda: user_shells(vm, self.uid) or None, 30) or {}
        if self.systemd:
            # gnome-shell is a unit of the user manager here (its environ has no XDG_SESSION_ID): it belongs to the new session when it
            # started after that session was created, anything older is a leftover of a previous login
            t0 = vm.out(f"date -d \"$(loginctl show-session {sid} -p Timestamp --value)\" +%s 2>/dev/null || echo 0").strip() or "0"
            started = {p: int(vm.out(f"stat -c %Y /proc/{p} 2>/dev/null || echo 0").strip() or 0) for p in shells}
            mine = [p for p in shells if started[p] >= int(t0) - 5]
            stale = {p: "started before the session" for p in shells if p not in mine}
        else:
            mine = [p for p, s in shells.items() if s == sid]
            stale = {p: s for p, s in shells.items() if s != sid}
        self.check(n, "one-shell-in-new-session", len(mine) == 1, f"gnome-shell of session {sid}: {mine}")
        self.check(n, "no-stale-shell", not stale, f"gnome-shell of other sessions: {stale or 'none'}")
        log = syslog_since(vm, marker)
        bad = [l for l in log.splitlines() if "Failed to take control of the session" in l or ("EBUSY" in l and "gnome-shell" in l)]
        self.check(n, "seat-control", not bad, (bad[0][:200] if bad else "no 'Failed to take control of the session' since the cycle began"))
        holders = input_holders(vm)
        ev_mine = sorted({d for p, (c, ds) in holders.items() if p in mine for d in ds})
        other_shells = {p: ds for p, (c, ds) in holders.items() if c == "gnome-shell" and p not in mine}
        self.check(n, "input-in-active-compositor", bool(ev_mine) and not other_shells,
                   f"active compositor holds {ev_mine or 'NO input devices'}; other gnome-shell holding input: {other_shells or 'none'}")
        dup = supervisors(vm, self.uid, a.user)
        self.check(n, "no-duplicate-supervisors", not dup, f"{dup or 'none'}")
        # screen sharing: xdg-desktop-portal must have picked the GNOME backend, which needs XDG_CURRENT_DESKTOP & co. in the user bus's
        # activation environment (owner-ws 2026-11-13: no ScreenCast interface, OBS/browser screen sharing broken)
        cast = self.wait(lambda: self.screencast_types() or None, 40, step=4)
        self.check(n, "portal-screencast", bool(cast), f"org.freedesktop.portal.ScreenCast AvailableSourceTypes={cast or self.last_cast}")
        self.verify_autostart_env(n)
        self.verify_x11_ui_scale(n)

    # X11 apps under Xwayland at fractional scales (owner-ws 2026-11-23: Steam/Qt apps tiny at 133 %). mutter 50 always scales Xwayland
    # natively (X screen = ceil(scale) x logical size); X11 clients render at the right size only when gsd-xsettings publishes that factor.
    # For each target scale the monitor is set (persistently: the temporary method makes gnome-shell revert after its "keep changes?"
    # countdown; the original scale is restored at the end) to the closest scale mutter supports for its mode, then X11 clients
    # are checked: gsd-xsettings serves the display, Xft.dpi = 96 x UiScalingFactor, GTK3 on X11 reads the same factor from XSETTINGS, and
    # the X screen is logical size x factor.
    X11_SCALES = (1.25, 1.333, 1.5, 2.0)
    SCALE_PROBE = r'''
import json, math, sys, gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib
want = float(sys.argv[1])
bus = Gio.bus_get_sync(Gio.BusType.SESSION)
dc = Gio.DBusProxy.new_sync(bus, 0, None, "org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig", "org.gnome.Mutter.DisplayConfig")
serial, monitors, logical, props = dc.call_sync("GetCurrentState", None, 0, 10000).unpack()
mode_of, scales = {}, {}
for (conn, vendor, product, ser), modes, mprops in monitors:
    for mid, w, h, rate, pref, sup, mp in modes:
        if mp.get("is-current"):
            mode_of[conn] = (mid, w, h)
            scales[conn] = sup
lm_out, applied = [], []
for x, y, scale, transform, primary, mons, lp in logical:
    conn = mons[0][0]
    sup = scales.get(conn) or [scale]
    s = min(sup, key=lambda v: abs(v - want))
    applied.append(s)
    lm_out.append((x, y, s, transform, primary, [(c[0], mode_of[c[0]][0], {}) for c in mons]))
dc.call_sync("ApplyMonitorsConfig", GLib.Variant("(uua(iiduba(ssa{sv}))a{sv})", (serial, 2, lm_out, {})), 0, 10000)
w, h = mode_of[logical[0][5][0][0]][1:]
print(json.dumps({"applied": applied, "mode": [w, h], "was": logical[0][2]}))
'''
    # GTK3 on the X11 backend: its scale factor (from XSETTINGS) and the monitor size in X pixels (geometry x scale)
    GDK_PROBE = ('import gi; gi.require_version("Gdk", "3.0"); from gi.repository import Gdk; d = Gdk.Display.get_default(); '
                 'm = d.get_monitor(0) if d else None; g = m.get_geometry() if m else None; '
                 'print("%d %d %d" % (m.get_scale_factor(), g.width * m.get_scale_factor(), g.height * m.get_scale_factor()) if m else "no-display")')

    XFT_PROBE = ('import gi; gi.require_version("Gtk", "3.0"); from gi.repository import Gtk; '
                 's = Gtk.Settings.get_default(); print(round(s.get_property("gtk-xft-dpi") / 1024) if s else "no-settings")')

    def diag_x11(self, n, disp, xenv):
        """Failure diagnostics for the X11 scale checks (printed once): which Xwayland/gsd-xsettings run, on which display, and what each X display says."""
        vm = self.vm
        cmds = [
            "ps -eo pid,etimes,user,args | grep -E '[X]wayland|[g]sd-xsettings|[g]nome-shell' | cut -c1-220",
            "for p in $(pgrep -x Xwayland) $(pgrep -x gsd-xsettings); do echo pid $p $(cat /proc/$p/comm): $(xargs -0 -n1 echo < /proc/$p/cmdline | tr '\\n' ' ' | cut -c1-160); xargs -0 -n1 echo < /proc/$p/environ 2>/dev/null | grep -E '^(DISPLAY|XAUTHORITY|WAYLAND_DISPLAY)='; done",
            "ls -l /tmp/.X11-unix /tmp/.X*-lock 2>&1",
        ]
        ucmds = [
            "systemctl --user show-environment | grep -E '^(DISPLAY|XAUTHORITY)='",
            "systemctl --user list-units --all --no-pager 'org.gnome.SettingsDaemon.XSettings*' 'gnome-session-x11*' 2>&1 | cut -c1-200",
            "journalctl --user -b --no-pager 2>&1 | grep -iE 'xwayland|xsettings|x11 services|native scal' | tail -40 | cut -c1-260",
            "gsettings get org.gnome.mutter experimental-features; gsettings get org.gnome.desktop.interface scaling-factor",
        ]
        for c in cmds:
            print(f"DIAG-x11 c{n} root$ {c[:60]}\n" + vm.out(c).strip()[-1500:], flush=True)
        for c in ucmds:
            print(f"DIAG-x11 c{n} user$ {c[:60]}\n" + self.as_user(c).strip()[-2500:], flush=True)
        for d in (":0", ":1", ":2"):
            r = self.as_user(f"DISPLAY={d} XAUTHORITY=$(xargs -0 -n1 echo < /proc/$(pgrep -u {self.uid} -x Xwayland | head -n1)/cmdline | sed -n '/^-auth$/{{n;p;q}}') GDK_BACKEND=x11 timeout 20 python3 -c {shq(self.XFT_PROBE)} 2>&1 | tail -2")
            print(f"DIAG-x11 c{n} xft-probe {d}: {r.strip()[-200:]}", flush=True)

    def as_user(self, cmd, timeout=60):
        a = self.a
        env = f"XDG_RUNTIME_DIR=/run/user/{self.uid} DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{self.uid}/bus"
        return self.vm.out(f"su - {a.user} -c {shq(env + ' ' + cmd)} 2>&1", timeout=timeout)

    def verify_x11_ui_scale(self, n):
        vm = self.vm
        hp = "/usr/libexec/gnome-session-openrc-helper"
        if self.systemd:
            # no helper on systemd: mutter starts Xwayland on demand (first X11 client), so there may be no Xwayland process yet.
            # The display name is in the user manager's environment (gnome-shell imports DISPLAY); an X client then starts Xwayland.
            def getdisp():
                o = self.as_user("systemctl --user show-environment | grep -m1 '^DISPLAY='").strip().splitlines()
                d = o[-1].split("=", 1)[1].strip() if o and "=" in o[-1] else ""
                if not d.startswith(":"):
                    d = vm.out(f"tr '\\0' '\\n' </proc/$(pgrep -u {self.uid} -x Xwayland | head -n1)/cmdline 2>/dev/null | grep -m1 '^:[0-9]'").strip()
                if not d and vm.out(f"pgrep -u {self.uid} -x gnome-shell | head -n1").strip():
                    d = ":0"  # mutter's default socket, Xwayland not started yet
                if not d.startswith(":"):
                    # mutter listens on the X socket even while no Xwayland runs
                    d = vm.out(f"for f in /tmp/.X11-unix/X*; do [ -S \"$f\" ] && [ \"$(stat -c %u \"$f\")\" = {self.uid} ] && {{ echo \":${{f##*/X}}\"; break; }}; done").strip()
                return d or None
        else:
            getdisp = lambda: self.as_user(f"{hp} x-display") or None  # noqa: E731
        disp = self.wait(getdisp, 40, step=3) or ""
        if not self.check(n, "x11-xwayland-display", disp.startswith(":"), f"Xwayland display of the session: {disp!r} (X sockets: {vm.out('ls -l /tmp/.X11-unix 2>&1').strip()[:200]!r}; user env: {self.as_user('systemctl --user show-environment | grep -E ^DISPLAY')[:80]!r})"):
            return
        if self.systemd:
            # start Xwayland by connecting an X11 client (GTK3 on the X11 backend), then wait for the process
            self.as_user(f"GDK_BACKEND=x11 DISPLAY={disp} python3 -c {shq(self.GDK_PROBE)}")
            self.wait(lambda: vm.out(f"pgrep -u {self.uid} -x Xwayland | head -n1").strip() or None, 40, step=2)
        # gnome-settings-daemon wants the gsd-xsettings service: the SERVICE STATE must be started, not only the process
        # alive (seen on a real session: supervise-daemon ran it while rc-service printed "eend: command not found", reported "failed to
        # start" and left no started/ symlink, so `rc-service -U gsd-xsettings status` said stopped and the next restart/stop skipped it)
        if self.systemd:
            svc = self.wait(lambda: vm.out(f"pgrep -u {self.uid} -x gsd-xsettings | head -n1").strip() or None, 40, step=3) or ""
            self.check(n, "x11-gsd-xsettings-service-started", bool(svc), f"gsd-xsettings process (systemd user manager): {svc or 'none'}")
        else:
            svc = self.wait(lambda: (lambda o: o if "started" in o else None)(self.as_user("rc-service -U gsd-xsettings status")), 40, step=3)
            svc = svc or self.as_user("rc-service -U gsd-xsettings status")
            self.check(n, "x11-gsd-xsettings-service-started", "started" in svc and "stopped" not in svc and "crashed" not in svc,
                       f"rc-service -U gsd-xsettings status: {svc.strip()[-160:]!r}")
        auth = vm.out(f"tr '\\0' '\\n' </proc/$(pgrep -u {self.uid} -x Xwayland | head -n1)/cmdline | sed -n '/^-auth$/{{n;p;q}}'")
        xenv = f"DISPLAY={disp} XAUTHORITY={auth}"
        vm.sh(f"cat > /tmp/gsl-scale-probe.py; chmod 644 /tmp/gsl-scale-probe.py", inp=self.SCALE_PROBE)
        orig = None
        try:
            for want in self.X11_SCALES:
                o = self.as_user(f"python3 /tmp/gsl-scale-probe.py {want}")
                try:
                    st = json.loads(o.strip().splitlines()[-1])
                except (ValueError, IndexError):
                    self.check(n, f"x11-scale-{want}-apply", False, f"ApplyMonitorsConfig failed: {o[-300:]}")
                    continue
                if orig is None:
                    orig = st["was"]
                s = st["applied"][0]
                f = math.ceil(s - 1e-6)
                time.sleep(4)  # mutter -> UiScalingFactor -> gsd-xsettings -> XSETTINGS/xrdb
                if self.systemd:
                    # same verdict as the helper's: the X server serves XSETTINGS with Xft.dpi = 96 x factor (read through GTK3 on X11)
                    xq = lambda: self.as_user(f"{xenv} GDK_BACKEND=x11 python3 -c {shq(self.XFT_PROBE)}").strip().splitlines()  # noqa: E731
                    want_dpi = 96 * f
                    stat = self.wait(lambda: (lambda o: f"OK Xft.dpi={want_dpi} (X11 UI scale {f}, text scaling 1.0)" if o and o[-1] == str(want_dpi) else None)(xq()), 20, step=2)
                    stat = stat or f"FAIL Xft.dpi={(xq() or ['?'])[-1]}"
                else:
                    stat = self.wait(lambda: (lambda o: o if "FAIL" not in o and "OK Xft.dpi" in o else None)(
                        self.as_user(f"{xenv} {hp} x11-scale-status")), 20, step=2) or self.as_user(f"{xenv} {hp} x11-scale-status")
                if "FAIL" in stat and not getattr(self, "_x11_diag_done", False):
                    self._x11_diag_done = True
                    self.diag_x11(n, disp, xenv)
                self.check(n, f"x11-scale-{want}-xsettings", "FAIL" not in stat and "OK Xft.dpi" in stat and f"X11 UI scale {f}," in stat,
                           f"monitor scale {s:.4f} (want {want}) -> {stat!r}")
                gdk = self.as_user(f"{xenv} GDK_BACKEND=x11 python3 -c {shq(self.GDK_PROBE)}").strip().splitlines()
                try:
                    gf, xw, xh = (int(v) for v in gdk[-1].split())
                except (ValueError, IndexError):
                    gf = xw = xh = 0
                self.check(n, f"x11-scale-{want}-gtk3-x11-client", gf == f, f"GTK3 X11 client scale factor {gf or (gdk[-1] if gdk else '?')} (expected {f})")
                # mutter 50 scales Xwayland natively: the X screen is the logical size x ceil(scale)
                lw, lh = (round(st["mode"][0] / s), round(st["mode"][1] / s))
                self.check(n, f"x11-scale-{want}-x-screen", abs(xw - lw * f) <= f * 2 and abs(xh - lh * f) <= f * 2,
                           f"X monitor {xw}x{xh}, logical {lw}x{lh} x factor {f}")
        finally:
            # back to the scale the session started with
            if orig is not None:
                self.as_user(f"python3 /tmp/gsl-scale-probe.py {orig}")

    def verify_autostart_env(self, n):
        vm = self.vm
        env = self.wait(lambda: vm.out("cat /tmp/gsl-probe.env 2>/dev/null"), 40, step=3) or ""
        kv = dict(l.split("=", 1) for l in env.splitlines() if "=" in l)
        cur = kv.get("XDG_CURRENT_DESKTOP", "")
        self.check(n, "autostart-xdg-current-desktop", "GNOME" in cur.split(":"), f"autostarted app sees XDG_CURRENT_DESKTOP={cur!r}")
        self.check(n, "autostart-xdg-session-desktop", bool(kv.get("XDG_SESSION_DESKTOP")) and bool(kv.get("DESKTOP_SESSION")),
                   f"XDG_SESSION_DESKTOP={kv.get('XDG_SESSION_DESKTOP')!r} DESKTOP_SESSION={kv.get('DESKTOP_SESSION')!r}")
        ran = vm.out("test -e /tmp/gsl-notshowin.ran && echo yes || echo no")
        self.check(n, "autostart-notshowin-gnome-honoured", ran == "no", f"NotShowIn=GNOME probe started: {ran}")
        nm = vm.out(f"test -e /etc/xdg/autostart/nm-applet.desktop && {{ pgrep -u {self.uid} -x nm-applet || echo none; }} || echo not-installed")
        self.check(n, "nm-applet-not-autostarted", nm in ("none", "not-installed"), f"nm-applet: {nm}")
        # gnome-session-service itself (the parent of everything it autostarts) carries the identity too
        # (the wrapper script shares the 15-character comm "gnome-session-s": look at the real binary, whose argv[0] is not the wrapper)
        gss = vm.out(f"for p in $(pgrep -u {self.uid} -x gnome-session-s); do tr '\\0' ' ' </proc/$p/cmdline | grep -q wrapper && continue; "
                     f"tr '\\0' '\\n' </proc/$p/environ | grep -c '^XDG_CURRENT_DESKTOP=' ; done | head -n1")
        self.check(n, "gnome-session-service-env", gss == "1", f"gnome-session-service has XDG_CURRENT_DESKTOP in its environ: {gss or 'process not found'}")

    def screencast_types(self):
        o = self.vm.out(f"su - {self.a.user} -c 'DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{self.uid}/bus timeout 20 gdbus call --session "
                        "-d org.freedesktop.portal.Desktop -o /org/freedesktop/portal/desktop -m org.freedesktop.DBus.Properties.Get "
                        "org.freedesktop.portal.ScreenCast AvailableSourceTypes' 2>&1", timeout=60)
        self.last_cast = o[-200:] or "(no output)"
        m = re.search(r"uint32 (\d+)", o)
        return int(m.group(1)) if m and int(m.group(1)) else 0

    def main(self):
        self.held = []
        self.setup()
        try:
            for n in range(1, self.a.cycles + 1):
                self.cycle(n)
            # the last check: after the final logout the next login must still work
            marker = f"GSLTEST-{os.getpid()}-final"
            self.vm.sh(f"logger -t gsl-test {marker}")
            sid = self.login(self.a.cycles + 1)
            if sid:
                time.sleep(20)
                self.verify_session(self.a.cycles + 1, sid, marker)
        finally:
            for p in self.held:
                p.kill()
        os.makedirs(self.a.out, exist_ok=True)
        summary = {"result": "FAIL" if self.failed else "PASS", "versions": self.versions, "checks": self.results}
        with open(os.path.join(self.a.out, "result.json"), "w") as f:
            json.dump(summary, f, indent=1)
        nfail = sum(not r["ok"] for r in self.results)
        print(f"RESULT {summary['result']}: {len(self.results) - nfail} passed, {nfail} failed", flush=True)
        return 1 if self.failed else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--qmp", required=True, help="QMP unix socket of the VM")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=22)
    ap.add_argument("--key", required=True, help="ssh private key authorised for root in the VM")
    ap.add_argument("--user", default="owner", help="the desktop login user (listed at the greeter)")
    ap.add_argument("--cycles", type=int, default=3)
    ap.add_argument("--out", default="gsl-out")
    ap.add_argument("--vm-out", default=None, help="--out as seen by QEMU (screendump path) when QEMU runs in a container")
    sys.exit(Run(ap.parse_args()).main())


if __name__ == "__main__":
    main()
