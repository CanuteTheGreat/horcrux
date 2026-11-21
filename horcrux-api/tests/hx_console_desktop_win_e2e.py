#!/usr/bin/env python3
"""Horcrux Console desktop client on Windows, headless, against a real node.

Driven by hx_console_desktop_win_e2e.sh (node already running on :8006,
listening on every address). Steps:
  1. an Alpine console VM on the node (root console, no guest tools);
  2. the cached Windows 11 test image booted as the CLIENT machine (QEMU
     user-mode network: the guest reaches this machine's addresses), through
     a throwaway overlay, fresh UEFI variables and a fresh swtpm;
  3. horcrux-console.exe copied in through the QEMU guest agent
     (guest-file-write), checked by SHA-256 in the guest;
  4. `horcrux-console.exe --headless ... --exercise-input --type-text ...
     --snapshot` run in the guest (guest-exec) over the direct path and
     with WAN access + force_relay, asserting like the WAN e2e desktop paths:
     >= 10 frames decoded by Media Foundation, 0 decode errors, the path the
     node and the client report, keys_in >= 24, gamepad_rx >= 2; the decoded
     snapshot is copied back (PPM + PNG) and must not be blank.
"""
import base64
import hashlib
import json
import os
import shutil
import socket
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request

EXE = os.environ["HXD_EXE"]
ADDR = os.environ["HXD_NODE_ADDR"]
WORK = os.environ["HXD_WORK"]
ALPINE_ISO = os.environ["HXC_ALPINE_ISO"]
WIN_IMAGE = os.environ["HXD_WIN_IMAGE"]
OUT = os.environ.get("HXC_OUT", "/tmp/hxd-win-e2e")
MEM = os.environ.get("HXD_WIN_MEM_MB", "4096")
CPUS = os.environ.get("HXD_WIN_CPUS", "4")
AGENT_WAIT = int(os.environ.get("HXD_AGENT_WAIT_SECS", "900"))
RUN_WAIT = int(os.environ.get("HXD_RUN_WAIT_SECS", "300"))
# how long a guest file transfer may keep timing out while the Windows agent
# stalls around a guest-exec child's exit (see Qga._transfer)
FILE_GRACE = int(os.environ.get("HXD_FILE_GRACE_SECS", "300"))
EXEC_START_WAIT = int(os.environ.get("HXD_EXEC_START_WAIT_SECS", "180"))
GUEST_DIR = os.environ.get("HXD_GUEST_DIR", r"C:\hxc")
TYPED = os.environ.get("HXD_TYPE_TEXT", r"root\necho horcrux-console-on-windows\n")
# Relay listen address for the relay run: HXD_RELAY_LISTEN=<addr> pins it,
# =all leaves the node default (every address). Unset = automatic: every
# address when the node answers relay requests from the address they reached
# (console setting wan.relay_reply_from_request_address), else the address
# the guest uses (a relay on 0.0.0.0 would answer from the route's address,
# which QEMU's user-mode network drops).
RELAY_LISTEN = os.environ.get("HXD_RELAY_LISTEN", "")
BASE = "https://127.0.0.1:8006/api"
CTX = ssl._create_unverified_context()
OVMF_CODE = ["/usr/share/OVMF/OVMF_CODE_4M.secboot.fd", "/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd"]
OVMF_VARS = ["/usr/share/OVMF/OVMF_VARS_4M.ms.fd", "/usr/share/edk2/ovmf/OVMF_VARS.secboot.fd"]
FAILS = []
RESULTS = {}
os.makedirs(OUT, exist_ok=True)


def check(cond, what, fatal=True):
    print(("ok - " if cond else "FAIL ") + what, flush=True)
    if not cond:
        FAILS.append(what)
        if fatal:
            dump()
            sys.exit(1)


def dump():
    RESULTS["fails"] = FAILS
    with open(os.path.join(OUT, "results.json"), "w") as f:
        json.dump(RESULTS, f, indent=1)


