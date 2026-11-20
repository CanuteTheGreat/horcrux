#!/usr/bin/env python3
"""Real-VM test of the Linux Horcrux guest tools through the virtio-serial
guest-tools port, with this script as the node's end of the port.

The VM gets exactly the device Horcrux gives console VMs
(`virtio-serial-pci` + `virtserialport name=org.horcrux.console.0`, socket on
the host) and the guest tools on a CD (label HXTOOLS), the way Horcrux
delivers them. Alpine's virt ISO boots; the script logs in through QEMU key
events, installs FFmpeg from the Alpine mirror, starts the agent from the CD
and then checks over the port:

  hello      capabilities are probed, not assumed
  video      kmsgrab capture + H.264 encode in the guest; the packets decode
             (ffprobe) with the guest's mode size; keyframe request -> IDR
  keyboard   keys injected through the tools (uinput) run a shell command
  latency    keystroke (through the tools) -> first changed encoded frame
             at the node, same host
  gamepad    a uinput gamepad appears and the relative mouse moves
  honesty    no clipboard/resize claimed without a graphical session

Usage: linux_guest_e2e.py AGENT_BINARY ALPINE_VIRT_ISO OUTDIR
Prints `ok - ...` / `FAIL ...` lines and writes OUTDIR/linux-guest-e2e.json.
Needs qemu-system-x86_64 (KVM), xorriso, ffprobe.
"""
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import time

AGENT, ISO, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
PORT_NAME = os.environ.get("HXGT_PORT", "org.horcrux.console.0")
MIRROR = os.environ.get("HXGT_ALPINE_MIRROR", "http://dl-cdn.alpinelinux.org/alpine/latest-stable")
BOOT = int(os.environ.get("HXGT_BOOT_SECS", "35"))
W = os.path.join(OUT, "lgt")
shutil.rmtree(W, ignore_errors=True)
os.makedirs(os.path.join(W, "cd"))
results = {}
fails = 0


def ok(name, msg):
    print(f"ok - {name}: {msg}", flush=True)
    results[name] = {"ok": True, "detail": msg}


def fail(name, msg):
    global fails
    fails += 1
    print(f"FAIL {name}: {msg}", flush=True)
    results[name] = {"ok": False, "detail": msg}


# ---- the tools CD (how Horcrux hands the tools to a VM) ----
shutil.copy(AGENT, os.path.join(W, "cd", "horcrux-guest-agent"))
os.chmod(os.path.join(W, "cd", "horcrux-guest-agent"), 0o755)
subprocess.run(["xorriso", "-as", "mkisofs", "-quiet", "-V", "HXTOOLS", "-J", "-R", "-o",
                os.path.join(W, "tools.iso"), os.path.join(W, "cd")], check=True)

qmp_path, gt_path, serial = (os.path.join(W, n) for n in ("qmp.sock", "hxgt.sock", "serial.log"))
open(serial, "w").close()
qemu = subprocess.Popen([
    "qemu-system-x86_64", "-machine", "q35,accel=kvm:tcg", "-cpu", "host", "-m", "1024", "-smp", "2",
    "-drive", f"file={ISO},media=cdrom,readonly=on,if=none,id=cd0", "-device", "ide-cd,drive=cd0,bus=ide.0,bootindex=1",
    "-drive", f"file={os.path.join(W, 'tools.iso')},media=cdrom,readonly=on,if=none,id=cd1", "-device", "ide-cd,drive=cd1,bus=ide.1",
    "-vga", "std", "-display", "none",
    "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0",
    # the Horcrux guest-tools port, as display::qemu_args builds it
    "-device", "virtio-serial-pci,id=hxgt-vs",
    "-chardev", f"socket,id=hxgt0,path={gt_path},server=on,wait=off",
    "-device", f"virtserialport,bus=hxgt-vs.0,chardev=hxgt0,name={PORT_NAME}",
    "-chardev", f"socket,id=s0,path={serial}.sock,server=on,wait=off,logfile={serial}", "-serial", "chardev:s0",
    "-qmp", f"unix:{qmp_path},server=on,wait=off",
], stdout=subprocess.DEVNULL, stderr=open(os.path.join(W, "qemu.err"), "w"))


def cleanup():
    qemu.kill()


