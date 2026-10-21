#!/usr/bin/env python3
"""Incus (and LXD-API) console end-to-end tests through the Horcrux HTTP API.

Runs inside the privileged sibling container incus_e2e.sh starts, next to a
real Incus daemon and a horcrux-api built with the incus feature.

MODE=ct (default) - system containers, no KVM needed:
  create an Incus container from the form's image field, start it, then
  * Terminal: exec WebSocket on a real PTY, command in -> output out
  * Console:  type=console WebSocket: boot log history, getty prompt, log in
              and run a command on /dev/console
  * Logs:     console-log WebSocket
MODE=vm - Incus VMs (needs /dev/kvm):
  create + start an Incus VM through the API, then
  * Serial:  type=console WebSocket, log in on ttyS0 and run a command
  * SPICE:   POST /console/:id/spice -> Horcrux's SPICE relay: full SPICE
             link with RSA ticket auth, MAIN_INIT and the channel list
             (display + inputs) from the VM's real SPICE server; a wrong
             password is refused; the same through the browser WebSocket
             proxy; the .vv file names the relay
  * VNC is refused with a pointer to SPICE/Serial - and no reply anywhere
    tells the user to use a CLI.
"""
import base64
import json
import os
import re
import socket
import ssl
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request

API = os.environ.get("API", "https://127.0.0.1:8006/api")
# The API serves HTTPS from the node-local CA: trust that CA when given
# (HORCRUX_CA_FILE), otherwise accept the self-signed certificate.
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL_CTX = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
if _CA:
    # Strict RFC 5280 checks, as Python >= 3.13 does by default (the node
    # CA's leaf must carry an Authority Key Identifier).
    SSL_CTX.verify_flags |= ssl.VERIFY_X509_STRICT
MODE = os.environ.get("MODE", "ct")
CT = "hx-incus-ct"
VM = "hx-incus-vm"
STEP = [0]
TOKEN = [None]
CLI_HINTS = re.compile(r"incus console|lxc console|use the console|--type=vga|--type=console", re.I)


def log(msg):
    print(f"[incus-e2e:{MODE}] {msg}", flush=True)


def step(msg):
    STEP[0] += 1
    log(f"--- {STEP[0]}. {msg}")


def fail(msg):
    log(f"FAIL: {msg}")
    subprocess.run(
        ["sh", "-c", "echo '== api log'; grep -v 'Failed to broadcast' /tmp/api.log | tail -60;"
         " echo '== incus'; incus list </dev/null; tail -30 /tmp/incusd.log"]
    )
    sys.exit(1)


def check(cond, msg):
    if not cond:
        fail(msg)
    log(f"ok: {msg}")


