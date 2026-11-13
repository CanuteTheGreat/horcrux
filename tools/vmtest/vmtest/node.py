"""Node: one VM with the nixosTest-style API (succeed / fail / wait_for_unit / ...)."""
from __future__ import annotations

import base64
import json
import os
import random
import re
import shlex
import socket
import ssl
import subprocess
import threading
import time
import urllib.error
import urllib.request

from . import guest
from .qemu import ARCHS, Qemu, find_ovmf, kvm_available
from .spec import Disk, NodeSpec


class VmtestError(Exception):
    pass


class CommandFailed(VmtestError):
    pass


class Timeout(VmtestError):
    pass


class UnitFailed(VmtestError):
    pass


def free_port():
    for _ in range(200):
        p = random.SystemRandom().randrange(20000, 60000)
        s = socket.socket()
        try:
            s.bind(("127.0.0.1", p))
            return p
        except OSError:
            continue
        finally:
            s.close()
    raise VmtestError("no free host port")


class Node:
    def __init__(self, spec: NodeSpec, idx: int, m):
        self.spec, self.idx, self.m = spec, idx, m
        self.name = spec.name
        self.out = os.path.join(m.out, spec.name)
        os.makedirs(self.out, exist_ok=True)
        self.q: Qemu | None = None
        self.ports: dict[str, int] = {}        # forward label -> host port
        self.macs: list[str] = []
        self.ssh: guest.SshChannel | None = None
        self.ssh_ok = False
        self.shell_ready = False
        self._init = None
        self._shell_lock = threading.RLock()
        self.vnc_port = free_port() if spec.vnc else None
        self.disks: list[str] = []             # paths actually attached (overlays)
        self.boots = 0
        self._prepare_disks()

    # ---- convenience ---------------------------------------------------------------
    def T(self, sec):
        return sec * self.m.slow

    def log(self, msg):
        print(f"[{self.name}] {msg}", flush=True)

    # ---- disks / command line -----------------------------------------------------------
    def _overlay(self, base, fmt, dst):
        subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", "-b", os.path.abspath(base), "-F", fmt, dst])

    def _prepare_disks(self):
        s = self.spec
        n = 0
        if s.image:
            dst = os.path.join(self.out, "boot.qcow2")
            self._overlay(s.image, s.image_fmt, dst)
            self.disks.append(dst)
        for d in s.disks:
            dst = os.path.join(self.out, f"disk{n}.qcow2")
            n += 1
            if d.path and d.direct:
                self.disks.append(d.path)
            elif d.path:
                self._overlay(d.path, d.fmt, dst)
                self.disks.append(dst)
            else:
                subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", dst, d.size])
                self.disks.append(dst)
        self.vars_path = None
        if s.firmware == "uefi":
            _, vars_ = find_ovmf(s.arch)
            self.vars_path = os.path.join(self.out, "OVMF_VARS.qcow2")   # qcow2 pflash: keeps savevm/loadvm usable
            subprocess.check_call(["qemu-img", "convert", "-q", "-f", "raw", "-O", "qcow2", vars_, self.vars_path])

    def argv(self):
        s, A = self.spec, ARCHS[self.spec.arch]
        kvm = kvm_available(s.arch)
        a = [A["qemu"], "-machine", "q35,accel=kvm" if kvm else "q35", "-cpu", "host" if (kvm and s.cpu == "host") else s.cpu,
             "-smp", str(s.cpus), "-m", str(s.mem)]
        if not kvm:
            a[a.index("-machine") + 1] = "q35,accel=tcg"
        for i, p in enumerate(self.disks):
            a += ["-drive", f"file={p},if=virtio,format={'qcow2' if p.endswith('.qcow2') else 'raw'}"]
        if not s.nics:
            a += ["-nic", "none"]
        for i, nic in enumerate(s.nics):
            mac = nic.mac or f"52:54:00:{self.idx:02x}:00:{i:02x}"
            if i >= len(self.macs):
                self.macs.append(mac)
            model = nic.model
            if nic.net == "user":
                fw = []
                for label, gport in nic.forwards.items():
                    hp = self.ports.get(label) or free_port()
                    self.ports[label] = hp
                    fw.append(f"hostfwd=tcp:127.0.0.1:{hp}-{nic.guest_ip or ''}:{gport}")
                sub = []
                if nic.subnet:
                    base = nic.subnet.split("/")[0].rsplit(".", 1)[0]
                    sub = [f"net={nic.subnet}", f"host={base}.2"]
                a += ["-netdev", ",".join([f"user,id=n{i}"] + sub + fw)]
            else:
                net = self.m.network(nic.net)
                if hasattr(net, "port"):
                    a += ["-netdev", f"socket,id=n{i},connect=127.0.0.1:{net.port}"]
                    net.register(mac, self.name)
                else:
                    a += ["-netdev", f"bridge,id=n{i},br={net}"]
            a += ["-device", f"{model},netdev=n{i},mac={mac}"]
        if s.rtc_base:
            a += ["-rtc", f"base={s.rtc_base}"]
        # PCI topology must not differ between the live boot (ISO + answer drive) and the installed boot: the installer records NIC
        # names (enpXsY) that depend on the slot, so VGA + xhci are ALWAYS present, after the NICs, like the ISO harness did.
        a += A["vga"] + ["-device", "qemu-xhci,id=xhci"]
        if s.iso:
            if s.iso_media == "usb":
                a += ["-drive", f"if=none,id=media,file={s.iso},format=raw,readonly=on",
                      "-device", "usb-storage,bus=xhci.0,drive=media,bootindex=0"]
            else:
                a += ["-drive", f"if=none,id=media,file={s.iso},format=raw,media=cdrom,readonly=on",
                      "-device", "ide-cd,bus=ide.0,drive=media,bootindex=0"]
        for j, (path, bus) in enumerate(s.extra_drives):
            if bus == "usb":
                a += ["-drive", f"if=none,id=xd{j},file={path},format=raw", "-device", f"usb-storage,bus=xhci.0,drive=xd{j}"]
            else:
                a += ["-drive", f"file={path},if=virtio,format=raw"]
        if s.firmware == "uefi":
            code, _ = find_ovmf(s.arch)
            a += ["-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=qcow2,file={self.vars_path}"]
        if self.vnc_port:
            a += ["-vnc", f"127.0.0.1:{self.vnc_port - 5900}"]     # display N = TCP 5900+N
        return a + s.extra_args

    # ---- power -----------------------------------------------------------------------------
    def start(self, paused=False):
        """Boot the machine (again: disks persist across power_off/crash, like real hardware)."""
        if self.q and self.q.alive():
            raise VmtestError(f"{self.name} already running")
        a = self.argv() + (["-S"] if paused else [])
        self.q = Qemu(f"{self.name}-boot{self.boots}", a, self.out)
        self.boots += 1
        self.q.start()
        self.shell_ready = False
        self.ssh_ok = False
        if "ssh" in self.ports:
            self.ssh = guest.SshChannel(self.ports["ssh"], self.spec.ssh_key, self.spec.ssh_user)
        self.log(f"started pid={self.q.p.pid} accel={'kvm' if kvm_available(self.spec.arch) else 'tcg'}")
        return self

    def eject_media(self):
        """Take the install ISO and the extra drives away; the next start() boots the disk (first-boot of installed system)."""
        self.spec.iso = None
        self.spec.extra_drives = []

    def reset_nvram(self):
        """Fresh UEFI variable store, like a first power-on: firmware re-scans the disk's fallback boot path."""
        if self.vars_path:
            _, vars_ = find_ovmf(self.spec.arch)
            subprocess.check_call(["qemu-img", "convert", "-q", "-f", "raw", "-O", "qcow2", vars_, self.vars_path])

    def power_off(self, timeout=60):
        """Clean shutdown from inside the guest."""
        try:
            self.execute("poweroff", timeout=20)
        except Exception:
            pass
        if not self.q.wait_exit(self.T(timeout)):
            self.log("guest did not power off in time: killing it")
            self.q.kill()

    def crash(self):
        """Power loss: SIGKILL the QEMU process, no flush, no goodbye."""
        self.log("crash (SIGKILL)")
        self.q.kill()
        self.shell_ready = self.ssh_ok = False

    def freeze(self):
        """Hung node: vCPUs stopped, NICs silent, process alive (QMP `stop`)."""
        self.q.qmp("stop")

    def thaw(self):
        self.q.qmp("cont")

    def reboot(self, timeout=300):
        """Warm reboot from inside the guest, then wait for a shell again."""
        try:
            self.execute("reboot", timeout=10)
        except Exception:
            pass
        self.shell_ready = self.ssh_ok = False
        time.sleep(3)
        self.wait_for_shell(timeout)

    def reset(self):
        """Hardware reset button (QMP system_reset)."""
        self.q.qmp("system_reset")
        self.shell_ready = self.ssh_ok = False

    def set_link(self, nic: int, up: bool):
        """Pull/plug the virtual cable of NIC #nic (guest sees carrier loss)."""
        self.q.qmp("set_link", name=f"n{nic}", up=up)

    def snapshot(self, tag):
        self.q.savevm(tag)

    def restore(self, tag):
        self.q.loadvm(tag)

    # ---- serial shell ------------------------------------------------------------------------
    def wait_for_shell(self, timeout=None):
        """Get a usable root shell on the serial console (logging in when a login prompt shows)."""
        timeout = self.T(timeout or self.spec.boot_timeout)
        end = time.time() + timeout
        q = self.q
        last_poke = 0
        with self._shell_lock:
            while time.time() < end:
                if not q.alive():
                    raise VmtestError(f"{self.name}: QEMU exited while waiting for a shell\n{q.serial_tail()}")
                tail = q.serial_tail(300)
                if time.time() - last_poke > 4:
                    q.send("\necho @@RDY@@$((20+22))\n")
                    last_poke = time.time()
                if q.expect(r"@@RDY@@42", 1.5):
                    q.send("stty -echo 2>/dev/null; export PS1= PS2= TERM=dumb; dmesg -n 1 2>/dev/null; echo @@OK@@$((1+1))\n")
                    q.expect(r"@@OK@@2", 10)
                    self.shell_ready = True
                    return self
                if self.spec.console_menu_key and re.search(r"Enter an option:\s*$", tail):
                    q.send(self.spec.console_menu_key + "\n")      # appliance console menu: this entry is the root shell
                    time.sleep(1.5)
                if re.search(r"login:\s*$", tail):
                    q.send(self.spec.login_user + "\n")
                    time.sleep(1)
                    if self.spec.login_password and q.expect(r"[Pp]assword:", 3):
                        q.send(self.spec.login_password + "\n")
                    time.sleep(2)
            raise Timeout(f"{self.name}: no shell on the serial console within {int(timeout)}s\n{q.serial_tail(800)}")

    def _serial_run(self, cmd, timeout):
        if not self.shell_ready:
            self.wait_for_shell()
        with self._shell_lock:
            line, rid = guest.wrap_for_serial(cmd)
            q = self.q
            with q.lock:
                start = len(q.buf)
            q.send("\n" + line + "\n")
            m = q.expect(rf"@@E{rid}@@(\d+)", timeout, consume=False)
            with q.lock:
                data = q.buf[start:].decode("utf-8", "replace")
                if m:
                    q.cursor = max(q.cursor, m.end())
            if not m:
                return 124, f"timeout after {timeout}s\n" + data[-1500:]
            mm = re.search(rf"@@S{rid}@@\r?\n(.*?)\r?\n?@@E{rid}@@(\d+)", data, re.S)
            if not mm:
                return 125, "marker parse error: " + data[-800:]
            return int(mm.group(2)), mm.group(1).replace("\r", "")

    # ---- ssh channel -------------------------------------------------------------------------------
    def wait_for_ssh(self, timeout=240):
        end = time.time() + self.T(timeout)
        while time.time() < end:
            if self.ssh and self.ssh.run("true", timeout=15)[0] == 0:
                self.ssh_ok = True
                return self
            if self.q and not self.q.alive():
                raise VmtestError(f"{self.name}: QEMU exited while waiting for ssh")
            time.sleep(3)
        raise Timeout(f"{self.name}: ssh not reachable within {timeout}s")

    # ---- the API ------------------------------------------------------------------------------------
    def execute(self, cmd, timeout=60, via="auto"):
        """Run `cmd` in the guest; returns (rc, output) with stderr folded in."""
        t = self.T(timeout)
        if via == "ssh" or (via == "auto" and self.ssh_ok):
            return self.ssh.run(cmd, t)
        return self._serial_run(cmd, t)

    def succeed(self, *cmds, timeout=60, via="auto"):
        out = ""
        for c in cmds:
            rc, o = self.execute(c, timeout, via)
            if rc != 0:
                raise CommandFailed(f"{self.name}: `{c}` exited {rc}\n{o[-1500:]}")
            out += o
        return out

    def fail(self, *cmds, timeout=60, via="auto"):
        out = ""
        for c in cmds:
            rc, o = self.execute(c, timeout, via)
            if rc == 0:
                raise CommandFailed(f"{self.name}: `{c}` unexpectedly succeeded\n{o[-800:]}")
            out += o
        return out

    def wait_until(self, fn, timeout=60, every=1.0, what="condition"):
        """Poll a Python callable until truthy; the generic primitive under every wait_for_*."""
        end = time.time() + self.T(timeout)
        last = None
        while True:
            try:
                last = fn()
                if last:
                    return last
            except (CommandFailed, VmtestError):
                raise
            except Exception as e:   # noqa: BLE001
                last = e
            if time.time() >= end:
                raise Timeout(f"{self.name}: timed out after {timeout}s waiting for {what} (last: {last!r})")
            time.sleep(every)

    def wait_until_succeeds(self, cmd, timeout=60, every=1.0, via="auto"):
        end = time.time() + self.T(timeout)
        while True:
            rc, o = self.execute(cmd, 30, via)
            if rc == 0:
                return o
            if time.time() >= end:
                raise Timeout(f"{self.name}: `{cmd}` still failing after {timeout}s (rc={rc}): {o[-500:]}")
            time.sleep(every)

    def wait_until_fails(self, cmd, timeout=60, every=1.0, via="auto"):
        end = time.time() + self.T(timeout)
        while True:
            rc, o = self.execute(cmd, 30, via)
            if rc != 0:
                return o
            if time.time() >= end:
                raise Timeout(f"{self.name}: `{cmd}` still succeeding after {timeout}s")
            time.sleep(every)

    # ---- init-agnostic services ---------------------------------------------------------------
    @property
    def init(self) -> guest.InitSystem:
        if self._init is None:
            rc, o = self.execute(guest.DETECT_CMD, 30)
            kind = o.strip().splitlines()[-1] if o.strip() else "unknown"
            if kind not in guest.INITS:
                raise VmtestError(f"{self.name}: cannot detect the init system (got {kind!r}): vmtest supports openrc and systemd")
            self._init = guest.INITS[kind]
        return self._init

    def _unit(self, u):
        return self.spec.aliases.get(u, {}).get(self.init.name, u)

    def wait_for_boot(self, timeout=240):
        self.wait_until_succeeds(self.init.boot_complete(), timeout, 3)
        return self

    def wait_for_unit(self, name, timeout=120):
        """Service is active.  Raises UnitFailed at once when the init system reports it crashed/failed."""
        u = self._unit(name)
        end = time.time() + self.T(timeout)
        while True:
            if self.execute(self.init.is_active(u), 30)[0] == 0:
                return self
            if self.execute(self.init.is_failed(u), 30)[0] == 0:
                raise UnitFailed(f"{self.name}: unit {u} ({self.init.name}) is failed/crashed\n{self.execute(self.init.log(u, 60), 30)[1][-1200:]}")
            if time.time() >= end:
                raise Timeout(f"{self.name}: unit {u} not active after {timeout}s ({self.init.name})")
            time.sleep(2)

    wait_for_service = wait_for_unit

    def wait_for_unit_stopped(self, name, timeout=60):
        self.wait_until_fails(self.init.is_active(self._unit(name)), timeout)
        return self

    def start_unit(self, name): return self.succeed(self.init.start(self._unit(name)), timeout=120)
    def stop_unit(self, name): return self.succeed(self.init.stop(self._unit(name)), timeout=120)
    def restart_unit(self, name): return self.succeed(self.init.restart(self._unit(name)), timeout=120)
    def enable_unit(self, name): return self.succeed(self.init.enable(self._unit(name)))
    def disable_unit(self, name): return self.succeed(self.init.disable(self._unit(name)))
    def unit_active(self, name): return self.execute(self.init.is_active(self._unit(name)), 30)[0] == 0
    def unit_log(self, name, lines=200): return self.execute(self.init.log(self._unit(name), lines), 30)[1]

    def wait_for_open_port(self, port, timeout=60):
        cmd = f"(ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null) | grep -E '[:.]{int(port)}[[:space:]]'"
        self.wait_until_succeeds(cmd, timeout, 2)
        return self

    def wait_for_file(self, path, timeout=60):
        self.wait_until_succeeds(f"test -e {shlex.quote(path)}", timeout)
        return self

    # ---- files --------------------------------------------------------------------------------
    def put_file(self, local, remote, mode="644"):
        data = base64.b64encode(open(local, "rb").read()).decode()
        self.succeed(f": > {shlex.quote(remote)}")
        for i in range(0, len(data), 2400):
            self.succeed(f"printf %s {data[i:i + 2400]} >> {shlex.quote(remote)}.b64")
        self.succeed(f"base64 -d {shlex.quote(remote)}.b64 > {shlex.quote(remote)} && rm {shlex.quote(remote)}.b64 && chmod {mode} {shlex.quote(remote)}")

    def get_file(self, remote, local):
        out = self.succeed(f"base64 {shlex.quote(remote)}")
        open(local, "wb").write(base64.b64decode(out))

    # ---- host side of the forwards ------------------------------------------------------------
    def url(self, label, path="/", scheme="https"):
        return f"{scheme}://127.0.0.1:{self.ports[label]}{path}"

    def wait_for_forward(self, label, timeout=120):
        def ok():
            try:
                socket.create_connection(("127.0.0.1", self.ports[label]), 2).close()
                return True
            except OSError:
                return False
        self.wait_until(ok, timeout, 2, f"host port forward {label}")
        return self

    def http(self, label, path="/", method="GET", body=None, headers=None, scheme="https", timeout=15):
        ctx = ssl.create_default_context()
        ctx.check_hostname, ctx.verify_mode = False, ssl.CERT_NONE   # appliances ship self-signed certs
        data = json.dumps(body).encode() if body is not None else None
        h = {"Content-Type": "application/json", **(headers or {})} if body is not None else dict(headers or {})
        req = urllib.request.Request(self.url(label, path, scheme), data=data, method=method, headers=h)
        try:
            with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
                return r.status, r.read()
        except urllib.error.HTTPError as e:
            return e.code, e.read()
        except (urllib.error.URLError, OSError):
            return 0, b""

    # ---- screen -----------------------------------------------------------------------------
    def screenshot(self, name):
        """PNG of the VGA console (QMP screendump) into <out>/<node>/<name>.png."""
        return self.q.screendump(os.path.join(self.out, f"{name}.png"))

    def send_key(self, *keys): self.q.send_key(*keys)
    def type_text(self, text): self.q.type_text(text)

    # ---- failure evidence -----------------------------------------------------------------------
    def collect(self):
        """Best-effort evidence; never raises.  Serial log + qemu stderr are already in self.out."""
        if not (self.q and self.q.alive()):
            return
        try:
            self.screenshot("failure-screen")
        except Exception as e:   # noqa: BLE001
            self.log(f"screenshot failed: {e}")
        if not (self.ssh_ok or self.shell_ready):
            return
        try:
            kind = self.init
            cmds = {"init-diag": kind.diag(), "status": kind.status_all(),
                    "net": "ip -br addr; ip route; ss -tulpn 2>/dev/null | head -50",
                    "proc": "ps aux 2>/dev/null | head -80 || ps | head -80", "df": "df -h; free -m 2>/dev/null; uptime",
                    "dmesg": "dmesg 2>/dev/null | tail -300"}
            for k, c in cmds.items():
                rc, o = self.execute(c, 30)
                open(os.path.join(self.out, f"guest-{k}.txt"), "w").write(o)
        except Exception as e:   # noqa: BLE001
            self.log(f"guest diagnostics failed: {e}")
