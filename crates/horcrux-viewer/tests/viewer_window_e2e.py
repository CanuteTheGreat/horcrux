#!/usr/bin/env python3
"""The native viewer WINDOW on a real Wayland compositor, against a real
node and guest (viewer_window_e2e.sh starts horcrux-api; this file does the
rest).

  node       a VM with the guest tools CD, the guest-tools virtio-serial port
             and the frame ring (all added by the node) boots Alpine; the tools
             are installed from the CD and run in the guest (FFmpeg from the
             Alpine mirror, user-mode NIC hot-plugged through QMP)
  compositor headless sway (WLR_BACKENDS=headless, pixman renderer, no GPU)
  window     the real `horcrux-viewer --vm ...` signs in to the node, takes
             the ring over the local socket and opens its window; sway
             lists it and gives it the keyboard focus
  picture    the guest paints four colour bands on its console; a grim
             screenshot of the compositor shows the same picture inside the
             window (compared with QMP `screendump` of the guest itself,
             sampled on a grid, aspect-fit like the viewer) while the ring
             sequence advances; the picture of one state does not match the
             screendump of another (the comparison discriminates)
  input      keys typed into the compositor (virtual-keyboard protocol with
             real evdev codes: wl_keys.py) reach the viewer window, go to the
             guest and run a command there (a marker on the guest's serial
             port, and the console turns red, seen again in a screenshot)

Settings (env, defaults in brackets): HXW_VIEWER, HXW_ALPINE_ISO,
HXW_TOOLS_ISO, HXW_DATA (the API's data dir), HXW_RUNTIME_DIR
(XDG_RUNTIME_DIR for the compositor), HXW_OUT, HXW_BASE
[https://127.0.0.1:8006], HXW_VM [hxw-alpine], HXW_MEM_MIB [1024],
HXW_SCREEN [1600x1000], HXW_MIRROR (Alpine mirror root), HXW_MATCH_MIN [0.95]
(grid fraction that must agree), HXW_TOL [48] (per-channel level),
HXW_BOOT_SECS [180], HXW_APK_SECS [420], HXW_KEY_DELAY_MS [40].
"""
import glob
import json
import os
import socket
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wl_keys  # noqa: E402

E = os.environ.get
BASE = E("HXW_BASE", "https://127.0.0.1:8006")
VIEWER = os.environ["HXW_VIEWER"]
ALPINE = os.environ["HXW_ALPINE_ISO"]
TOOLS = os.environ["HXW_TOOLS_ISO"]
DATA = os.environ["HXW_DATA"]
RUNTIME = os.environ["HXW_RUNTIME_DIR"]
OUT = E("HXW_OUT", "/tmp/hxw-e2e")
VM = E("HXW_VM", "hxw-alpine")
MEM = int(E("HXW_MEM_MIB", "1024"))
SCREEN = E("HXW_SCREEN", "1600x1000")
MIRROR = E("HXW_MIRROR", "http://dl-cdn.alpinelinux.org/alpine/latest-stable")
MATCH_MIN = float(E("HXW_MATCH_MIN", "0.95"))
TOL = int(E("HXW_TOL", "48"))
BOOT_SECS = int(E("HXW_BOOT_SECS", "180"))
APK_SECS = int(E("HXW_APK_SECS", "420"))
KEY_DELAY = int(E("HXW_KEY_DELAY_MS", "40"))
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
RESULTS = {}
os.makedirs(OUT, exist_ok=True)


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


def finish_fail(what):
    dump_debug()
    raise SystemExit("FAIL " + what)


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
ADMIN = r.get("ticket") or r.get("token")

# ------------------------------------------------------------ node settings
_, st = call("GET", "/console/settings", token=ADMIN)
st.update(guest_tools=True, guest_tools_iso=TOOLS, guest_tools_auto_install=False, shm_frames=True)
call("PUT", "/console/settings", st, token=ADMIN)

call("POST", f"/vms/{VM}/stop", token=ADMIN, ok=None)
call("DELETE", f"/vms/{VM}", token=ADMIN, ok=None)
call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 2, "memory": MEM, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "vnc", "video": "std"},
    "hardware": {"nets": [], "cdrom": ALPINE, "boot": ["cdrom", "disk"]},
}, token=ADMIN)
call("POST", f"/vms/{VM}/start", token=ADMIN, ok=(200, 202))


