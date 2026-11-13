"""QEMU process driver: command line, QMP, serial console, keys, screenshots, snapshots.

The serial line is a unix-socket chardev (never `-serial stdio`: the 16550 TX stalls mid-frame
until the next input byte after a burst - see the ISO harness history).  A reader thread appends
everything to one buffer and to <out>/<name>-serial.log; `expect()` consumes from that buffer.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import socket
import struct
import subprocess
import threading
import time
import zlib

HOST_ARCH = {"amd64": "x86_64", "arm64": "aarch64"}.get(os.uname().machine.lower(), os.uname().machine.lower())
ARCHS = {   # machine args ported from build/live-iso/common/qemu-test.py (x86_64 is the CI-proven one)
    "x86_64": dict(qemu="qemu-system-x86_64", nic="virtio-net-pci", vga=["-device", "VGA"]),
    "aarch64": dict(qemu="qemu-system-aarch64", nic="virtio-net-pci,romfile=", vga=["-device", "virtio-gpu-pci"]),
}
FIRMWARE = {
    "x86_64": [("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
               ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd")],
    "aarch64": [("/usr/share/AAVMF/AAVMF_CODE.fd", "/usr/share/AAVMF/AAVMF_VARS.fd")],
}
LIVE: list["Qemu"] = []     # every QEMU this process started (deadline/teardown kills them by pid, never by pattern)


def kill_all():
    for q in list(LIVE):
        q.kill()


def kvm_available(arch="x86_64") -> bool:
    return os.path.exists("/dev/kvm") and os.access("/dev/kvm", os.W_OK) and arch == HOST_ARCH


def find_ovmf(arch="x86_64"):
    for code, vars_ in FIRMWARE[arch]:
        if os.path.exists(code) and os.path.exists(vars_):
            return code, vars_
    raise RuntimeError(f"no UEFI firmware for {arch} (apt: ovmf / qemu-efi-aarch64)")


def png_from_ppm(ppm_path, png_path):
    d = open(ppm_path, "rb").read()
    m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", d)
    w, h = int(m.group(1)), int(m.group(2))
    px = d[m.end():]
    raw = b"".join(b"\x00" + px[y * w * 3:(y + 1) * w * 3] for y in range(h))

    def ch(t, b):
        return struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b) & 0xFFFFFFFF)
    open(png_path, "wb").write(b"\x89PNG\r\n\x1a\n" + ch(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                               + ch(b"IDAT", zlib.compress(raw, 6)) + ch(b"IEND", b""))


# US-layout keys for QMP send-key
_SHIFT = {c: k for c, k in zip('!@#$%^&*()_+{}|:"<>?~', "1234567890-=[]\\;',./`")}
_PLAIN = {" ": "spc", "-": "minus", "=": "equal", "[": "bracket_left", "]": "bracket_right", "\\": "backslash", ";": "semicolon",
          "'": "apostrophe", ",": "comma", ".": "dot", "/": "slash", "`": "grave_accent", "\n": "ret", "\t": "tab"}


def qcodes(ch):
    if ch.isalpha():
        return (["shift"] if ch.isupper() else []) + [ch.lower()]
    if ch.isdigit():
        return [ch]
    if ch in _PLAIN:
        return [_PLAIN[ch]]
    if ch in _SHIFT:
        base = _SHIFT[ch]
        return ["shift", base if base.isdigit() else _PLAIN[base]]
    raise ValueError(ch)


class Qemu:
    def __init__(self, name: str, argv: list[str], out: str):
        self.name, self.out, self.argv = name, out, argv
        self.qmp_path = os.path.join(out, f"{name}.qmp")
        self.ser_path = os.path.join(out, f"{name}.serial")
        self.log_path = os.path.join(out, f"{name}-serial.log")
        self.err_path = os.path.join(out, f"{name}-qemu.err")
        self.buf = b""
        self.cursor = 0
        self.lock = threading.Lock()
        self.qlock = threading.Lock()
        self.last_rx = time.time()
        self.sock = None
        self.p = None

    # ---- lifecycle -----------------------------------------------------------------
    def start(self):
        os.makedirs(self.out, exist_ok=True)
        full = self.argv + ["-qmp", f"unix:{self.qmp_path},server,nowait",
                            "-chardev", f"socket,id=ser0,path={self.ser_path},server=on,wait=off", "-serial", "chardev:ser0",
                            "-display", "none", "-name", self.name]
        err = ""
        for attempt in range(3):
            for pth in (self.qmp_path, self.ser_path):
                if os.path.exists(pth):
                    os.unlink(pth)
            self.p = subprocess.Popen(full, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=open(self.err_path, "wb"))
            LIVE.append(self)
            for _ in range(300):
                try:
                    s = socket.socket(socket.AF_UNIX)
                    s.connect(self.ser_path)
                    self.sock = s
                    break
                except OSError:
                    if self.p.poll() is not None:
                        break
                    time.sleep(0.1)
            if self.sock:
                break
            err = open(self.err_path, "rb").read()[-600:].decode("utf-8", "replace").strip()
            if self.p.poll() is None:
                self.p.kill()
            self.p.wait()
            LIVE.remove(self)
            time.sleep(1)
        if not self.sock:
            raise RuntimeError(f"{self.name}: QEMU never opened its serial socket in 3 tries: {err or 'no stderr'}")
        self.log = open(self.log_path, "ab")
        threading.Thread(target=self._rd, daemon=True).start()

    def _rd(self):
        while True:
            try:
                b = self.sock.recv(4096)
            except OSError:
                break
            if not b:
                break
            self.log.write(b)
            self.log.flush()
            with self.lock:
                self.buf += b
                self.last_rx = time.time()

    def alive(self):
        return self.p is not None and self.p.poll() is None

    def kill(self):
        if self.p and self.p.poll() is None:
            self.p.kill()
            try:
                self.p.wait(10)
            except Exception:
                pass
        if self in LIVE:
            LIVE.remove(self)

    def wait_exit(self, timeout):
        try:
            self.p.wait(timeout)
            return True
        except subprocess.TimeoutExpired:
            return False

    # ---- serial --------------------------------------------------------------------
    def send(self, s):
        try:
            self.sock.sendall(s.encode() if isinstance(s, str) else s)
        except OSError:
            pass

    def expect(self, pat, timeout, consume=True):
        """Search not-yet-consumed serial output for `pat`; returns the match or None."""
        rx = re.compile(pat.encode() if isinstance(pat, str) else pat)
        end = time.time() + timeout
        while True:
            with self.lock:
                m = rx.search(self.buf, self.cursor)
                if m:
                    if consume:
                        self.cursor = m.end()
                    return m
            if time.time() >= end or (not self.alive() and not m):
                with self.lock:
                    m = rx.search(self.buf, self.cursor)
                    if m and consume:
                        self.cursor = m.end()
                    return m
            time.sleep(0.2)

    def serial_text(self):
        with self.lock:
            return self.buf.decode("utf-8", "replace")

    def serial_tail(self, n=1500):
        return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", self.serial_text()[-n:]).replace("\r", "")

    # ---- QMP ------------------------------------------------------------------------
    def qmp(self, cmd, **args):
        with self.qlock:
            s = socket.socket(socket.AF_UNIX)
            s.settimeout(30)
            s.connect(self.qmp_path)
            f = s.makefile("rw")
            f.readline()
            f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n"); f.flush(); f.readline()
            f.write(json.dumps({"execute": cmd, "arguments": args}) + "\n"); f.flush()
            while True:
                r = json.loads(f.readline())
                if "return" in r or "error" in r:
                    break              # skip async events
            s.close()
        if "error" in r:
            raise RuntimeError(f"qmp {cmd}: {r['error'].get('desc')}")
        return r["return"]

    def hmp(self, line):
        return self.qmp("human-monitor-command", **{"command-line": line})

    def screendump(self, png_path):
        ppm = os.path.abspath(png_path[:-4] + ".ppm")
        self.qmp("screendump", filename=ppm)
        for _ in range(20):
            if os.path.exists(ppm) and os.path.getsize(ppm) > 0:
                break
            time.sleep(0.25)
        time.sleep(0.3)
        png_from_ppm(ppm, png_path)
        os.unlink(ppm)
        return png_path

    def send_key(self, *keys):
        self.qmp("send-key", keys=[{"type": "qcode", "data": k} for k in keys])

    def type_text(self, s, delay=0.05):
        for ch in s:
            self.send_key(*qcodes(ch))
            time.sleep(delay)

    # ---- snapshots (qcow2 disks only; see docs: UEFI vars must be a qcow2 pflash too) ----
    def savevm(self, tag):
        r = self.hmp(f"savevm {tag}")
        if r.strip():
            raise RuntimeError(f"savevm {tag}: {r.strip()}")

    def loadvm(self, tag):
        r = self.hmp(f"loadvm {tag}")
        if r.strip():
            raise RuntimeError(f"loadvm {tag}: {r.strip()}")
        with self.lock:
            self.cursor = len(self.buf)
