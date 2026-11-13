"""Init-agnostic guest helpers.  OWNER RULE: 100% support for OpenRC *and* systemd.

Every service operation is expressed once here and rendered for the init system the guest
runs.  Nothing above this module may contain `systemctl` or `rc-service`.

Unit names are the same on both inits (`horcrux-api`, `chronyd`, ...).  When a distro names the
same daemon differently, pass `aliases={"sshd": {"systemd": "ssh"}}` on the Node.
"""
from __future__ import annotations

import base64
import shlex
import subprocess
import uuid

# Detection runs in the guest: /run/systemd/system exists only while systemd is PID 1.
DETECT_CMD = (
    "if [ -d /run/systemd/system ]; then echo systemd; "
    "elif command -v rc-service >/dev/null 2>&1; then echo openrc; "
    "else echo unknown; fi"
)


class InitSystem:
    name = "?"

    def is_active(self, u): raise NotImplementedError
    def is_failed(self, u): raise NotImplementedError
    def start(self, u): raise NotImplementedError
    def stop(self, u): raise NotImplementedError
    def restart(self, u): raise NotImplementedError
    def enable(self, u): raise NotImplementedError
    def disable(self, u): raise NotImplementedError
    def boot_complete(self): raise NotImplementedError
    def status_all(self): raise NotImplementedError
    def log(self, u, lines=200): raise NotImplementedError
    def diag(self): raise NotImplementedError


class Systemd(InitSystem):
    name = "systemd"

    def is_active(self, u): return f"systemctl is-active --quiet {shlex.quote(u)}"
    def is_failed(self, u): return f"systemctl is-failed --quiet {shlex.quote(u)}"
    def start(self, u): return f"systemctl start {shlex.quote(u)}"
    def stop(self, u): return f"systemctl stop {shlex.quote(u)}"
    def restart(self, u): return f"systemctl restart {shlex.quote(u)}"
    def enable(self, u): return f"systemctl enable {shlex.quote(u)}"
    def disable(self, u): return f"systemctl disable {shlex.quote(u)}"
    # `is-system-running` says "running" or "degraded" once boot finished; "starting"/"initializing" while booting.
    def boot_complete(self): return "s=$(systemctl is-system-running 2>/dev/null); [ \"$s\" = running ] || [ \"$s\" = degraded ]"
    def status_all(self): return "systemctl list-units --all --no-pager --no-legend 2>&1 | head -150"
    def log(self, u, lines=200): return f"journalctl -u {shlex.quote(u)} --no-pager -n {lines} 2>&1"
    def diag(self): return "systemctl --failed --no-pager 2>&1; journalctl -b --no-pager -p warning -n 300 2>&1"


class OpenRC(InitSystem):
    name = "openrc"

    # `rc-service X status` exits 0 only for "started" (3 = stopped, 32 = crashed, 1/2 = not found/inactive).
    def is_active(self, u): return f"rc-service {shlex.quote(u)} status >/dev/null 2>&1"
    def is_failed(self, u): return f"rc-service {shlex.quote(u)} status 2>&1 | grep -q crashed"
    def start(self, u): return f"rc-service {shlex.quote(u)} start"
    def stop(self, u): return f"rc-service {shlex.quote(u)} stop"
    def restart(self, u): return f"rc-service {shlex.quote(u)} restart"
    def enable(self, u): return f"rc-update add {shlex.quote(u)} default"
    def disable(self, u): return f"rc-update del {shlex.quote(u)} default"
    # runlevel reached and nothing still "starting"
    def boot_complete(self): return "[ \"$(rc-status -r 2>/dev/null)\" = default ] && ! rc-status -a 2>/dev/null | grep -qi starting"
    def status_all(self): return "rc-status -a 2>&1"
    def log(self, u, lines=200):
        q = shlex.quote(u)
        return (f"for f in /var/log/{q}.log /var/log/{q}/*.log /var/log/messages /var/log/syslog; do "
                f"[ -r \"$f\" ] && {{ echo \"== $f\"; tail -n {lines} \"$f\" | grep -i -E {q} || tail -n 40 \"$f\"; }}; done 2>&1")
    def diag(self): return "rc-status -a 2>&1; (dmesg | tail -150) 2>&1; tail -n 200 /var/log/messages /var/log/syslog 2>/dev/null"


INITS = {"systemd": Systemd(), "openrc": OpenRC()}


def wrap_for_serial(cmd: str):
    """One serial line that runs `cmd` in a POSIX shell and brackets its output with markers.

    The marker text inside the typed line is split (`@@S%s@@`) so the tty echo of the command
    itself can never be mistaken for the marker the shell prints.  Returns (line, id).
    """
    rid = uuid.uuid4().hex[:8]
    b64 = base64.b64encode(cmd.encode()).decode()
    line = (f"printf '@@S%s@@\\n' {rid}; eval \"$(printf %s {b64} | base64 -d)\" 2>&1; "
            f"printf '@@E%s@@%d\\n' {rid} $?")
    return line, rid


class SshChannel:
    def __init__(self, port: int, key: str | None, user: str = "root", host: str = "127.0.0.1"):
        self.port, self.key, self.user, self.host = port, key, user, host

    def _base(self):
        a = ["ssh", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR",
             "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=3",
             "-p", str(self.port)]
        if self.key:
            a += ["-i", self.key, "-o", "IdentitiesOnly=yes"]
        return a + [f"{self.user}@{self.host}"]

    def run(self, cmd: str, timeout: float = 60):
        try:
            r = subprocess.run(self._base() + ["sh", "-c", shlex.quote(cmd)], capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired as e:
            return 124, f"timeout after {timeout}s\n{(e.stdout or b'').decode(errors='replace') if isinstance(e.stdout, bytes) else (e.stdout or '')}"
        return r.returncode, r.stdout + r.stderr   # rc 255 = ssh itself failed (guest not reachable)
