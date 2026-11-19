#!/bin/bash
# A VM created through the API WITHOUT a CPU type boots a Windows 11 ISO to
# Setup (real QEMU/KVM). QEMU's bare qemu64 default has no SSE4.2/POPCNT, and
# Windows 11 24H2 hangs at the boot logo on it forever; a new VM must get the
# node's default CPU model (x86-64-v2-AES: qemu64 + SSE4.2/POPCNT/AES) written
# into its config instead.
#
# Usage: vm_default_cpu_win_e2e.sh <horcrux-api binary> <dir with the Windows ISO> <iso file name>
# Env:   HXW_OUT (screenshots, default /tmp/hxw-out), HXW_MINUTES (wait for
#        Setup, default 15), HXW_MEM_MB (default 3072),
#        HXW_EXPECT_CPU (default x86-64-v2-AES)
# Needs: /dev/kvm, qemu-system-x86_64, ffmpeg, python3.
set -euo pipefail
BIN=$(readlink -f "${1:?usage: $0 <horcrux-api binary> <iso dir> <iso file>}")
ISODIR=$(readlink -f "${2:?iso dir}")
ISO=${3:?iso file name}
OUT=${HXW_OUT:-/tmp/hxw-out}
EXPECT=${HXW_EXPECT_CPU:-x86-64-v2-AES}
DATA=$(mktemp -d /tmp/hxw-data.XXXXXX)
mkdir -p "$OUT"
# a binary built elsewhere (dc2-rust) links libvirt
if ldd "$BIN" 2>/dev/null | grep -q 'libvirt.so.* not found'; then
  SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
  for i in 1 2 3; do
    $SUDO apt-get update -qq >/dev/null 2>&1 || true
    apt-cache policy libvirt0 | grep -q 'Candidate: [0-9]' && break
    sleep 10
  done
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends libvirt0 >/dev/null
fi
command -v ffmpeg >/dev/null || { echo "FAIL ffmpeg missing"; exit 1; }
[ -s "$ISODIR/$ISO" ] || { echo "FAIL no ISO at $ISODIR/$ISO"; exit 1; }
[ -w /dev/kvm ] || { echo "FAIL /dev/kvm is not usable"; exit 1; }

cleanup() {
  [ -f "$DATA/api.pid" ] && kill "$(cat "$DATA/api.pid")" 2>/dev/null || true
  pkill -x qemu-system-x86 2>/dev/null || true
  pkill -x qemu-system-x86_64 2>/dev/null || true
  sleep 1
  rm -rf "$DATA"
}
trap cleanup EXIT

env HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR="$DATA" \
  HORCRUX_DATABASE_URL="sqlite://$DATA/horcrux.db" HORCRUX_SSL_DIR="$DATA/ssl" \
  HORCRUX_VM_STORAGE="$DATA/vms" HORCRUX_IMAGE_DIR="$ISODIR" \
  ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 \
  nohup "$BIN" >> "$DATA/api.log" 2>&1 &
echo $! > "$DATA/api.pid"
for i in $(seq 1 90); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "FAIL API did not start"; tail -50 "$DATA/api.log"; exit 1; }

HXW_ISO="$ISODIR/$ISO" HXW_OUT="$OUT" HXW_DATA="$DATA" HXW_EXPECT="$EXPECT" python3 -u - <<'PY'
import json, os, socket, ssl, subprocess, sys, time, urllib.request

ISO = os.environ["HXW_ISO"]; OUT = os.environ["HXW_OUT"]; DATA = os.environ["HXW_DATA"]
EXPECT = os.environ["HXW_EXPECT"]
MINUTES = int(os.environ.get("HXW_MINUTES", "15")); MEM = int(os.environ.get("HXW_MEM_MB", "3072"))
ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE

def call(method, path, body=None, token=None, ok=(200, 201, 202)):
    req = urllib.request.Request("https://127.0.0.1:8006/api" + path, method=method,
                                 data=json.dumps(body).encode() if body is not None else None)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        r = urllib.request.urlopen(req, context=ctx, timeout=60); st, txt = r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        st, txt = e.code, e.read().decode()
    try:
        data = json.loads(txt) if txt else {}
    except ValueError:
        data = {"raw": txt}
    if ok is not None and st not in ok:
        raise SystemExit(f"FAIL {method} {path} -> {st} {txt[:300]}")
    return st, data