def req(method, path, body=None, expect=200, timeout=1200, raw=False):
    data = None if body is None else json.dumps(body).encode()
    r = urllib.request.Request(API + path, data=data, method=method)
    r.add_header("Content-Type", "application/json")
    if TOKEN[0]:
        r.add_header("Authorization", "Bearer " + TOKEN[0])
    try:
        with urllib.request.urlopen(r, timeout=timeout, context=SSL_CTX) as resp:
            status, text = resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if CLI_HINTS.search(text):
        fail(f"{method} {path} tells the user to use a CLI: {text[:400]}")
    if expect is not None and status != expect:
        fail(f"{method} {path} -> {status} (expected {expect}): {text[:600]}")
    if raw:
        return status, text
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def sh(cmd, check_rc=True):
    p = subprocess.run(cmd, shell=True, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    if check_rc and p.returncode != 0:
        fail(f"`{cmd}` failed ({p.returncode}): {p.stderr.strip()} {p.stdout.strip()}")
    return p.stdout.strip()


def wait_for(desc, fn, timeout=60, every=2):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        try:
            last = fn()
            if last:
                return last
        except SystemExit:
            raise
        except Exception as e:  # noqa: BLE001
            last = e
        time.sleep(every)
    fail(f"timed out waiting for {desc} (last: {last})")


# ---- minimal WebSocket client (stdlib only, keeps its read buffer) -------
class WS:
    def __init__(self, path):
        scheme, host, port = re.match(r"(https?)://([^:/]+):(\d+)", API).groups()
        self.s = socket.create_connection((host, int(port)), timeout=30)
        if scheme == "https":
            self.s = SSL_CTX.wrap_socket(self.s, server_hostname=host)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall(
            (
                f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\n"
                f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
                f"Authorization: Bearer {TOKEN[0]}\r\n\r\n"
            ).encode()
        )
        resp = b""
        while b"\r\n\r\n" not in resp:
            chunk = self.s.recv(1)
            if not chunk:
                break
            resp += chunk
        if b" 101 " not in resp.split(b"\r\n")[0]:
            fail(f"WebSocket upgrade for {path} failed: {resp[:200]!r}")
        self.buf = b""
        self.data = b""  # payload bytes received, not yet consumed
        self.closed = False

    def send(self, payload, binary=False):
        if isinstance(payload, str):
            payload = payload.encode()
        mask = os.urandom(4)
        hdr = bytearray([0x82 if binary else 0x81])
        n = len(payload)
        if n < 126:
            hdr.append(0x80 | n)
        elif n < 65536:
            hdr.append(0x80 | 126)
            hdr += n.to_bytes(2, "big")
        else:
            hdr.append(0x80 | 127)
            hdr += n.to_bytes(8, "big")
        self.s.sendall(bytes(hdr) + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))

    def _pump(self, timeout):
        self.s.settimeout(timeout)
        try:
            chunk = self.s.recv(65536)
        except socket.timeout:
            return
        if not chunk:
            self.closed = True
            return
        self.buf += chunk
        while len(self.buf) >= 2:
            op, ln = self.buf[0] & 0x0F, self.buf[1] & 0x7F
            off = 2
            if ln == 126:
                if len(self.buf) < 4:
                    break
                ln, off = int.from_bytes(self.buf[2:4], "big"), 4
            elif ln == 127:
                if len(self.buf) < 10:
                    break
                ln, off = int.from_bytes(self.buf[2:10], "big"), 10
            if len(self.buf) < off + ln:
                break
            payload, self.buf = self.buf[off : off + ln], self.buf[off + ln :]
            if op in (0, 1, 2):
                self.data += payload
            elif op == 8:
                self.closed = True

    def read_until(self, pattern, timeout=60, what=""):
        """Consume text until `pattern` (regex) appears; returns it all."""
        end = time.time() + timeout
        while time.time() < end:
            text = self.data.decode(errors="replace")
            if re.search(pattern, text):
                self.data = b""
                return text
            if self.closed:
                break
            self._pump(1)
        fail(f"WebSocket {what} never produced /{pattern}/; got: {self.data[-800:]!r}")

    def read_exact(self, n, timeout=30):
        end = time.time() + timeout
        while len(self.data) < n and time.time() < end and not self.closed:
            self._pump(1)
        if len(self.data) < n:
            fail(f"WebSocket: wanted {n} bytes, got {len(self.data)}")
        out, self.data = self.data[:n], self.data[n:]
        return out

    def close(self):
        try:
            self.s.close()
        except OSError:
            pass


def login():
    _, body = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN[0] = body.get("ticket") or body.get("token")


def incus(cmd, check_rc=True):
    return sh(f"incus {cmd}", check_rc)


# ---- shell login on a console (getty) -------------------------------------
def console_login_and_run(ws, what, timeout=180):
    """Get a shell on a getty console and prove a command round-trip."""
    def prompt():
        ws.send("\r")
        try:
            return ws.read_until(r"login:|# $|# \S*$", timeout=10, what=what)
        except SystemExit:
            return None

    seen = None
    end = time.time() + timeout
    while time.time() < end and not seen:
        seen = _try(prompt)
    check(seen, f"{what}: getty answers Enter with a login prompt")
    if "login:" in seen:
        ws.send("root\r")
        ws.read_until(r"#", timeout=60, what=f"{what} root shell")
    ws.send("echo HX$((6*7))-$(tty)\r")
    # Up to the line end: output can arrive split mid-word ("/dev/t" + "tyS0").
    out = ws.read_until(r"HX42-/dev/\S+\s", timeout=60, what=f"{what} command")
    tty = re.search(r"HX42-(/dev/\S+)\s", out).group(1)
    check(True, f"{what}: command typed in the browser ran in the guest (tty {tty})")
    return tty


