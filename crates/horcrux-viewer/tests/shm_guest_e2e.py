#!/usr/bin/env python3
"""Real-VM test of the same-host raw frame path (Horcrux frame ring).

The VM gets exactly the device Horcrux gives console VMs when
`[console] shm_frames` is on (`memory-backend-file share=on` over a file
whose header the node wrote, `ivshmem-plain`; display::qemu_args), plus the
guest-tools port and the tools CD. Alpine's virt ISO boots, FFmpeg comes
from the Alpine mirror, the guest tools start from the CD and find the ring
on the ivshmem BAR. The native viewer's headless reader
(`horcrux-viewer measure`) then attaches to the same file, the way it maps
the descriptor the node hands over.

Checks:
  idle       no reader -> the guest tools capture nothing (ring sequence
             does not move, writer state idle)
  frames     with a reader: raw frames at the asked rate, guest mode size,
             frame-to-viewer time (writer capture -> publish on the guest's
             clock + publish -> seen on the host clock + copy)
  latency    keystroke (QMP input, as the node's D-Bus input path ends in
             QEMU's input layer) -> first frame in the viewer showing the
             guest's screen-wide reaction; negative control without input
  stop       the reader leaves -> the guest tools stop capturing

Usage: shm_guest_e2e.py AGENT VIEWER ALPINE_VIRT_ISO OUTDIR
Prints `ok - ...` / `FAIL ...` lines and writes OUTDIR/shm-guest-e2e.json.
"""
import json
import mmap
import os
import shutil
import socket
import struct
import subprocess
import sys
import threading
import time

AGENT, VIEWER, ISO, OUT = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
MIRROR = os.environ.get("HXGT_ALPINE_MIRROR", "http://dl-cdn.alpinelinux.org/alpine/latest-stable")
FPS = int(os.environ.get("HXSHM_FPS", "60"))
# std = bochs VGA (framebuffer in the VGA BAR), virtio = virtio-vga (guest RAM)
VGA = {"std": ["-vga", "std"], "virtio": ["-vga", "none", "-device", "virtio-vga"]}[os.environ.get("HXSHM_VGA", "std")]
W = os.path.join(OUT, "shm")
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


shutil.copy(AGENT, os.path.join(W, "cd", "horcrux-guest-agent"))
os.chmod(os.path.join(W, "cd", "horcrux-guest-agent"), 0o755)
subprocess.run(["xorriso", "-as", "mkisofs", "-quiet", "-V", "HXTOOLS", "-J", "-R", "-o",
                os.path.join(W, "tools.iso"), os.path.join(W, "cd")], check=True)

# the frame ring file, header written like the node does at VM start
# under /dev/shm like the node's default (`[console] shm_frames_dir`): a
# ring on a disk-backed file pays page-cache writeback on every frame
RING = os.path.join(os.environ.get("HXSHM_DIR", "/dev/shm"), f"horcrux-frames-test-{os.getpid()}")
subprocess.run([VIEWER, "ring-init", RING, "64", "3", str(FPS)], check=True)

qmp_path, gt_path, serial = (os.path.join(W, n) for n in ("qmp.sock", "hxgt.sock", "serial.log"))
open(serial, "w").close()
qemu = subprocess.Popen([
    "qemu-system-x86_64", "-machine", "q35,accel=kvm:tcg", "-cpu", "host", "-m", "1024", "-smp", "2",
    "-drive", f"file={ISO},media=cdrom,readonly=on,if=none,id=cd0", "-device", "ide-cd,drive=cd0,bus=ide.0,bootindex=1",
    "-drive", f"file={os.path.join(W, 'tools.iso')},media=cdrom,readonly=on,if=none,id=cd1", "-device", "ide-cd,drive=cd1,bus=ide.1",
    *VGA, "-display", "none",
    "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0",
    "-device", "virtio-serial-pci,id=hxgt-vs",
    "-chardev", f"socket,id=hxgt0,path={gt_path},server=on,wait=off",
    "-device", "virtserialport,bus=hxgt-vs.0,chardev=hxgt0,name=org.horcrux.console.0",
    # the Horcrux frame ring, as display::qemu_args builds it
    "-object", f"memory-backend-file,id=hxfbmem,size=64M,mem-path={RING},share=on",
    "-device", "ivshmem-plain,memdev=hxfbmem,id=hxframes",
    "-chardev", f"socket,id=s0,path={serial}.sock,server=on,wait=off,logfile={serial}", "-serial", "chardev:s0",
    "-qmp", f"unix:{qmp_path},server=on,wait=off",
], stdout=subprocess.DEVNULL, stderr=open(os.path.join(W, "qemu.err"), "w"))

