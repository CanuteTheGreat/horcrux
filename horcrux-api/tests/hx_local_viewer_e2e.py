#!/usr/bin/env python3
"""Horcrux Console same-host viewer, node half, end to end against a real
horcrux-api and a real QEMU guest (hx_local_viewer_e2e.sh starts both):

  settings   frame ring / local socket settings have their defaults and
             are validated
  device     a console VM gets the frame ring (ivshmem-plain over a root-only
             file under /dev/shm whose header carries the ring magic) and
             QEMU's D-Bus audio
  rbac       local tickets need VM.Console (403 otherwise); a view-only user
             gets input=false
  handover   horcrux-viewer signs in, gets a local ticket and receives the
             ring's descriptor over the local socket (SCM_RIGHTS)
  binding    a ticket bound to another uid, or reused, is refused
  input      keys sent over the local socket reach the guest (Alpine runs a
             command typed that way; seen on its serial console); a
             view-only ticket's keys are refused and counted
  sessions   the local session is listed with video_source shm and counts;
             audit has start/end entries

Env: HXL_VIEWER (horcrux-viewer binary), HXL_ALPINE_ISO, HXL_DATA (the
API's data dir), HXL_OUT.
"""
import array
import json
import os
import socket
import ssl
import stat
import struct
import subprocess
import time
import urllib.error
import urllib.request

BASE = os.environ.get("HXL_BASE", "https://127.0.0.1:8006")
VIEWER = os.environ["HXL_VIEWER"]
ALPINE = os.environ["HXL_ALPINE_ISO"]
DATA = os.environ["HXL_DATA"]
OUT = os.environ.get("HXL_OUT", "/tmp/hxl-e2e")
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
RESULTS = {}


def call(method, path, body=None, token=None, ok=(200, 201, 202, 204)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + "/api" + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=120) as r:
            status, text = r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if ok is not None and status not in ok:
        raise SystemExit(f"FAIL {method} {path} -> {status}: {text[:500]}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL " + what)
    print("ok -", what, flush=True)


def login(user, pw):
    _, r = call("POST", "/auth/login", {"username": user, "password": pw}, ok=(200,))
    return r.get("ticket") or r.get("token")


ADMIN = login("admin", "admin")
os.makedirs(OUT, exist_ok=True)

# ---------------------------------------------------------------- settings
_, s = call("GET", "/console/settings", token=ADMIN)
check(s["shm_frames"] and s["shm_frames_size_mib"] == 128 and s["shm_frames_slots"] == 3
      and s["shm_frames_dir"] == "/dev/shm" and s["local_socket"] == "/run/horcrux/console.sock"
      and s["audio_frame_ms"] == 10,
      "frame ring, local socket and audio frame settings have their documented defaults")
st, err = call("PUT", "/console/settings", dict(s, shm_frames_size_mib=100), token=ADMIN, ok=None)
check(st in (400, 422) and "power of two" in json.dumps(err), "a frame ring size that is not a power of two is refused")
call("PUT", "/console/settings", dict(s, shm_frames_size_mib=64), token=ADMIN)

# ---------------------------------------------------------------- users
call("POST", "/roles", {"name": "HxlViewer", "description": "console view only",
                        "privileges": ["VM.Audit", "VM.Console"]}, token=ADMIN, ok=None)
for u in ("hxlview", "hxlnone"):
    call("POST", "/users", {"username": u, "password": "Hxl-e2e-Pass-1!",
                            "email": f"{u}@errant.solutions"}, token=ADMIN, ok=None)
call("POST", "/acl", {"path": "/vms", "subject_type": "user", "subject": "hxlview", "role": "HxlViewer"},
     token=ADMIN, ok=(200, 201, 409))

# ---------------------------------------------------------------- VM
VM = "hxl-alpine"
call("POST", f"/vms/{VM}/stop", token=ADMIN, ok=None)
call("DELETE", f"/vms/{VM}", token=ADMIN, ok=None)
call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 2, "memory": 768, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "vnc", "video": "std", "sound": "ich9-intel-hda"},
    "hardware": {"nets": [], "cdrom": ALPINE, "boot": ["cdrom", "disk"]},
}, token=ADMIN)
call("POST", f"/vms/{VM}/start", token=ADMIN, ok=(200, 202))


def qemu_cmdline(vm_id):
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            argv = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")
        except OSError:
            continue
        if argv and b"qemu-system" in argv[0] and any(vm_id.encode() in a for a in argv):
            return [a.decode(errors="replace") for a in argv]
    return []


argv = " ".join(qemu_cmdline(VM))
ring = f"/dev/shm/horcrux-frames-{VM}"
check(f"memory-backend-file,id=hxfbmem,size=64M,mem-path={ring},share=on" in argv
      and "ivshmem-plain,memdev=hxfbmem,id=hxframes" in argv,
      "the console VM has the frame ring device (ivshmem-plain over the ring file, size from the settings)")