def call(method, path, body=None, token=None, ok=(200, 201, 202, 204)):
    data = json.dumps(body).encode() if body is not None else None
    h = {"Content-Type": "application/json"}
    if token:
        h["Authorization"] = "Bearer " + token
    r = urllib.request.Request(BASE + path, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(r, context=CTX, timeout=120) as f:
            t = f.read()
            code = f.status
    except urllib.error.HTTPError as e:
        t, code = e.read(), e.code
    v = json.loads(t) if t and t[:1] in (b"{", b"[") else t.decode(errors="replace")
    if ok is not None and code not in ok:
        raise SystemExit(f"FAIL {method} {path}: {code} {str(v)[:300]}")
    return code, v


def first(paths):
    for p in paths:
        if os.path.exists(p):
            return p
    raise SystemExit(f"FAIL none of {paths} exists (install ovmf)")


class Qga:
    """QEMU guest agent over its UNIX socket (one connection per call:
    the Windows agent keeps no state between them)."""

    def __init__(self, path):
        self.path = path

    def __call__(self, cmd, args=None, timeout=30):
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(timeout)
        s.connect(self.path)
        try:
            f = s.makefile("rw")
            sid = int(time.time() * 1000) % 1000000
            f.write(json.dumps({"execute": "guest-sync", "arguments": {"id": sid}}) + "\n")
            f.flush()
            while True:
                r = json.loads(f.readline())
                if r.get("return") == sid:
                    break
            f.write(json.dumps({"execute": cmd, "arguments": args or {}}) + "\n")
            f.flush()
            r = json.loads(f.readline())
        finally:
            s.close()
        if "error" in r:
            raise RuntimeError(f"{cmd}: {r['error']}")
        return r["return"]

    def exec(self, path, argv, env=None, wait=120):
        a = {"path": path, "arg": argv, "capture-output": True}
        if env is not None:
            a["env"] = env
        # never re-sent (a lost reply would start the program twice): wait
        # out an agent stall instead (see _transfer)
        pid = self("guest-exec", a, timeout=EXEC_START_WAIT)["pid"]
        end = time.time() + wait
        while time.time() < end:
            try:
                st = self("guest-exec-status", {"pid": pid}, timeout=60)
            except (TimeoutError, OSError, ValueError) as e:
                # the Windows agent can stall for a while as the child
                # exits (it collects the captured output); ask again
                print(f"   guest-exec-status: {e!r}, asking again", flush=True)
                time.sleep(2)
                continue
            if st.get("exited"):
                dec = lambda k: base64.b64decode(st.get(k, "")).decode("utf-8", errors="replace")  # noqa: E731
                return st.get("exitcode"), dec("out-data"), dec("err-data")
            time.sleep(1)
        raise TimeoutError(f"{path} did not exit within {wait} s")

    def ps(self, script, wait=120):
        return self.exec("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], wait=wait)

    # The Windows agent stalls for a minute or more around a guest-exec
    # child's exit (it is still collecting the child's output), so file calls
    # right after a run can time out: CI saw guest-file-read/close of the
    # snapshot time out 90 s after horcrux-console.exe exited, and a CI-like
    # run saw the exe copy's write/close time out right after a PowerShell
    # call. The agent still runs a timed-out command later (one queue), but
    # the handle's offset is then unknown, so a transfer starts over with a
    # fresh handle until the grace window ends; a real agent error (missing
    # file, access denied) fails at once.
    def _transfer(self, what, fn, grace):
        end = time.time() + grace
        while True:
            try:
                return fn()
            except (TimeoutError, OSError, ValueError) as e:
                if time.time() >= end:
                    raise
                print(f"   {what}: {e!r}, starting the transfer again", flush=True)
                time.sleep(5)

    def _close(self, h):
        try:
            self("guest-file-close", {"handle": h}, timeout=60)
        except (TimeoutError, OSError, ValueError) as e:
            # the agent closes it when it reaches the queued command; callers
            # check the content afterwards (SHA-256 of a put; a get ends only on eof)
            print(f"   guest-file-close: {e!r} (queued in the agent)", flush=True)

    def put(self, local, remote, chunk=1 << 20, grace=FILE_GRACE):
        def once():
            h = self("guest-file-open", {"path": remote, "mode": "wb"}, timeout=60)
            try:
                with open(local, "rb") as f:
                    while True:
                        b = f.read(chunk)
                        if not b:
                            break
                        w = self("guest-file-write", {"handle": h, "buf-b64": base64.b64encode(b).decode()},
                                 timeout=60)
                        if w.get("count") != len(b):
                            raise RuntimeError(f"short write {w} of {len(b)}")
            finally:
                self._close(h)
        self._transfer(f"guest-file write of {remote}", once, grace)

    def get(self, remote, chunk=1 << 20, grace=FILE_GRACE):
        def once():
            h = self("guest-file-open", {"path": remote, "mode": "rb"}, timeout=60)
            out = bytearray()
            try:
                while True:
                    r = self("guest-file-read", {"handle": h, "count": chunk}, timeout=60)
                    out += base64.b64decode(r.get("buf-b64", ""))
                    if r.get("eof") or not r.get("count"):
                        break
            finally:
                self._close(h)
            return bytes(out)
        return self._transfer(f"guest-file read of {remote}", once, grace)

def qmp(path, cmd, **args):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(30)
    s.connect(path)
    f = s.makefile("rw")
    f.readline()
    for c, a in (("qmp_capabilities", {}), (cmd, args)):
        f.write(json.dumps({"execute": c, "arguments": a}) + "\n")
        f.flush()
        while True:
            r = json.loads(f.readline())
            if "return" in r or "error" in r:
                break
    s.close()
    return r


def ppm_png(ppm_path):
    png = ppm_path[:-4] + ".png"
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", ppm_path, png], check=False)
    return png


def ppm_busy_fraction(data):
    """Share of pixels that differ from the most common one (0 = blank)."""
    parts = data.split(b"\n", 3)
    if len(parts) < 4 or parts[0] != b"P6":
        return None
    w, h = map(int, parts[1].split())
    px = parts[3][: w * h * 3]
    counts = {}
    for i in range(0, len(px), 3 * 7):  # sampled
        k = px[i:i + 3]
        counts[k] = counts.get(k, 0) + 1
    total = sum(counts.values())
    return 1 - max(counts.values()) / total if total else None


# ---- 1. the console VM on the node
_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
ADMIN = r.get("ticket") or r.get("token")
_, BASE_SETTINGS = call("GET", "/console/settings", token=ADMIN)


def settings(**wan):
    s = json.loads(json.dumps(BASE_SETTINGS))
    s["wan"].update(wan)
    _, saved = call("PUT", "/console/settings", s, token=ADMIN)
    return saved


if RELAY_LISTEN == "all":
    RELAY_LISTEN = None
elif not RELAY_LISTEN:
    RELAY_LISTEN = None if BASE_SETTINGS["wan"].get("relay_reply_from_request_address") else ADDR
print(f"ok - relay run listens on {RELAY_LISTEN or 'every address'}", flush=True)

VM = os.environ.get("HXD_CONSOLE_VM", "hxd-alpine")
call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 1, "memory": 512, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "vnc", "video": "std", "sound": "none"},
    "hardware": {"nets": [], "cdrom": ALPINE_ISO, "boot": ["cdrom", "disk"]},
}, token=ADMIN)
call("POST", f"/vms/{VM}/start", token=ADMIN, ok=(200, 202))
der = ssl.PEM_cert_to_DER_cert(ssl.get_server_certificate(("127.0.0.1", 8006)))
FP = hashlib.sha256(der).hexdigest()
print(f"ok - node certificate sha256 {FP}", flush=True)