for _ in range(100):
    if os.path.exists(qmp_path) and os.path.exists(gt_path):
        break
    if qemu.poll() is not None:
        break
    time.sleep(0.1)
if qemu.poll() is not None:
    print("FAIL qemu: " + open(os.path.join(W, "qemu.err")).read(), flush=True)
    sys.exit(1)
qs = socket.socket(socket.AF_UNIX)
qs.connect(qmp_path)
qf = qs.makefile("rw")


def qmp(c, **a):
    qf.write(json.dumps({"execute": c, "arguments": a} if a else {"execute": c}) + "\n")
    qf.flush()
    while True:
        r = json.loads(qf.readline())
        if "return" in r or "error" in r:
            return r


qf.readline()
qmp("qmp_capabilities")

PLAIN = {" ": "spc", "\n": "ret", "-": "minus", "=": "equal", "[": "bracket_left", "]": "bracket_right",
         ";": "semicolon", "'": "apostrophe", "`": "grave_accent", "\\": "backslash", ",": "comma",
         ".": "dot", "/": "slash"}
SHIFT = {"_": "minus", "+": "equal", "{": "bracket_left", "}": "bracket_right", ":": "semicolon",
         '"': "apostrophe", "~": "grave_accent", "|": "backslash", "<": "comma", ">": "dot", "?": "slash",
         "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6", "&": "7", "*": "8", "(": "9", ")": "0"}


def keyspec(ch):
    if ch in PLAIN:
        return False, PLAIN[ch]
    if ch in SHIFT:
        return True, SHIFT[ch]
    if ch.isupper():
        return True, ch.lower()
    return False, ch


def qtype(text):
    """Type through QEMU's emulated keyboard (before the tools run)."""
    for ch in text:
        shift, k = keyspec(ch)
        keys = ([{"type": "qcode", "data": "shift"}] if shift else []) + [{"type": "qcode", "data": k}]
        qmp("send-key", keys=keys)
        time.sleep(0.03)


import threading
ser = None


def serial_reader():
    while True:
        try:
            if not ser.recv(4096):
                return
        except OSError:
            return


def stype(text):
    """Type on the guest's serial console (Alpine runs a getty there)."""
    for line in text.splitlines(keepends=True):
        ser.sendall(line.replace("\n", "\r").encode())
        time.sleep(0.2)


def serial_text():
    with open(serial, errors="replace") as f:
        return f.read()


def wait_serial(marker, secs):
    end = time.time() + secs
    while time.time() < end:
        if marker in serial_text():
            return True
        time.sleep(0.5)
    return False


# ---- boot, network, FFmpeg, agent ----
for _ in range(50):
    if os.path.exists(serial + ".sock"):
        break
    time.sleep(0.1)
ser = socket.socket(socket.AF_UNIX)
ser.connect(serial + ".sock")
threading.Thread(target=serial_reader, daemon=True).start()
if not wait_serial("login:", 120):
    fail("setup", "no login prompt on the serial console")
stype("root\n")
time.sleep(3)
stype("ls /sys/class/net; ip link set eth0 up; udhcpc -i eth0 -q -n -t 10 2>&1 | tail -3; echo HX_$((40+2))_NET $?\n")
if not wait_serial("HX_42_NET", 90):
    fail("setup", "no network: " + serial_text()[-500:])
stype(f"echo {MIRROR}/main >/etc/apk/repositories; echo {MIRROR}/community >>/etc/apk/repositories\n")
stype("apk add -q ffmpeg >/dev/ttyS0 2>&1; echo HX_$((40+3))_APK $?\n")
if not wait_serial("HX_43_APK", 300):
    fail("setup", "apk add ffmpeg did not finish: " + serial_text()[-400:])
stype("modprobe uinput; mkdir -p /mnt/t; mount -o ro $(findfs LABEL=HXTOOLS) /mnt/t; "
      "/mnt/t/horcrux-guest-agent >/dev/ttyS0 2>&1 &\n")
time.sleep(2)

# ---- the node's end of the port ----
gs = socket.socket(socket.AF_UNIX)
gs.connect(gt_path)
gs.settimeout(0.5)
rxbuf = bytearray()


def send(kind, payload=b""):
    gs.sendall(struct.pack("<I", len(payload) + 1) + bytes([kind]) + payload)