check("-audiodev dbus,id=snd0" in argv, "guest audio goes to QEMU's D-Bus audio (no tools needed)")
stt = os.stat(ring)
with open(ring, "rb") as f:
    head = f.read(8)
check(stat.S_IMODE(stt.st_mode) == 0o600 and stt.st_uid == 0 and stt.st_size == 64 << 20 and head == b"HXFRAME1",
      "ring file: root-only (0600), 64 MiB, header written by the node (magic HXFRAME1)")

# ---------------------------------------------------------------- rbac
uid = os.getuid()
t_none = login("hxlnone", "Hxl-e2e-Pass-1!")
st, _ = call("POST", f"/console/{VM}/hxc/local-ticket", {"uid": uid}, token=t_none, ok=None)
check(st == 403, f"a user without VM.Console gets no local ticket ({st})")
t_view = login("hxlview", "Hxl-e2e-Pass-1!")
_, vt = call("POST", f"/console/{VM}/hxc/local-ticket", {"uid": uid}, token=t_view)
check(vt["input"] is False and vt["ticket"].startswith("hxc-"), "a view-only user gets a local ticket without input")


# ---------------------------------------------------------------- local socket client
def frame(kind, payload):
    return struct.pack("<I", len(payload) + 1) + bytes([kind]) + payload


def open_local(sock_path, ticket):
    c = socket.socket(socket.AF_UNIX)
    c.connect(sock_path)
    c.sendall(frame(0x01, json.dumps({"v": 1, "ticket": ticket}).encode()))
    head, fds, _, _ = socket.recv_fds(c, 4, 1)
    while len(head) < 4:
        more = c.recv(4 - len(head))
        if not more:
            break
        head += more
    n = struct.unpack("<I", head)[0]
    body = b""
    while len(body) < n:
        body += c.recv(n - len(body))
    return c, body[0], json.loads(body[1:]), fds


def keys(c, text):
    names = {" ": "Space", "\n": "Enter", "$": ("ShiftLeft", "Digit4"), "(": ("ShiftLeft", "Digit9"),
             ")": ("ShiftLeft", "Digit0"), "*": ("ShiftLeft", "Digit8"), ">": ("ShiftLeft", "Period"),
             "/": "Slash", "_": ("ShiftLeft", "Minus")}
    for ch in text:
        k = names.get(ch)
        if k is None:
            k = f"Digit{ch}" if ch.isdigit() else ("ShiftLeft", f"Key{ch}") if ch.isupper() else f"Key{ch.upper()}"
        mods, code = ((), k) if isinstance(k, str) else ((k[0],), k[1])
        for m in mods:
            c.sendall(frame(0x10, json.dumps({"t": "down", "code": m}).encode()))
        c.sendall(frame(0x10, json.dumps({"t": "down", "code": code}).encode()))
        c.sendall(frame(0x10, json.dumps({"t": "up", "code": code}).encode()))
        for m in mods:
            c.sendall(frame(0x10, json.dumps({"t": "up", "code": m}).encode()))
        time.sleep(0.04)


SOCK = s["local_socket"]
# binding: a ticket for another uid is refused, and not burnt
_, other = call("POST", f"/console/{VM}/hxc/local-ticket", {"uid": uid + 4242}, token=ADMIN)
c, k, msg, fds = open_local(SOCK, other["ticket"])
check(k == 0x03 and "another user" in msg.get("message", "") and not fds,
      f"a local ticket bound to another uid is refused ({msg.get('message')})")
c.close()

# ---------------------------------------------------------------- handover (the real viewer)
env = dict(os.environ, HORCRUX_PASSWORD="admin")
p = subprocess.run([VIEWER, "measure", "--vm", VM, "--user", "admin", "--secs", "2"],
                   capture_output=True, text=True, env=env, timeout=60)
welcome = None
for line in p.stderr.splitlines():
    if line.startswith("welcome: "):
        welcome = json.loads(line[len("welcome: "):])
summary = json.loads(p.stdout.strip().splitlines()[-1]) if p.stdout.strip() else {}
RESULTS["viewer_welcome"] = welcome
check(welcome is not None and welcome["frame_ring_bytes"] == 64 << 20 and welcome["input"] is True
      and welcome["vm"] == VM,
      f"horcrux-viewer signed in, redeemed a local ticket and mapped the ring it received over the socket ({welcome})")
check(summary.get("frames") == 0 and summary.get("writer") == "",
      "without guest tools nothing writes frames (the viewer sees an empty ring, no fake frames)")

# ---------------------------------------------------------------- input through the local socket
serial_logs = []
for root, _, files in os.walk(DATA):
    serial_logs += [os.path.join(root, f) for f in files if f.endswith(".serial.log")]