def check(cond, what):
    if not cond:
        raise SystemExit("FAIL " + what)
    print("ok -", what, flush=True)

_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"})
T = r.get("ticket") or r.get("token")

# The default the form would preselect, then a VM with NO cpu_type at all.
_, ho = call("GET", "/vms/host-options", token=T)
check(ho["default_cpu_model"] == EXPECT, f"host-options default_cpu_model is {ho['default_cpu_model']}")
VM = "hxw-win11"
st, vm = call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 2, "memory": MEM, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "vnc", "video": "std", "sound": "none"},
    "hardware": {"nets": [], "cdrom": ISO, "boot": ["cdrom", "disk"]},
}, token=T)
check(vm["hardware"].get("cpu_type") == EXPECT, f"created without cpu_type: stored {vm['hardware'].get('cpu_type')}")
call("POST", f"/vms/{VM}/start", token=T, ok=(200, 202))

def qemu_argv():
    for pid in os.listdir("/proc"):
        if pid.isdigit():
            try:
                argv = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")
            except OSError:
                continue
            if argv and b"qemu-system" in argv[0] and any(VM.encode() in a for a in argv):
                return [a.decode(errors="replace") for a in argv]
    return []

argv = qemu_argv()
check(argv, "QEMU is running")
i = argv.index("-cpu") if "-cpu" in argv else -1
cpu = argv[i + 1] if i >= 0 else "(none)"
# QEMU has no x86-64-v2-AES model of its own: Horcrux passes qemu64 plus the baseline's flags
check(i >= 0 and cpu.startswith("qemu64,") and "+sse4.2" in cpu and "+popcnt" in cpu, f"QEMU runs with -cpu {cpu}")
check("accel=kvm" in " ".join(argv), "KVM acceleration")

sock = None
for _ in range(100):
    for root, _d, files in os.walk(DATA):
        for f in files:
            if f.endswith(".qmp.sock") and VM in f:
                sock = os.path.join(root, f)
    if sock:
        break
    time.sleep(0.2)
check(sock, "QMP socket found")
s = socket.socket(socket.AF_UNIX); s.connect(sock); f = s.makefile("rw")
def qmp(c, **a):
    f.write(json.dumps({"execute": c, "arguments": a} if a else {"execute": c}) + "\n"); f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r or "error" in r:
            return r
f.readline(); qmp("qmp_capabilities")

def luma(ppm):
    d = open(ppm, "rb").read()
    # P6 <w> <h> 255
    parts = d.split(b"\n", 3)
    px = parts[3]
    step = max(3, (len(px) // 3 // 20000) * 3)
    vals = px[0::step]
    return sum(vals) / max(1, len(vals))

t0 = time.time(); best = 0.0; reached = None
n = 0
while time.time() - t0 < MINUTES * 60:
    time.sleep(30)
    # "Press any key to boot from CD or DVD"
    for k in ("spc",):
        qmp("send-key", keys=[{"type": "qcode", "data": k}])
    ppm = f"{OUT}/win-{n:02d}.ppm"
    qmp("screendump", filename=ppm)
    l = luma(ppm); best = max(best, l)
    subprocess.call(["ffmpeg", "-loglevel", "error", "-y", "-i", ppm, f"{OUT}/win-{n:02d}.png"])
    os.unlink(ppm)
    print(f"t={int(time.time()-t0)}s mean brightness {l:.0f}", flush=True)
    n += 1
    if l > 70 and time.time() - t0 > 60:
        reached = int(time.time() - t0)
        # let the setup window finish drawing, then a last frame
        time.sleep(20)
        qmp("screendump", filename=f"{OUT}/windows-setup.ppm")
        subprocess.call(["ffmpeg", "-loglevel", "error", "-y", "-i", f"{OUT}/windows-setup.ppm", f"{OUT}/windows-setup.png"])
        os.unlink(f"{OUT}/windows-setup.ppm")
        break
check(reached is not None, f"Windows Setup drew its window after {reached}s (brightest frame {best:.0f}; the boot logo is near-black)")
call("POST", f"/vms/{VM}/stop", token=T, ok=None)
call("DELETE", f"/vms/{VM}", token=T, ok=None)
print("PASS vm_default_cpu_win_e2e")
PY