def send_json(kind, obj):
    send(kind, json.dumps(obj).encode())


def recv_msgs(secs):
    """Yield (kind, payload, t_received) for `secs` seconds."""
    end = time.time() + secs
    while time.time() < end:
        while len(rxbuf) >= 4:
            n = struct.unpack_from("<I", rxbuf)[0]
            if len(rxbuf) < 4 + n:
                break
            kind, payload = rxbuf[4], bytes(rxbuf[5:4 + n])
            del rxbuf[:4 + n]
            yield kind, payload, time.time()
        try:
            d = gs.recv(1 << 20)
            if not d:
                return
            rxbuf.extend(d)
        except socket.timeout:
            pass


HELLO, HOST_HELLO, START_VIDEO, STOP_VIDEO, KEYFRAME, VIDEO, VIDEO_STATUS = 1, 2, 0x10, 0x11, 0x12, 0x13, 0x14
INPUT = 0x40


def key(qnum, down):
    send(INPUT, struct.pack("<BHB", 1, qnum, 1 if down else 0))


QNUM = {**{c: q for c, q in zip("1234567890", range(0x02, 0x0c))}, "-": 0x0c, "=": 0x0d,
        **{c: q for c, q in zip("qwertyuiop", range(0x10, 0x1a))}, "[": 0x1a, "]": 0x1b, "\n": 0x1c,
        **{c: q for c, q in zip("asdfghjkl", range(0x1e, 0x27))}, ";": 0x27, "'": 0x28, "`": 0x29,
        "\\": 0x2b, **{c: q for c, q in zip("zxcvbnm", range(0x2c, 0x33))}, ",": 0x33, ".": 0x34,
        "/": 0x35, " ": 0x39}
SHIFTQ = {"_": "-", "+": "=", "{": "[", "}": "]", ":": ";", '"': "'", "~": "`", "|": "\\", "<": ",",
          ">": ".", "?": "/", "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6", "&": "7",
          "*": "8", "(": "9", ")": "0"}


def ttype(text, gap=0.02):
    """Type through the guest tools (Msg::Input keys -> uinput)."""
    for ch in text:
        base, shift = ch, False
        if ch in SHIFTQ:
            base, shift = SHIFTQ[ch], True
        elif ch.isupper():
            base, shift = ch.lower(), True
        q = QNUM[base]
        if shift:
            key(0x2a, True)
        key(q, True)
        key(q, False)
        if shift:
            key(0x2a, False)
        time.sleep(gap)


hello = None
for attempt in range(40):
    send_json(HOST_HELLO, {"v": 1, "clipboard_max_bytes": 1 << 20})
    for kind, p, _ in recv_msgs(1.0):
        if kind == HELLO:
            hello = json.loads(p)
            break
    if hello:
        break
if not hello:
    fail("hello", "no Hello from the guest tools; serial: " + serial_text()[-600:])
    cleanup()
    sys.exit(1)
caps = hello.get("caps", {})
results["hello_raw"] = hello
if hello.get("os") == "linux" and "kmsgrab" in caps.get("capture", []) and "h264" in caps.get("codecs", []):
    ok("hello", f"{hello.get('os_version')} agent {hello.get('agent_version')}: capture {caps.get('capture')}, "
       f"encoders {caps.get('encoders')}, gamepad {caps.get('gamepad')}, relative {caps.get('relative_mouse')}")
else:
    fail("hello", json.dumps(hello))
if not caps.get("clipboard") and not caps.get("resize"):
    ok("honesty", "no clipboard/resize claimed without a graphical session")
else:
    fail("honesty", f"clipboard={caps.get('clipboard')} resize={caps.get('resize')} with no X/Wayland session")

# ---- video ----
send_json(START_VIDEO, {"codec": "h264", "fps": 60, "bitrate_kbps": 2000, "max_bitrate_kbps": 2000,
                        "keyframe_interval_frames": 600, "low_delay": True, "fixed_rate": True, "chroma": "420"})