check(len(serial_logs) == 1, f"the VM's serial log exists ({serial_logs})")
SER = serial_logs[0]


def serial_text():
    return open(SER, errors="replace").read()


deadline = time.time() + 150
while time.time() < deadline and "login:" not in serial_text():
    time.sleep(2)
check("login:" in serial_text(), "Alpine booted to its login prompt")
# the serial getty comes up first; give the VGA console's getty time too
time.sleep(int(os.environ.get("HXL_CONSOLE_SETTLE_SECS", "20")))

_, at = call("POST", f"/console/{VM}/hxc/local-ticket", {"uid": uid}, token=ADMIN)
c, k, w, fds = open_local(SOCK, at["ticket"])
check(k == 0x02 and len(fds) == 1 and w["input"], "admin's local session: welcome with the ring descriptor and input")
st2 = os.fstat(fds[0])
check(st2.st_size == 64 << 20 and st2.st_ino == os.stat(ring).st_ino,
      "the descriptor is the VM's ring file (same inode), though the file itself is root-only")
os.close(fds[0])
keys(c, "root\n")
time.sleep(5)
for attempt in range(2):
    keys(c, "echo HXLOCAL$((6*7)) >/dev/ttyS0\n")
    deadline = time.time() + 15
    while time.time() < deadline and "HXLOCAL42" not in serial_text():
        time.sleep(0.5)
    if "HXLOCAL42" in serial_text():
        break
    # maybe the login was not ready yet: log in again and retry
    keys(c, "root\n")
    time.sleep(5)
if "HXLOCAL42" not in serial_text():
    _, sessions = call("GET", f"/console/{VM}/hxc/sessions", token=ADMIN, ok=None)
    print("DEBUG sessions", json.dumps(sessions)[:1500], flush=True)
    print("DEBUG serial tail", serial_text()[-800:], flush=True)
check("HXLOCAL42" in serial_text(), "keys sent over the local socket ran a command in the guest (HXLOCAL42 on its serial port)")
# ping/pong over the same socket
c.sendall(frame(0x13, json.dumps({"t": "ping", "ts": 7}).encode()))
c.settimeout(5)
buf = b""
pong = False
end = time.time() + 5
while time.time() < end and not pong:
    try:
        buf += c.recv(65536)
    except socket.timeout:
        break
    while len(buf) >= 4 and len(buf) >= 4 + struct.unpack_from("<I", buf)[0]:
        n = struct.unpack_from("<I", buf)[0]
        kind, body = buf[4], buf[5:4 + n]
        buf = buf[4 + n:]
        if kind == 0x21 and b'"pong"' in body:
            pong = True
check(pong, "the node answers a ping on the local socket")
_, sessions = call("GET", f"/console/{VM}/hxc/sessions", token=ADMIN)
loc = [x for x in sessions if x["stats"]["video_source"] == "shm"]
check(loc and loc[0]["stats"]["keys_in"] >= 20 and loc[0]["user"] == "admin",
      f"the local session is listed (video_source shm, keys_in {loc[0]['stats']['keys_in'] if loc else None})")
c.close()

# view-only: keys refused server-side
_, vt2 = call("POST", f"/console/{VM}/hxc/local-ticket", {"uid": uid}, token=t_view)
c, k, w, fds = open_local(SOCK, vt2["ticket"])
for f in fds:
    os.close(f)
check(k == 0x02 and w["input"] is False, "view-only local session: welcome says no input")
before = serial_text().count("HXDENIED")
keys(c, "echo HXDENIED >/dev/ttyS0\n")
time.sleep(3)
_, sessions = call("GET", f"/console/{VM}/hxc/sessions", token=ADMIN)
vo = [x for x in sessions if x["user"] == "hxlview"]
check(serial_text().count("HXDENIED") == before and vo and vo[0]["stats"]["input_denied"] > 0
      and vo[0]["stats"]["keys_in"] == 0,
      f"a view-only user's keys never reach the guest (input_denied {vo[0]['stats']['input_denied'] if vo else None})")
c.close()

# single use
c, k, msg, fds = open_local(SOCK, at["ticket"])
check(k == 0x03 and not fds, f"a used local ticket is refused ({msg.get('message')})")
c.close()

time.sleep(1)
_, audit = call("GET", "/audit/events?limit=200", token=ADMIN, ok=None)
text = json.dumps(audit)
check("local console session started" in text and "local console session ended" in text
      and "local console ticket refused" in text,
      "audit has local session start/end and the refused ticket")

call("POST", f"/vms/{VM}/stop", token=ADMIN, ok=None)
time.sleep(3)
check(not os.path.exists(ring), "the ring file is removed when the VM stops")
json.dump(RESULTS, open(f"{OUT}/hxl-results.json", "w"), indent=1)
print("PASS hx_local_viewer_e2e", flush=True)