def qemu_cmdline():
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            argv = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")
        except OSError:
            continue
        if argv and b"qemu-system" in argv[0] and any(VM.encode() in a for a in argv):
            return [a.decode(errors="replace") for a in argv]
    return []


argv = " ".join(qemu_cmdline())
RING = f"/dev/shm/horcrux-frames-{VM}"
check("ivshmem-plain,memdev=hxfbmem" in argv and "name=org.horcrux.console.0" in argv and "hxgtcd" in argv,
      "the node gave the VM the frame ring, the guest-tools port and the tools CD")


def sockets(suffix):
    for _ in range(60):
        found = glob.glob(f"{DATA}/**/*{suffix}", recursive=True)
        if found:
            return found[0]
        time.sleep(0.5)
    raise SystemExit(f"FAIL no {suffix} for the VM")


SER_LOG = sockets(".serial.log")
SER_SOCK = sockets(".serial.sock")
QMP_SOCK = sockets(".qmp.sock")


def serial_text():
    return open(SER_LOG, errors="replace").read()


def wait_serial(marker, secs):
    end = time.time() + secs
    while time.time() < end:
        if marker in serial_text():
            return True
        time.sleep(0.5)
    return False


ser = socket.socket(socket.AF_UNIX)
for _ in range(30):
    try:
        ser.connect(SER_SOCK)
        break
    except OSError:
        time.sleep(1)


def drain():
    while True:
        try:
            if not ser.recv(4096):
                return
        except OSError:
            return


threading.Thread(target=drain, daemon=True).start()


def stype(text):
    for line in text.splitlines(keepends=True):
        ser.sendall(line.replace("\n", "\r").encode())
        time.sleep(0.2)


def qmp(cmd, **args):
    """One QMP command on its own connection (the node uses the socket too)."""
    last = None
    for _ in range(20):
        try:
            s = socket.socket(socket.AF_UNIX)
            s.settimeout(30)
            s.connect(QMP_SOCK)
            f = s.makefile("rw")
            f.readline()
            for c, a in (("qmp_capabilities", None), (cmd, args or None)):
                f.write(json.dumps({"execute": c, **({"arguments": a} if a else {})}) + "\n")
                f.flush()
                while True:
                    r = json.loads(f.readline())
                    if "return" in r or "error" in r:
                        break
            s.close()
            return r
        except (OSError, ValueError) as e:
            last = e
            time.sleep(1)
    raise SystemExit(f"FAIL QMP {cmd}: {last}")


# ------------------------------------------------------------ guest setup
if not wait_serial("login:", BOOT_SECS):
    finish_fail("Alpine did not reach its login prompt")
stype("root\n")
time.sleep(3)
# the node gives a console VM no NIC; QEMU user-mode networking is added to the
# running guest for the FFmpeg download
r = qmp("netdev_add", type="user", id="hxwn0")
r2 = qmp("device_add", driver="virtio-net-pci", netdev="hxwn0", id="hxwnic")
check("error" not in r and "error" not in r2, f"user-mode NIC hot-plugged for the package download ({r2})")
stype("sleep 2; ip link set eth0 up; udhcpc -i eth0 -q -n -t 10 2>&1 | tail -1; echo HX_$((40+2))_NET $?\n")
if not wait_serial("HX_42_NET", 90):
    finish_fail("no network in the guest")
stype(f"echo {MIRROR}/main >/etc/apk/repositories; echo {MIRROR}/community >>/etc/apk/repositories\n")
stype("apk add -q ffmpeg >/dev/ttyS0 2>&1; echo HX_$((40+3))_APK $?\n")
if not wait_serial("HX_43_APK 0", APK_SECS):
    finish_fail("apk add ffmpeg failed: " + serial_text()[-400:])
PAINT = r"""A="$*"
set -- $(stty size </dev/tty1); R=$1; C=$2
awk -v r=$R -v c=$C -v cols="$A" 'BEGIN{n=split(cols,k," "); printf "\033[?25l\033[H"; for(y=0;y<r;y++){printf "\033[%sm",k[int(y*n/r)+1]; for(x=0;x<c;x++){if(y==r-1&&x==c-1)break; printf " "}} printf "\033[H"}' >/dev/tty1
"""
stype("cat > /tmp/paint.sh <<'HXEOF'\n" + PAINT + "HXEOF\n")
stype("echo 0 > /sys/class/graphics/fbcon/cursor_blink; modprobe virtio_scsi; modprobe sr_mod; modprobe iso9660; modprobe uinput; sleep 1; "
      "mkdir -p /mnt/t; mount -o ro $(findfs LABEL=HXTOOLS) /mnt/t && echo HX_$((40+4))_MOUNT\n")
