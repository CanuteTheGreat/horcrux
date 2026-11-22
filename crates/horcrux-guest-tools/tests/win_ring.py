#!/usr/bin/env python3
"""Host half of the Windows frame-ring test (guest tools shm writer + IVSHMEM driver).

The eval VM kit's vm.sh takes EXTRA_QEMU_ARGS (empty by default). Steps:

  1. win_ring.py init RING            create the ring file (header before QEMU starts)
     EXTRA_QEMU_ARGS="-object memory-backend-file,id=hxfbmem,size=128M,mem-path=RING,share=on \
       -device ivshmem-plain,memdev=hxfbmem,id=hxframes" ./vm.sh start
  2. in the guest (via the kit's www/ + up/ exchange): install the virtio-win ivshmem driver
     (pnputil /add-driver ivshmem.inf /install; the test-signed publisher prompt needs one
     click on "Install", untick "Always trust"), start
       horcrux-guest-agent.exe --shm-stats-secs 5
     in the logged-in session and run win_ring_anim.ps1 so the desktop keeps changing.
  3. win_ring.py measure RING [--secs 14]   run `horcrux-viewer measure`, dump a frame as PNG

The agent logs a missing virtio-serial port every few seconds and keeps the ring writer
running regardless; the ring does not need the port.
"""
import argparse, json, os, struct, subprocess, sys, zlib


def viewer(args):
    return [os.environ.get("HORCRUX_VIEWER", "horcrux-viewer")] + args


def ppm_to_png(src, dst):
    d = open(src, "rb").read()
    magic, w, h, mx, pix = d.split(None, 4) if d[:2] == b"P6" else (None,) * 5
    if magic != b"P6":
        raise SystemExit("not a P6 file")
    w, h = int(w), int(h)
    raw = b"".join(b"\0" + pix[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def chunk(t, b):
        c = struct.pack(">I", len(b)) + t + b
        return c + struct.pack(">I", zlib.crc32(t + b) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b"")
    open(dst, "wb").write(png)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    i = sub.add_parser("init"); i.add_argument("ring"); i.add_argument("--mib", default="128")
    i.add_argument("--slots", default="3"); i.add_argument("--fps", default="60")
    m = sub.add_parser("measure"); m.add_argument("ring"); m.add_argument("--secs", default="14")
    m.add_argument("--fps", default="60"); m.add_argument("--png", default="win-ring-frame.png")
    m.add_argument("--min-frames", type=int, default=50)
    a = ap.parse_args()
    if a.cmd == "init":
        subprocess.check_call(viewer(["ring-init", a.ring, a.mib, a.slots, a.fps]))
        return
    ppm = a.png + ".ppm"
    out = subprocess.check_output(viewer(["measure", "--ring", a.ring, "--secs", a.secs,
                                          "--fps", a.fps, "--dump", ppm]), text=True)
    r = json.loads(out.strip().splitlines()[-1])
    ppm_to_png(ppm, a.png); os.remove(ppm)
    print(json.dumps(r, indent=1))
    ok = r["frames"] >= a.min_frames and r["writer"].endswith("dxgi-dup")
    print("PASS" if ok else "FAIL: %d frames from %s" % (r["frames"], r["writer"]))
    sys.exit(0 if ok else 1)


main()