def _try(fn):
    try:
        return fn()
    except SystemExit:
        return None


# ---- SPICE ------------------------------------------------------------------
def spice_link(read_exact, write, password, channel=1, connection_id=0):
    """SPICE link + ticket auth; returns the link result (0 = ok).

    Channels other than main must carry the session id MAIN_INIT gave the
    main channel (`connection_id`), like a real client."""
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import padding

    mess = struct.pack("<IBBIII", connection_id, channel, 0, 1, 0, 18) + struct.pack("<I", 0b11)
    write(b"REDQ" + struct.pack("<III", 2, 2, len(mess)) + mess)
    hdr = read_exact(16)
    if hdr[:4] != b"REDQ":
        fail(f"SPICE: not a SPICE server: {hdr!r}")
    reply = read_exact(struct.unpack("<I", hdr[12:16])[0])
    err = struct.unpack("<I", reply[:4])[0]
    if err:
        fail(f"SPICE link error {err}")
    key = serialization.load_der_public_key(reply[4:166])
    write(struct.pack("<I", 1))  # auth mechanism: SPICE ticket
    enc = key.encrypt(
        password.encode() + b"\0",
        padding.OAEP(mgf=padding.MGF1(hashes.SHA1()), algorithm=hashes.SHA1(), label=None),
    )
    write(enc)
    result = struct.unpack("<I", read_exact(4))[0]
    return result


def spice_main_channels(read_exact, write):
    """After a successful main-channel link: MAIN_INIT -> ATTACH -> list.

    Returns (session id, [(channel type, id)])."""
    serial = 0
    session = 0
    attached = False
    for _ in range(50):
        h = read_exact(18)
        ty = struct.unpack("<H", h[8:10])[0]
        size = struct.unpack("<I", h[10:14])[0]
        body = read_exact(size)
        if ty == 103 and not attached:  # MAIN_INIT
            session = struct.unpack("<I", body[:4])[0]
            serial += 1
            write(struct.pack("<QHII", serial, 104, 0, 0))  # ATTACH_CHANNELS
            attached = True
        elif ty == 104:  # CHANNELS_LIST
            n = struct.unpack("<I", body[:4])[0]
            return session, [(body[4 + 2 * i], body[5 + 2 * i]) for i in range(n)]
    fail("SPICE: no channel list")


def tcp_io(host, port):
    s = socket.create_connection((host, port), timeout=30)

    def read_exact(n):
        out = b""
        while len(out) < n:
            c = s.recv(n - len(out))
            if not c:
                fail(f"SPICE relay closed after {len(out)}/{n} bytes")
            out += c
        return out

    return s, read_exact, s.sendall