for _ in range(100):
    if os.path.exists(qmp_path) or qemu.poll() is not None:
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

ser = None


def serial_reader():
    while True:
        try:
            if not ser.recv(4096):
                return
        except OSError:
            return


def stype(text):
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


QCODE = {" ": "spc", "\n": "ret", "/": "slash", ".": "dot", "-": "minus"}


def qkey(ch):
    qmp("send-key", keys=[{"type": "qcode", "data": QCODE.get(ch, ch)}])


def qtype(text):
    for ch in text:
        qkey(ch)
        time.sleep(0.03)


# ---- the ring header, read the way any reader would ----
rf = open(RING, "r+b")
mm = mmap.mmap(rf.fileno(), 0)


def latest_seq():
    return struct.unpack_from("<Q", mm, 64)[0] >> 8


def writer_state():
    st = struct.unpack_from("<I", mm, 96)[0]
    info = bytes(mm[128:192]).split(b"\0")[0].decode(errors="replace")
    return st, info


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
stype("ip link set eth0 up; udhcpc -i eth0 -q -n -t 10 2>&1 | tail -1; echo HX_$((40+2))_NET $?\n")
if not wait_serial("HX_42_NET", 90):
    fail("setup", "no network: " + serial_text()[-500:])
stype(f"echo {MIRROR}/main >/etc/apk/repositories; echo {MIRROR}/community >>/etc/apk/repositories\n")
stype("apk add -q ffmpeg >/dev/ttyS0 2>&1; echo HX_$((40+3))_APK $?\n")
if not wait_serial("HX_43_APK", 300):
    fail("setup", "apk add ffmpeg did not finish: " + serial_text()[-400:])
stype("echo 0 > /sys/class/graphics/fbcon/cursor_blink\n")
stype("lspci -n | grep -c 1af4:1110; echo HX_$((40+4))_IVSHMEM\n")
stype("mkdir -p /mnt/t; mount -o ro $(findfs LABEL=HXTOOLS) /mnt/t; "
      "/mnt/t/horcrux-guest-agent --shm-poll-ms 20 >/dev/ttyS0 2>&1 &\n")
stype("sleep 3; /mnt/t/horcrux-guest-agent --print-caps | grep shm_frames; echo HX_$((40+5))_CAPS\n")
if wait_serial("HX_45_CAPS", 30) and '"shm_frames": true' in serial_text():
    ok("caps", "guest tools report shm_frames (found the ring on the ivshmem BAR)")
else:
    fail("caps", serial_text()[-600:])

# stimulus on tty1: reverse video on "x", normal on "y" (screen-wide change)
INV = """stty -icanon -echo
while c=$(dd bs=1 count=1 2>/dev/null); do
case "$c" in x) printf '\\033[?5h';; y) printf '\\033[?5l';; q) break;; esac
done
printf '\\033[?5l'; stty sane
"""
stype("cat > /tmp/inv.sh <<'HXEOF'\n" + INV + "HXEOF\n")
# its own VT (no getty there), shown on screen; keystrokes from QEMU's
# keyboard reach the foreground VT
stype("openvt -c 8 -s -- sh -c 'sh /tmp/inv.sh </dev/tty8 >/dev/tty8 2>&1'; sleep 1; fgconsole; echo HX_$((40+6))_VT\n")
wait_serial("HX_46_VT", 20)
time.sleep(1)

# ---- idle: no reader, no capture ----
s0 = latest_seq()
time.sleep(3)
s1 = latest_seq()
st, info = writer_state()
if s1 == s0 and st == 0:
    ok("idle", f"no reader: ring sequence stayed at {s0}, writer state idle ({info!r})")
else:
    fail("idle", f"sequence {s0}->{s1}, writer state {st} {info!r}")


def measure(secs, jsonl):
    p = subprocess.run([VIEWER, "measure", "--ring", RING, "--secs", str(secs), "--fps", str(FPS),
                        "--poll-us", "200", "--json", jsonl], capture_output=True, text=True)
    try:
        return json.loads(p.stdout.strip().splitlines()[-1])
    except Exception:
        return {"error": p.stderr[-400:]}


