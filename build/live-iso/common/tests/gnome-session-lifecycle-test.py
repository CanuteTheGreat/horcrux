#!/usr/bin/env python3
"""GNOME-on-OpenRC session lifecycle test: logins/logouts at the real GDM greeter while ssh sessions of the same user come and go.

Why: on OpenRC, gnome-session-openrc runs gnome-shell & co. as OpenRC USER services. The user runlevel (service user.<name>, started by
pam_openrc) is shared by every login of the user and only stops with the LAST one. An ssh login of the desktop user (agents, scripts) kept it
alive across a GNOME logout, supervise-daemon respawned gnome-shell into the closed session, that shell held the seat, and the next login got
mutter "Failed to take control of the session: ... EBUSY" and no mouse/keyboard (HOGSMEADE, 2026-11-13). The fix lives in
gnome-base/gnome-session-openrc-49.2-r2::horcrux (session-scoped user services) plus sys-auth/horcrux-ssh-pam (ssh logins start no OpenRC user
services). This test reproduces the owner's sequence on an installed GNOME VM:

    cycle N:  GNOME login at the greeter (QMP key events)  ->  20 parallel `ssh USER@vm true` + one ssh session of USER held open
              ->  GNOME logout (gnome-session-quit, i.e. the menu)  ->  checks  ->  next cycle logs in again

Per cycle it asserts:
  * after the logout no gnome-shell of the user survives or gets respawned (the stale compositor that held the seat);
  * after the login the new session is seat0's active session, and exactly one user gnome-shell runs, in THAT session;
  * no "Failed to take control of the session" in syslog since the cycle started (mutter EBUSY);
  * the input devices (/dev/input/event*) are open in the active session's compositor and in no other gnome-shell of the user;
  * no duplicated user supervisors (two `supervise-daemon dbus` of the user, two `supervise-daemon user.USER`, ...).

Attach mode only: the VM must already run an installed GNOME edition (e.g. installed with qemu-test.py --flow auto and
tests/data/gnome-session-answers.toml), reachable over ssh as root with --key, with a QMP socket for key events. The test sets the login user's
password (test-only value) and authorises --key for that user.

    gnome-session-lifecycle-test.py --qmp /w/qmp.sock --port 22790 --key k [--cycles 3] [--user owner] [--out DIR]

Exit 0 = PASS (every check of every cycle), 1 = FAIL. A JSON summary goes to OUT/result.json, screenshots on failure to OUT/.
"""
import argparse
import collections
import json
import os
import re
import socket
import subprocess
import sys
import time

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
    return vm.out(f"cat /var/log/messages.0 /var/log/messages 2>/dev/null | sed -n '/{marker}/,$p'")


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
        vm.sh(f"logger -t gsl-test {marker}")
        sid = self.login(n)
        if not sid:
            return
        print(f"  c{n}: logged in, session {sid}", flush=True)
        time.sleep(20)  # let the session's services come up
        self.verify_session(n, sid, marker)
        ok, retried, errors = self.ssh_burst()
        self.check(n, "ssh-burst", ok == 20, f"{ok}/20 parallel `ssh {a.user}@vm true` succeeded ({retried} retried after sshd MaxStartups drops)"
                   + (f"; errors: {errors[:3]}" if errors else ""))
        # an agent/script session of the same user that stays open across the GNOME logout (what kept the runlevel alive on HOGSMEADE)
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