# ---- container ----------------------------------------------------------------
def run_ct():
    step("features: incus compiled in")
    _, feats = req("GET", "/features")
    check("incus" in feats["features"], f"incus enabled in /api/features: {feats['features']}")

    step("create an Incus container from the image field (alpine/3.22) and start it")
    req("DELETE", f"/containers/{CT}", expect=None)
    incus(f"delete -f {CT}", check_rc=False)
    req("POST", "/containers", {
        "id": CT, "name": CT, "runtime": "incus", "memory": 256, "cpus": 1,
        "rootfs": os.environ.get("CT_IMAGE", "alpine/3.22"), "status": "stopped",
    })
    req("POST", f"/containers/{CT}/start")
    rel = wait_for("container running", lambda: incus(f"exec {CT} -- cat /etc/alpine-release", False), 120)
    check(re.match(r"3\.\d+", rel), f"container runs the chosen image (alpine {rel}), not the old hardcoded Ubuntu")

    step("Terminal: exec WebSocket on a real PTY")
    _, t = req("POST", f"/containers/{CT}/exec-ticket", {"runtime": "incus", "command": []})
    ws = WS(t["ws_path"])
    ws.send("echo HX$((6*7)) && tty\n")
    out = ws.read_until(r"HX42\s+/dev/pts/\d+", 60, "terminal")
    check(True, f"terminal round-trip: {re.search(r'HX42\s+(/dev/pts/[0-9]+)', out).group(1)}")
    ws.send("exit\n")
    ws.close()

    step("Console: type=console WebSocket (history, getty, log in, command)")
    incus(f"exec {CT} -- passwd -d root")
    _, t = req("POST", f"/containers/{CT}/console-ticket?runtime=incus")
    check(t["ws_path"].startswith("/api/containers/ws/console/"), f"console ticket {t['ws_path']}")
    ws = WS(t["ws_path"])
    hist = ws.read_until(r"\S", 30, "console history")
    log(f"console history ({len(hist)} bytes): {hist[-200:]!r}")
    tty = console_login_and_run(ws, "container console")
    check(tty in ("/dev/console", "/dev/tty1", "/dev/pts/0") or tty.startswith("/dev/"), f"console tty {tty}")
    ws.send("exit\r")
    ws.close()

    step("Logs: console log WebSocket")
    _, t = req("POST", f"/containers/{CT}/logs-ticket?runtime=incus")
    ws = WS(t["ws_path"])
    out = ws.read_until(r"\S", 30, "console log")
    check(len(out) > 0, f"console log streamed ({len(out)} bytes)")
    ws.close()

    step("Docker containers have no system console (clear 422, no CLI hint)")
    st, body = req("POST", f"/containers/{CT}/console-ticket?runtime=docker", expect=None, raw=True)
    check(st in (422, 400), f"docker console-ticket refused: {st} {body[:120]}")

    step("Clone: flag-like target names are refused (422), a valid clone works")
    req("POST", f"/containers/{CT}/stop", expect=None)
    wait_for("container stopped",
             lambda: "STOPPED" in incus(f"list {CT} --format csv -c s", False).upper() or None, 60)
    before = incus("list --format csv -c n", False)
    for bad in ["-csecurity.privileged=true", "--config=security.privileged=true", "-x",
                "a b", "a/b", "qa_b7"]:
        st, body = req("POST", f"/containers/{CT}/clone",
                       {"target_id": "hx-clone-bad", "target_name": bad}, expect=None, raw=True)
        check(st == 422 and "Invalid instance name" in body,
              f"clone to {bad!r} refused with 422: {st} {body[:160]}")
    check(incus("list --format csv -c n", False) == before,
          "no Incus instance created by a refused clone")
    privileged = sh("for i in $(incus list --format csv -c n </dev/null); do "
                    "incus config get $i security.privileged </dev/null; done", check_rc=False)
    check("true" not in privileged, f"no instance became privileged ({privileged!r})")
    st, _ = req("GET", "/containers/hx-clone-bad", expect=None)
    check(st == 404, f"refused clone not registered ({st})")
    clone = CT + "-copy"
    incus(f"delete -f {clone}", check_rc=False)
    req("DELETE", f"/containers/{clone}", expect=None)
    req("POST", f"/containers/{CT}/clone", {"target_id": clone, "target_name": clone})
    check(clone in incus("list --format csv -c n", False).split(),
          f"valid clone {clone} exists in Incus")
    req("DELETE", f"/containers/{clone}", expect=None)
    incus(f"delete -f {clone}", check_rc=False)

    if os.environ.get("KEEP") != "1":
        step("cleanup")
        req("POST", f"/containers/{CT}/stop", expect=None)
        req("DELETE", f"/containers/{CT}", expect=None)
        incus(f"delete -f {CT}", check_rc=False)