# ---- 2. the Windows client machine
os.makedirs(os.path.join(WORK, "tpm"), exist_ok=True)
disk = os.path.join(WORK, "client.qcow2")
subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-b", os.path.abspath(WIN_IMAGE), "-F", "qcow2", disk],
               check=True)
vars_fd = os.path.join(WORK, "vars.fd")
shutil.copy(first(OVMF_VARS), vars_fd)
tpm_sock, qmp_sock, qga_sock = (os.path.join(WORK, n) for n in ("tpm.sock", "qmp.sock", "qga.sock"))
subprocess.run(["swtpm", "socket", "--tpm2", "--tpmstate", "dir=" + os.path.join(WORK, "tpm"),
                "--ctrl", "type=unixio,path=" + tpm_sock, "--daemon"], check=True)
kvm = os.access("/dev/kvm", os.W_OK)
argv = [
    "qemu-system-x86_64", "-name", "hxd-win-client",
    "-machine", "q35,smm=on" + (",accel=kvm" if kvm else ""),
    "-global", "driver=cfi.pflash01,property=secure,value=on",
    "-cpu", "host,hv-relaxed,hv-vapic,hv-spinlocks=0x1fff,hv-time" if kvm else "max",
    "-smp", CPUS, "-m", MEM,
    "-drive", "if=pflash,unit=0,format=raw,readonly=on,file=" + first(OVMF_CODE),
    "-drive", "if=pflash,unit=1,format=raw,file=" + vars_fd,
    "-chardev", "socket,id=chrtpm,path=" + tpm_sock,
    "-tpmdev", "emulator,id=tpm0,chardev=chrtpm", "-device", "tpm-crb,tpmdev=tpm0",
    "-drive", f"file={disk},if=none,id=d0,format=qcow2,cache=unsafe,discard=unmap",
    "-device", "virtio-blk-pci,drive=d0,bootindex=1",
    "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0",
    "-chardev", f"socket,id=qga0,path={qga_sock},server=on,wait=off",
    "-device", "virtio-serial-pci",
    "-device", "virtserialport,chardev=qga0,name=org.qemu.guest_agent.0",
    "-device", "qemu-xhci", "-device", "usb-tablet",
    "-qmp", f"unix:{qmp_sock},server=on,wait=off",
    "-vga", "std", "-display", "none",
    "-pidfile", os.path.join(WORK, "qemu.pid"),
]
qlog = open(os.path.join(WORK, "qemu.log"), "w")
q = subprocess.Popen(argv, stdout=qlog, stderr=subprocess.STDOUT)
t0 = time.time()
qga = Qga(qga_sock)
info = None
while time.time() - t0 < AGENT_WAIT:
    if q.poll() is not None:
        check(False, f"Windows client VM exited ({q.returncode}): {open(os.path.join(WORK, 'qemu.log')).read()[-1500:]}")
    try:
        info = qga("guest-get-osinfo", timeout=5)
        break
    except (OSError, ValueError, RuntimeError):
        time.sleep(10)