status, packets = None, []
sv_sent = time.time()
kf_req_at, kf_latency = None, None
t0 = time.time()
for kind, p, t in recv_msgs(8.0):
    if kind == VIDEO_STATUS:
        status = json.loads(p)
    elif kind == VIDEO:
        keyframe = p[0] & 1
        cap_us, enc_us, w, h = struct.unpack_from("<QIHH", p, 4)
        packets.append({"t": t, "key": keyframe, "enc_us": enc_us, "w": w, "h": h, "size": len(p) - 20,
                        "data": p[20:]})
        if kf_req_at and keyframe and kf_latency is None and t > kf_req_at:
            kf_latency = (t - kf_req_at) * 1000
    if status is None and not packets and time.time() - sv_sent > 3:
        # the tools may have been between two connections: ask again
        send_json(HOST_HELLO, {"v": 1, "clipboard_max_bytes": 1 << 20})
        send_json(START_VIDEO, {"codec": "h264", "fps": 60, "bitrate_kbps": 2000, "max_bitrate_kbps": 2000,
                                "keyframe_interval_frames": 600, "low_delay": True, "fixed_rate": True, "chroma": "420"})
        sv_sent = time.time()
    if kf_req_at is None and time.time() - t0 > 3 and len(packets) > 10:
        kf_req_at = time.time()
        send(KEYFRAME)
h264 = os.path.join(W, "guest.h264")
with open(h264, "wb") as f:
    for pk in packets:
        f.write(pk["data"])