check(wait_serial("HX_44_MOUNT", 40), "the node's tools CD (virtio-scsi) mounts in the guest")
stype("(/mnt/t/linux/horcrux-guest-agent >/dev/ttyS0 2>&1 &); sleep 3; "
      "/mnt/t/linux/horcrux-guest-agent --print-caps | grep shm_frames; echo HX_$((40+5))_CAPS\n")
if not (wait_serial("HX_45_CAPS", 40) and '"shm_frames": true' in serial_text()):
    finish_fail("guest tools did not report the frame ring: " + serial_text()[-600:])
check(True, "guest tools run in the guest and found the ring on the ivshmem BAR")
# four horizontal bands: red, green, blue, magenta (ANSI background codes)
stype("sh /tmp/paint.sh 41 42 44 45; echo HX_$((40+6))_PAINT\n")
check(wait_serial("HX_46_PAINT", 20), "the guest console shows a four-band test picture")


# ------------------------------------------------------------ pictures
def read_ppm(path):
    d = open(path, "rb").read()
    toks, i = [], 0
    while len(toks) < 4:
        while d[i:i + 1].isspace():
            i += 1
        if d[i:i + 1] == b"#":
            while d[i:i + 1] != b"\n":
                i += 1
            continue
        j = i
        while not d[j:j + 1].isspace():
            j += 1
        toks.append(d[i:j])
        i = j
    i += 1
    w, h = int(toks[1]), int(toks[2])
    return w, h, d[i:i + w * h * 3]


def px(img, x, y):
    w, h, d = img
    o = (min(y, h - 1) * w + min(x, w - 1)) * 3
    return d[o], d[o + 1], d[o + 2]


def fit_rect(sw, sh, gw, gh):
    """Where the viewer draws a gw x gh guest picture in a sw x sh window."""
    scale = min(sw / gw, sh / gh)
    vw, vh = max(1, int(gw * scale)), max(1, int(gh * scale))
    return (sw - vw) // 2, (sh - vh) // 2, vw, vh


def agreement(shot, guest, nx=64, ny=40):
    """Fraction of grid points where the window shows the guest's colour."""
    sw, sh, _ = shot
    gw, gh, _ = guest
    ox, oy, vw, vh = fit_rect(sw, sh, gw, gh)
    same = 0
    for j in range(ny):
        for i in range(nx):
            a = px(shot, ox + int((i + .5) * vw / nx), oy + int((j + .5) * vh / ny))
            b = px(guest, int((i + .5) * gw / nx), int((j + .5) * gh / ny))
            same += all(abs(p - q) <= TOL for p, q in zip(a, b))
    return same / (nx * ny)


ENV_WL = {}


def wl_env():
    disp = sorted(p for p in glob.glob(os.path.join(RUNTIME, "wayland-*")) if not p.endswith(".lock"))
    return dict(os.environ, XDG_RUNTIME_DIR=RUNTIME, WAYLAND_DISPLAY=os.path.basename(disp[0]) if disp else "")


def grim(name):
    """Screenshot of the compositor output: PPM to compare, PNG for the artifact."""
    ppm = os.path.join(OUT, name + ".ppm")
    subprocess.run(["grim", "-t", "ppm", ppm], check=True, env=wl_env(), timeout=60)
    subprocess.run(["grim", "-t", "png", os.path.join(OUT, name + ".png")], check=False, env=wl_env(), timeout=60)
    return read_ppm(ppm)


def screendump(name):
    path = os.path.join(OUT, name + ".guest.ppm")
    r = qmp("screendump", filename=path, format="ppm")
    if "error" in r:
        raise SystemExit(f"FAIL screendump: {r}")
    return read_ppm(path)


def wait_match(name, secs=30):
    """Screenshot + guest screendump pairs until the window shows the guest."""
    end, best = time.time() + secs, (0.0, None, None)
    while True:
        g = screendump(name)
        s = grim(name)
        a = agreement(s, g)
        if a > best[0]:
            best = (a, s, g)
        if a >= MATCH_MIN or time.time() > end:
            return best
        time.sleep(1)


def ring_seq():
    with open(RING, "rb") as f:
        f.seek(64)
        return int.from_bytes(f.read(8), "little") >> 8