if info is None:
    try:
        qmp(qmp_sock, "screendump", filename=os.path.join(OUT, "win-stuck.ppm"))
        ppm_png(os.path.join(OUT, "win-stuck.ppm"))
    except OSError:
        pass
check(info is not None, f"Windows client: guest agent answers ({time.time() - t0:.0f} s after start)")
print(f"   client OS {info.get('pretty-name')} {info.get('kernel-release')}", flush=True)
RESULTS["client_os"] = f"{info.get('pretty-name')} {info.get('kernel-release')}"

# ---- 3. the client program, copied in through the guest agent
rc, out, err = qga.ps(f"New-Item -ItemType Directory -Force '{GUEST_DIR}' | Out-Null; "
                      f"try {{ Add-MpPreference -ExclusionPath '{GUEST_DIR}' -ErrorAction Stop }} catch {{ }}; 'ok'")
check(rc == 0 and "ok" in out, f"Windows client: work directory {GUEST_DIR} ({rc} {err.strip()[:200]})")
exe = GUEST_DIR + r"\horcrux-console.exe"
t1 = time.time()
qga.put(EXE, exe)
want = hashlib.sha256(open(EXE, "rb").read()).hexdigest()
rc, out, err = qga.ps(f"(Get-FileHash -Algorithm SHA256 '{exe}').Hash")
check(out.strip().lower() == want, f"horcrux-console.exe ({os.path.getsize(EXE)} bytes) copied in through the guest "
      f"agent in {time.time() - t1:.0f} s, SHA-256 matches in the guest ({out.strip()[:16].lower()}...)")

# the guest agent's environment (SYSTEM), plus the password variable: a
# guest-exec env list replaces the whole environment
rc, out, err = qga.ps("Get-ChildItem env: | ForEach-Object { $_.Name + '=' + $_.Value }")
ENV = [ln for ln in out.splitlines() if "=" in ln and not ln.startswith("HORCRUX_")]
ENV += ["HORCRUX_PASSWORD=admin", "RUST_LOG=info"]

# the Alpine console must be up (the Windows boot usually covers it)
end = time.time() + 180
st = None
while time.time() < end:
    _, st = call("GET", f"/console/{VM}/hxc/status", token=ADMIN, ok=None)
    if isinstance(st, dict) and st.get("available"):
        break
    time.sleep(3)
check(isinstance(st, dict) and st.get("available"), f"the node's Horcrux Console for {VM} is available")
time.sleep(max(0, 45 - (time.time() - t0)))  # Alpine reaches its login prompt