# ---- VM -----------------------------------------------------------------------
def run_vm():
    step("create an Incus VM through the API and start it")
    req("DELETE", f"/vms/{VM}", expect=None)
    incus(f"delete -f {VM}", check_rc=False)
    step("an image for a QEMU VM is refused (QEMU takes disk images via import-disk)")
    st, body = req("POST", "/vms", {
        "id": VM + "-q", "name": VM + "-q", "hypervisor": "qemu", "memory": 512, "cpus": 1,
        "disk_size": 10, "status": "stopped", "architecture": "x86_64", "image": "debian/12",
    }, expect=None, raw=True)
    check(st == 422 and "import-disk" in body, f"qemu + image: {st} {body[:160]}")
    req("POST", "/vms", {
        "id": VM, "name": VM, "hypervisor": "incus", "memory": 1024, "cpus": 1,
        "disk_size": 10, "status": "stopped", "architecture": "x86_64",
        # Not the old hardcoded default (ubuntu/22.04).
        "image": "ubuntu/24.04",
    }, expect=201)
    check(incus(f"list {VM} -c t --format csv") == "VIRTUAL-MACHINE", "instance is an Incus VM")
    rel = incus(f"config get {VM} image.release")
    check(rel in ("noble", "24.04"), f"VM was created from the requested image (ubuntu 24.04 'noble'), got {rel!r}")

    step("console of a stopped VM: clear message, no CLI pointer")
    st, body = req("POST", f"/console/{VM}/serial", expect=None, raw=True)
    check(st >= 400 and "start it" in body, f"stopped VM: {st} {body[:160]}")

    req("POST", f"/vms/{VM}/start")
    wait_for("VM agent", lambda: subprocess.run(
        f"incus exec {VM} -- true", shell=True, stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0, 300, 5)
    incus(f"exec {VM} -- passwd -d root")

    step("Serial console: log in on the VM's serial port and run a command")
    _, info = req("POST", f"/console/{VM}/serial")
    check(info["ws_path"].startswith("/api/console/ws/serial/"), f"serial ticket {info['ws_path']}")
    ws = WS(info["ws_path"])
    tty = console_login_and_run(ws, "VM serial console", timeout=240)
    check("ttyS" in tty or "hvc" in tty or "console" in tty, f"serial tty {tty}")
    ws.send("exit\r")
    ws.close()

    step("VNC is not offered for Incus VMs (points at SPICE/Serial)")
    st, body = req("POST", f"/console/{VM}/vnc", expect=None, raw=True)
    check(st == 422 and "SPICE" in body, f"vnc refused: {st} {body[:200]}")

    step("SPICE relay: link + ticket auth + channel list from the VM's SPICE server")
    _, sp = req("POST", f"/console/{VM}/spice")
    host, port, pw = sp["host"], sp["port"], sp["password"]
    check(port > 0 and pw, f"SPICE relay at {host}:{port}")
    host = "127.0.0.1" if host in ("0.0.0.0", "::") else host
    s, rd, wr = tcp_io(host, port)
    check(spice_link(rd, wr, pw) == 0, "SPICE ticket accepted by the relay and the VM")
    session, chans = spice_main_channels(rd, wr)
    kinds = {c[0] for c in chans}
    check(2 in kinds and 3 in kinds, f"VM SPICE channels include display (2) + inputs (3): {chans}")
    # A display channel on its own connection (a second vga session).
    s2, rd2, wr2 = tcp_io(host, port)
    r = spice_link(rd2, wr2, pw, channel=2, connection_id=session)
    check(r == 0, f"display channel links on a second connection (second vga session): result {r}")
    s2.close()
    s.close()

    s, rd, wr = tcp_io(host, port)
    check(spice_link(rd, wr, "wrong-password") == 7, "wrong SPICE password refused (permission denied)")
    s.close()

    step("SPICE through the browser WebSocket proxy")
    ws = WS(sp["ws_path"])
    check(spice_link(ws.read_exact, lambda b: ws.send(b, binary=True), pw) == 0,
          "SPICE link over the ticketed WebSocket")
    ws.close()

    step(".vv file for remote-viewer names the relay")
    _, vv = req("GET", f"/console/{VM}/spice.vv", raw=True)
    check("type=spice" in vv and f"port={port}" in vv and f"password={pw}" in vv,
          f".vv: {vv.splitlines()[:4]}")

    if os.environ.get("KEEP") != "1":
        step("cleanup")
        req("POST", f"/vms/{VM}/stop", expect=None)
        req("DELETE", f"/vms/{VM}", expect=None)
        incus(f"delete -f {VM}", check_rc=False)


def main():
    login()
    if MODE == "vm":
        run_vm()
    else:
        run_ct()
    log("PASS")


if __name__ == "__main__":
    main()