# ---- frames + frame-to-viewer ----
summary = measure(8, os.path.join(W, "frames.jsonl"))
results["frames_summary"] = summary
n = summary.get("frames", 0)
st, info = writer_state()
if n >= FPS * 3 and summary.get("size") and summary["frame_to_viewer_ms"]["median"] > 0:
    ok("frames", f"{n} raw frames in 8 s ({summary['fps']:.1f} fps, includes the capture start-up) at "
       f"{summary['size']} from the guest tools ({summary.get('writer')}); frame-to-viewer median "
       f"{summary['frame_to_viewer_ms']['median']:.2f} ms p95 {summary['frame_to_viewer_ms']['p95']:.2f} ms "
       f"(capture->publish {summary['capture_to_publish_ms']['median']:.2f} ms, publish->seen "
       f"{summary['publish_to_seen_ms']['median']:.2f} ms, copy {summary['copy_ms']['median']:.2f} ms)")
else:
    fail("frames", f"{summary}; writer {st} {info!r}; serial {serial_text()[-300:]}")

# ---- keystroke -> picture in the viewer ----
jl = os.path.join(W, "lat.jsonl")
pm = subprocess.Popen([VIEWER, "measure", "--ring", RING, "--secs", "30", "--fps", str(FPS),
                       "--poll-us", "200", "--json", jl], stdout=subprocess.PIPE, text=True)
time.sleep(4)  # capture restarts with the new reader
quiet_from = time.monotonic_ns()
time.sleep(3)  # negative control window: no input
quiet_to = time.monotonic_ns()
sends = []
for i in range(30):
    time.sleep(0.35)
    t = time.monotonic_ns()
    qkey("x" if i % 2 == 0 else "y")
    sends.append(t)
pm.wait(timeout=60)
frames = [json.loads(line) for line in open(jl) if line.strip()]
if frames:
    full = frames[-1]["width"] * frames[-1]["height"]
    # first visible reaction: more than 3 pixels moved by more than 64
    # levels (the C1 definition); full reaction: a quarter of the screen
    # (the console redraws the reverse video in pieces)
    false_hits = sum(1 for f in frames if quiet_from < f["seen_ns"] < quiet_to and f["changed"] > 3)

    def lat_for(thr):
        out = []
        for t in sends:
            hit = next((f for f in frames if f["seen_ns"] > t and f["changed"] > thr), None)
            if hit and (hit["seen_ns"] - t) < 1e9:
                out.append((hit["seen_ns"] - t) / 1e6)
        return sorted(out)

    first, whole = lat_for(3), lat_for(full // 4)

    def stats(v):
        return {"samples": len(v), "median": v[len(v) // 2], "min": v[0], "p95": v[max(0, int(len(v) * 0.95) - 1)]}

    if false_hits:
        fail("latency", f"{false_hits} frames changed without input")
    elif len(first) >= 25 and len(whole) >= 20:
        results["latency_first_change_ms"] = stats(first)
        results["latency_full_reaction_ms"] = stats(whole)
        f1, w1 = stats(first), stats(whole)
        ok("latency", f"keystroke (QEMU keyboard) -> raw frame in the viewer: first change median {f1['median']:.1f} ms "
           f"(min {f1['min']:.1f}, p95 {f1['p95']:.1f}), whole-screen reaction median {w1['median']:.1f} ms "
           f"(p95 {w1['p95']:.1f}) over {len(first)} keystrokes ({FPS} fps capture, 0 changed frames in 3 s idle)")
    else:
        fail("latency", f"only {len(first)}/{len(whole)} of {len(sends)} keystrokes produced a changed frame")
else:
    fail("latency", "no frames")
stype("chvt 1\n")

# ---- the reader left: capture stops ----
time.sleep(4)
a = latest_seq()
time.sleep(2)
b = latest_seq()
st, info = writer_state()
if a == b and st == 0:
    ok("stop", f"reader gone: capture stopped (sequence {a}, writer idle)")
else:
    fail("stop", f"sequence {a}->{b}, writer {st} {info!r}")

with open(os.path.join(OUT, "shm-guest-e2e.json"), "w") as f:
    json.dump(results, f, indent=1, default=str)
qemu.kill()
mm.close()
os.unlink(RING)
print(f"DONE fails={fails}")
sys.exit(1 if fails else 0)