if status and status.get("running") and packets:
    probe = subprocess.run(["ffprobe", "-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries",
                            "stream=codec_name,width,height,nb_read_frames", "-of", "json", h264],
                           capture_output=True, text=True)
    try:
        st = json.loads(probe.stdout)["streams"][0]
    except Exception:
        st = {}
    enc = sorted(pk["enc_us"] / 1000 for pk in packets)
    results["video_stats"] = {"packets": len(packets), "ffprobe": st, "status": status,
                              "encode_ms_median": enc[len(enc) // 2], "encode_ms_p95": enc[int(len(enc) * 0.95)],
                              "keyframe_request_ms": kf_latency, "first_is_key": bool(packets[0]["key"])}
    if (st.get("codec_name") == "h264" and int(st.get("nb_read_frames", 0)) >= len(packets) - 2
            and packets[0]["key"] and st.get("width") == packets[0]["w"]):
        ok("video", f"{len(packets)} packets from {status.get('capture')}+{status.get('encoder')}, ffprobe h264 "
           f"{st.get('width')}x{st.get('height')} {st.get('nb_read_frames')} frames, agent-reported encode_us median "
           f"{enc[len(enc) // 2]:.1f} ms p95 {enc[int(len(enc) * 0.95)]:.1f} ms")
    else:
        fail("video", f"ffprobe {st} first_key={packets[0]['key']} size={packets[0]['w']}x{packets[0]['h']}")
    if kf_latency is not None and kf_latency < 2000:
        ok("keyframe", f"IDR {kf_latency:.0f} ms after the request")
    else:
        fail("keyframe", f"no keyframe after the request ({kf_latency})")
else:
    fail("video", f"status={status} packets={len(packets)}; serial: {serial_text()[-400:]}")

# ---- keyboard through the tools (log in on the VGA console, tty1) ----
ttype("root\n")
time.sleep(3)
ttype("echo HXKEY$((6*7)) >/dev/ttyS0\n")
if wait_serial("HXKEY42", 10):
    ok("keyboard", "shell ran a command typed through the guest tools (HXKEY42 on the serial port)")
else:
    fail("keyboard", serial_text()[-300:])

# ---- latency: keystroke through the tools -> first changed encoded frame ----
lat = []
# Stimulus: a key that visibly changes the whole screen, the way a game
# reacts to input. inv.sh runs on tty1 and switches the console to reverse
# video on "x", back on "y", and ends on "q". Written over the serial
# console (a heredoc), started through the tools' keyboard.
INV = """stty -icanon -echo
while c=$(dd bs=1 count=1 2>/dev/null); do
case "$c" in x) printf '\033[?5h';; y) printf '\033[?5l';; q) break;; esac
done
printf '\033[?5l'; stty sane
"""
stype("echo 0 > /sys/class/graphics/fbcon/cursor_blink\n")
stype("cat > /tmp/inv.sh <<'HXEOF'\n" + INV.replace("\033", "\\033") + "HXEOF\n")
stype("sha1sum /tmp/inv.sh\n")
ttype("clear\n")
time.sleep(0.5)
ttype("sh /tmp/inv.sh\n")
time.sleep(1)
send_json(START_VIDEO, {"codec": "h264", "fps": 60, "bitrate_kbps": 2000, "max_bitrate_kbps": 2000,
                        "keyframe_interval_frames": 600, "low_delay": True, "fixed_rate": True, "chroma": "420"})
# baseline from THIS stream (the restart sends an IDR and a few large
# frames first): 2 s of idle P-frames after 1.5 s of settling
for _ in recv_msgs(1.5):
    pass
idle = sorted(len(p) - 20 for kind, p, t in recv_msgs(2.0) if kind == VIDEO and not p[0] & 1)
if len(idle) < 30:
    fail("latency", f"only {len(idle)} idle frames in 2 s (fixed-rate 60 fps expected)")
else:
    base = idle[len(idle) // 2]
    thr = max(3 * base, base + 200)
    # negative control: with no input, no frame may cross the threshold
    false_hits = sum(1 for kind, p, t in recv_msgs(2.0) if kind == VIDEO and not p[0] & 1 and len(p) - 20 > thr)
    for i in range(40):
        for _ in recv_msgs(0.3):
            pass
        sent = time.time()
        ttype("x" if i % 2 == 0 else "y", gap=0)
        for kind, p, t in recv_msgs(1.0):
            if kind == VIDEO and not p[0] & 1 and len(p) - 20 > thr:
                lat.append((t - sent) * 1000)
                break
    send(STOP_VIDEO)
    ttype("y")
    ttype("q")
    time.sleep(0.5)
    if false_hits:
        fail("latency", f"change detector fires without input ({false_hits} frames over {thr} bytes in 2 s idle)")
    elif len(lat) >= 20:
        lat.sort()
        results["latency_ms"] = {"samples": len(lat), "median": lat[len(lat) // 2], "min": lat[0],
                                 "p95": lat[int(len(lat) * 0.95) - 1], "idle_p_frame_bytes": base,
                                 "threshold_bytes": thr}
        ok("latency", f"keystroke -> changed encoded frame at the node: median {lat[len(lat) // 2]:.1f} ms, "
           f"min {lat[0]:.1f} ms, p95 {lat[int(len(lat) * 0.95) - 1]:.1f} ms over {len(lat)} samples "
           f"(screen-wide change on the key; idle P-frame {base} B, threshold {thr} B, 0 false hits idle; 60 fps software x264, 2-vCPU guest)")
    else:
        fail("latency", f"only {len(lat)} of 40 keystrokes produced a changed frame (idle P-frame {base} B)")

# ---- gamepad + relative mouse ----
send(INPUT, struct.pack("<BBIhhhhBB", 6, 0, 0b1, 1000, -1000, 0, 0, 0, 255))
send(INPUT, struct.pack("<BBIhhhhBB", 6, 0, 0, 0, 0, 0, 0, 0, 0))
time.sleep(1)
ttype("echo PADS $(grep -c Horcrux /proc/bus/input/devices) $(grep -c gamepad /proc/bus/input/devices) >/dev/ttyS0\n")
ttype("od -An -tx1 -N 6 /dev/input/mice >/dev/ttyS0 &\n")
time.sleep(1.5)
for _ in range(3):
    send(INPUT, struct.pack("<Bii", 3, 7, 3))
    time.sleep(0.2)
time.sleep(1)
s = serial_text()
import re
pads = re.findall(r"PADS (\d+) (\d+)", s)
if pads and int(pads[-1][0]) >= 3 and int(pads[-1][1]) >= 1:
    ok("gamepad", f"uinput devices: {pads[-1][0]} Horcrux devices, {pads[-1][1]} gamepad (keyboard+mouse, pointer, gamepad)")
else:
    fail("gamepad", f"{pads}")
# a PS/2-format packet with dx=7 (0x07) and dy=-3 (0xfd) from /dev/input/mice
if " 07 fd" in s or "07 fd" in s.replace("\n", " "):
    ok("relative-mouse", "raw relative motion reached /dev/input/mice")
else:
    fail("relative-mouse", s[-200:])

with open(os.path.join(OUT, "linux-guest-e2e.json"), "w") as f:
    json.dump(results, f, indent=1, default=str)
cleanup()
print(f"DONE fails={fails}")
sys.exit(1 if fails else 0)