# ---- 4. headless runs, direct and through the relay
for name, wan in (("desktop_win_direct", dict(enabled=False)),
                  ("desktop_win_relay", dict(enabled=True, force_relay=True, relay_listen_address=RELAY_LISTEN))):
    saved = settings(**wan)
    rs = {}
    if wan.get("enabled"):
        end = time.time() + 30
        while time.time() < end:
            _, rs = call("GET", "/console/relay", token=ADMIN)
            if rs.get("running"):
                break
            time.sleep(1)
        check(rs.get("running"), f"{name}: the node's relay runs ({rs.get('listen')})")
    snap = f"{GUEST_DIR}\\{name}.ppm"
    args = ["--headless", "--node", f"https://{ADDR}:8006", "--user", "admin", "--vm", VM,
            "--cert-sha256", FP, "--warmup-secs", "4", "--trials", "12", "--exercise-input",
            "--type-text", TYPED, "--snapshot", snap]
    t1 = time.time()
    rc, out, err = qga.exec(exe, args, env=ENV, wait=RUN_WAIT)
    with open(os.path.join(OUT, f"{name}.log"), "w") as f:
        f.write(out + "\n---- stderr\n" + err)
    check(rc == 0, f"{name}: horcrux-console.exe ran in the Windows guest in {time.time() - t1:.0f} s "
          f"(exit {rc}: {err.strip()[-300:]})")
    lines = [ln for ln in out.splitlines() if ln.startswith("{")]
    check(bool(lines), f"{name}: the client printed its JSON report")
    rep = json.loads(lines[-1])
    stats = (rep.get("node") or {}).get("stats") or {}
    lat = [x for x in rep.get("key_to_frame_ms") or [] if x is not None]
    want_path = "relay" if wan.get("force_relay") else "direct"
    res = {"path": stats.get("path"), "client_path": rep.get("path"), "decoder": rep.get("decoder"),
           "frames_decoded": rep.get("frames_decoded"), "decode_errors": rep.get("decode_errors"),
           "size": [rep.get("width"), rep.get("height")], "key_to_frame_median_ms": rep.get("key_to_frame_median_ms"),
           "key_to_frame_p90_ms": rep.get("key_to_frame_p90_ms"),
           "node": {k: stats.get(k) for k in ("keys_in", "pointer_in", "gamepad_in", "gamepad_rx", "encoder_impl",
                                              "client_refresh_hz", "frames_sent")}}
    RESULTS[name] = res
    print("PATH " + json.dumps({name: res}), flush=True)
    check(rep.get("frames_decoded", 0) >= 10 and rep.get("decode_errors") == 0,
          f"{name}: {rep.get('frames_decoded')} frames decoded ({rep.get('width')}x{rep.get('height')}), "
          f"{rep.get('decode_errors')} decode errors ({rep.get('last_decode_error')})")
    check(str(rep.get("decoder", "")).startswith("mediafoundation"),
          f"{name}: decoded by Windows Media Foundation ({rep.get('decoder')})")
    check(stats.get("path") == want_path and rep.get("path") == want_path,
          f"{name}: node and client see the {want_path} path ({stats.get('path')} / {rep.get('path')})")
    check((stats.get("keys_in") or 0) >= 24, f"{name}: the node took the keystrokes ({stats.get('keys_in')})")
    check((stats.get("gamepad_rx") or 0) >= 2, f"{name}: the node received the gamepad states "
          f"({stats.get('gamepad_rx')}; handed to guest tools: {stats.get('gamepad_in')})")
    check(len(lat) >= 6, f"{name}: {len(lat)} of 12 keystrokes came back as a new picture "
          f"(median {rep.get('key_to_frame_median_ms')} ms)", fatal=False)
    local = os.path.join(OUT, f"{name}.ppm")
    try:
        data = qga.get(snap)
        open(local, "wb").write(data)
        busy = ppm_busy_fraction(data)
        ppm_png(local)
    except (OSError, RuntimeError, ValueError) as e:
        busy = None
        print(f"   snapshot: {e}", flush=True)
    check(busy is not None and busy > 0.001, f"{name}: the decoded snapshot came back from the guest and is not "
          f"blank ({busy if busy is None else round(busy, 4)} of the picture differs from the background)")

settings(enabled=False)
try:
    qmp(qmp_sock, "screendump", filename=os.path.join(OUT, "win-client.ppm"))
    ppm_png(os.path.join(OUT, "win-client.ppm"))
except OSError:
    pass
q.kill()
dump()
print("RESULTS " + json.dumps(RESULTS), flush=True)
sys.exit(1 if FAILS else 0)