def dump_debug():
    print("DEBUG serial tail:", serial_text()[-1500:], flush=True)
    try:
        print("DEBUG sway tree:", json.dumps([(x.get("type"), x.get("name"), x.get("app_id")) for x in flat(tree())]), flush=True)
    except Exception as e:  # noqa: BLE001
        print("DEBUG no sway tree:", e, flush=True)
    for n in ("sway.log", "viewer.log"):
        p = os.path.join(OUT, n)
        if os.path.exists(p):
            print(f"DEBUG {n}:", open(p, errors="replace").read()[-1500:], flush=True)


# ------------------------------------------------------------ compositor
w, h = SCREEN.split("x")
cfg = os.path.join(RUNTIME, "sway.conf")
open(cfg, "w").write(f"output * resolution {w}x{h}\ndefault_border none\ndefault_floating_border none\n"
                     "focus_follows_mouse no\n")
sway_env = dict(os.environ, XDG_RUNTIME_DIR=RUNTIME, WLR_BACKENDS="headless", WLR_LIBINPUT_NO_DEVICES="1",
                WLR_RENDERER="pixman", XDG_CONFIG_HOME=os.path.join(RUNTIME, "cfg"))
sway_env.pop("WAYLAND_DISPLAY", None)
sway_env.pop("DISPLAY", None)
sway = subprocess.Popen(["sway", "-c", cfg], env=sway_env, stdout=open(os.path.join(OUT, "sway.log"), "w"),
                        stderr=subprocess.STDOUT)
open(os.path.join(RUNTIME, "sway.pid"), "w").write(str(sway.pid))
for _ in range(60):
    if sway.poll() is not None:
        finish_fail("sway exited")
    if wl_env()["WAYLAND_DISPLAY"] and glob.glob(os.path.join(RUNTIME, "sway-ipc.*.sock")):
        break
    time.sleep(0.5)
os.environ.update(XDG_RUNTIME_DIR=RUNTIME, WAYLAND_DISPLAY=wl_env()["WAYLAND_DISPLAY"])  # for wl_keys
SWAYSOCK = glob.glob(os.path.join(RUNTIME, "sway-ipc.*.sock"))[0]


def tree():
    p = subprocess.run(["swaymsg", "-s", SWAYSOCK, "-t", "get_tree", "-r"], capture_output=True, text=True, timeout=30)
    return json.loads(p.stdout)


def flat(node, acc=None):
    acc = [] if acc is None else acc
    acc.append(node)
    for c in node.get("nodes", []) + node.get("floating_nodes", []):
        flat(c, acc)
    return acc


def windows(node, acc=None):
    acc = [] if acc is None else acc
    if node.get("type") in ("con", "floating_con") and node.get("shell") in ("xdg_shell", "xwayland"):
        acc.append(node)
    for c in node.get("nodes", []) + node.get("floating_nodes", []):
        windows(c, acc)
    return acc


bg = grim("compositor-empty")
check((bg[0], bg[1]) == (int(w), int(h)), f"headless sway (pixman, no GPU) runs a {w}x{h} output")

# ------------------------------------------------------------ the viewer window
venv = wl_env()
venv.pop("DISPLAY", None)
venv.update(HORCRUX_PASSWORD="admin", HOME=os.path.join(RUNTIME, "home"), XDG_CONFIG_HOME=os.path.join(RUNTIME, "home/.config"))
os.makedirs(venv["HOME"], exist_ok=True)
viewer = subprocess.Popen([VIEWER, "--vm", VM, "--user", "admin"], env=venv,
                          stdout=open(os.path.join(OUT, "viewer.log"), "w"), stderr=subprocess.STDOUT)
win = None
for _ in range(60):
    if viewer.poll() is not None:
        finish_fail(f"horcrux-viewer exited with {viewer.returncode}")
    ws = [x for x in windows(tree()) if VM in (x.get("name") or "")]
    if ws:
        win = ws[0]
        break
    time.sleep(0.5)
if win is None:
    finish_fail("no viewer window in the compositor")
RESULTS["window"] = {k: win.get(k) for k in ("name", "app_id", "shell", "focused", "rect")}
check(win.get("shell") == "xdg_shell" and "Horcrux Console" in win["name"] and win.get("focused"),
      f"horcrux-viewer opened its window on Wayland (xdg_shell, title {win['name']!r}, keyboard focus)")

# ------------------------------------------------------------ frames reach the window
s0 = ring_seq()
time.sleep(2)
s1 = ring_seq()
check(s1 - s0 >= 10, f"the guest tools write raw frames into the ring while the window reads (sequence {s0} -> {s1})")
a1, shot1, g1 = wait_match("window-bands")
RESULTS["bands_agreement"] = a1
if a1 < MATCH_MIN:
    finish_fail(f"the window does not show the guest picture (grid agreement {a1:.3f} < {MATCH_MIN})")
# the bands are really there in the window: red, green, blue, magenta rows
sw, sh, _ = shot1
ox, oy, vw, vh = fit_rect(sw, sh, g1[0], g1[1])
cols = [px(shot1, ox + vw // 2, oy + int(vh * f)) for f in (0.125, 0.375, 0.625, 0.875)]
dom = lambda c: max(range(3), key=lambda i: c[i])  # noqa: E731
check(dom(cols[0]) == 0 and dom(cols[1]) == 1 and dom(cols[2]) == 2 and cols[3][0] > 100 and cols[3][2] > 100
      and cols[3][1] < 60, f"the window shows the guest's red/green/blue/magenta bands {cols}")
check(len({c for c in cols}) == 4, "four distinct colours in the window")
print(f"ok - window screenshot matches the guest's own screen on {a1:.1%} of the grid (needs {MATCH_MIN:.0%})", flush=True)

# ------------------------------------------------------------ input: keys typed into the compositor
def keys_in():
    _, sess = call("GET", f"/console/{VM}/hxc/sessions", token=ADMIN, ok=None)
    return [x["stats"]["keys_in"] for x in (sess or []) if x["stats"].get("video_source") == "shm"]


ser_before = serial_text().count("HXWIN42")
got = False
for attempt in range(3):
    print(f"typing into the compositor, attempt {attempt + 1} (keys_in so far {keys_in()})", flush=True)
    wl_keys.type_text("root\n", KEY_DELAY)  # tty1's login (a no-op command when already logged in)
    time.sleep(4)
    grim(f"typed-login-{attempt + 1}")
    wl_keys.type_text("echo HXWIN$((6*7)) >/dev/ttyS0; sh /tmp/paint.sh 41 41 41 41\n", KEY_DELAY)
    end = time.time() + 20
    while time.time() < end and serial_text().count("HXWIN42") <= ser_before:
        time.sleep(0.5)
    if serial_text().count("HXWIN42") > ser_before:
        got = True
        break
if not got:
    _, sess = call("GET", f"/console/{VM}/hxc/sessions", token=ADMIN, ok=None)
    print("DEBUG sessions", json.dumps(sess)[:1500], flush=True)
    finish_fail("keys typed into the compositor did not run a command in the guest (no HXWIN42 on its serial port)")
open(os.path.join(OUT, "guest-serial.log"), "w").write(serial_text())
check(True, "keys typed into the compositor reached the viewer window and ran a command in the guest (HXWIN42 on its serial port)")
_, sess = call("GET", f"/console/{VM}/hxc/sessions", token=ADMIN, ok=None)
loc = [x for x in (sess or []) if x["stats"].get("video_source") == "shm"]
check(loc and loc[0]["stats"]["keys_in"] >= 20, f"the node counts the viewer's keys (keys_in {loc[0]['stats']['keys_in'] if loc else None})")
a2, shot2, g2 = wait_match("window-after-keys")
RESULTS["after_keys_agreement"] = a2
sw, sh, _ = shot2
ox, oy, vw, vh = fit_rect(sw, sh, g2[0], g2[1])
red = px(shot2, ox + vw // 2, oy + vh // 2)
check(a2 >= MATCH_MIN and red[0] > 120 and red[1] < 60 and red[2] < 60,
      f"the typed command changed the guest console and the window followed (all red {red}, agreement {a2:.1%})")
cross = agreement(shot2, g1)
check(cross < 0.5, f"the comparison discriminates: the red window does not match the earlier band picture ({cross:.1%})")

# ------------------------------------------------------------ close
viewer.terminate()
try:
    viewer.wait(timeout=10)
except subprocess.TimeoutExpired:
    viewer.kill()
sway.terminate()
call("POST", f"/vms/{VM}/stop", token=ADMIN, ok=None)
json.dump(RESULTS, open(os.path.join(OUT, "viewer-window-results.json"), "w"), indent=1, default=str)
print("PASS viewer_window_e2e", flush=True)
