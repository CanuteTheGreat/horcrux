#!/usr/bin/env python3
"""Boot + unattended-install + reboot + web probe test for a live/installer ISO.

Runs QEMU/KVM (OVMF for UEFI, SeaBIOS for BIOS), picks the AUTOMATIC INSTALL
GRUB entry over the serial console, waits for the installer to finish and the
VM to halt, then boots the installed virtual disk (no ISO), logs in on the
serial console, and probes the project's web UI / API.

  qemu-test.py --iso X.iso --project horcrux --mode uefi --media usb --out DIR

Needs: qemu-system-x86_64, qemu-img, OVMF (UEFI), /dev/kvm. Python stdlib only.
Exit 0 = every step passed.  Evidence (serial logs, PNG screenshots,
result.json) is written to --out.
"""
import urllib.error, argparse, json, os, re, shutil, socket, ssl, struct, subprocess, sys, threading, time, urllib.request, zlib

PROJECTS = {
    # guest web port, https?, how to obtain the admin password on the serial console, probes
    "horcrux": dict(port=8006, pwcmd="cat /var/lib/horcrux/initial-admin-password", user="admin",
                    login=("/api/auth/login", lambda u, p: {"username": u, "password": p}),
                    health=["/api/health", "/"]),
    "patronus": dict(port=8443, pwcmd="cat /var/lib/patronus/initial-admin-password", user="admin",
                     login=("/api/auth/login", lambda u, p: {"username": u, "password": p}),
                     health=["/login"]),
}


def png_from_ppm(ppm_path, png_path):
    d = open(ppm_path, "rb").read()
    m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", d)
    w, h = int(m.group(1)), int(m.group(2))
    px = d[m.end():]
    raw = b"".join(b"\x00" + px[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def ch(t, b):
        c = struct.pack(">I", len(b)) + t + b
        return c + struct.pack(">I", zlib.crc32(t + b) & 0xFFFFFFFF)
    open(png_path, "wb").write(b"\x89PNG\r\n\x1a\n" + ch(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) +
                               ch(b"IDAT", zlib.compress(raw, 6)) + ch(b"IEND", b""))


class VM:
    def __init__(self, name, args, out):
        self.name, self.out = name, out
        self.log = open(f"{out}/{name}-serial.log", "wb")
        self.buf = b""
        self.lock = threading.Lock()
        self.qmp_path = f"{out}/{name}.qmp"
        self.p = subprocess.Popen(args + ["-qmp", f"unix:{self.qmp_path},server,nowait", "-serial", "stdio", "-display", "none"],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        threading.Thread(target=self._rd, daemon=True).start()

    def _rd(self):
        while True:
            b = self.p.stdout.read1(4096)
            if not b:
                break
            self.log.write(b); self.log.flush()
            with self.lock:
                self.buf += b

    def send(self, s):
        self.p.stdin.write(s.encode() if isinstance(s, str) else s); self.p.stdin.flush()

    def expect(self, pat, timeout, consume=True):
        rx = re.compile(pat.encode() if isinstance(pat, str) else pat)
        end = time.time() + timeout
        while time.time() < end:
            with self.lock:
                m = rx.search(self.buf)
                if m:
                    if consume:
                        self.buf = self.buf[m.end():]
                    return m
            if self.p.poll() is not None and not self.buf:
                break
            time.sleep(0.25)
        return None

    def alive(self):
        return self.p.poll() is None

    def qmp(self, cmd, **kw):
        s = socket.socket(socket.AF_UNIX); s.connect(self.qmp_path); s.settimeout(10)
        f = s.makefile("rw")
        f.readline(); f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n"); f.flush(); f.readline()
        f.write(json.dumps({"execute": cmd, "arguments": kw}) + "\n"); f.flush()
        r = f.readline(); s.close(); return r

    def shot(self, label):
        ppm = os.path.abspath(f"{self.out}/{self.name}-{label}.ppm")
        try:
            self.qmp("screendump", filename=ppm)
            time.sleep(1)
            png_from_ppm(ppm, ppm[:-4] + ".png"); os.unlink(ppm)
        except Exception as e:
            print("screenshot failed:", e)


def find_ovmf(secure=False):
    for code, vars_ in (("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
                        ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd")):
        if os.path.exists(code):
            return code, vars_
    raise SystemExit("no OVMF found")


def qemu_args(a, disk, iso, nvars, port_fwd):
    args = ["qemu-system-x86_64", "-machine", "q35,accel=kvm" if os.path.exists("/dev/kvm") else "q35",
            "-cpu", "host" if os.path.exists("/dev/kvm") else "max", "-smp", "2", "-m", str(a.mem),
            "-drive", f"file={disk},if=virtio,format=qcow2",
            "-netdev", f"user,id=n0,hostfwd=tcp:127.0.0.1:{port_fwd}-:{PROJECTS[a.project]['port']}",
            "-device", "virtio-net-pci,netdev=n0", "-device", "virtio-vga" if False else "VGA", "-no-reboot",
            "-device", "qemu-xhci,id=xhci"]
    if a.mode == "uefi":
        code, _ = find_ovmf()
        args += ["-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={nvars}"]
    if iso:
        if a.media == "usb":   # the hybrid ISO written to a USB stick
            args += ["-drive", f"if=none,id=stick,file={iso},format=raw,readonly=on", "-device", "usb-storage,bus=xhci.0,drive=stick,bootindex=0"]
        else:                  # optical media (El Torito)
            args += ["-drive", f"if=none,id=cd,file={iso},format=raw,media=cdrom,readonly=on", "-device", "ide-cd,bus=ide.0,drive=cd,bootindex=0"]
    return args


def http(port, path, method="GET", body=None, token=None):
    ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
    for scheme in ("https", "http"):
        req = urllib.request.Request(f"{scheme}://127.0.0.1:{port}{path}", method=method,
                                     data=json.dumps(body).encode() if body is not None else None,
                                     headers={"Content-Type": "application/json", **({"Authorization": f"Bearer {token}"} if token else {})})
        try:
            r = urllib.request.urlopen(req, timeout=10, context=ctx)
            return scheme, r.status, r.read()
        except urllib.error.HTTPError as e:
            return scheme, e.code, e.read()
        except Exception as e:
            last = e
    return None, 0, str(last).encode()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True); ap.add_argument("--project", required=True, choices=PROJECTS)
    ap.add_argument("--mode", choices=["uefi", "bios"], default="uefi"); ap.add_argument("--media", choices=["usb", "cdrom"], default="usb")
    ap.add_argument("--out", required=True); ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--disk-gb", type=int, default=24); ap.add_argument("--fwd-port", type=int, default=18443)
    ap.add_argument("--reuse-disk", action="store_true", help="skip phase 1: boot the existing target.qcow2 in --out")
    ap.add_argument("--menu-down", type=int, default=3, help="GRUB entries to move down to reach AUTOMATIC INSTALL")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    res = {"project": a.project, "mode": a.mode, "media": a.media, "iso": os.path.basename(a.iso), "steps": {}}
    def step(name, ok, info=""):
        res["steps"][name] = {"ok": bool(ok), "info": info}
        print(f"[{'PASS' if ok else 'FAIL'}] {name} {info}", flush=True)
        json.dump(res, open(f"{a.out}/result.json", "w"), indent=1)
        if not ok:
            sys.exit(1)

    disk = f"{a.out}/target.qcow2"
    if not a.reuse_disk:
        subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", disk, f"{a.disk_gb}G"])
    nvars = f"{a.out}/OVMF_VARS.fd"
    if a.mode == "uefi":
        shutil.copy(find_ovmf()[1], nvars)

    # ---- phase 1: boot the ISO, pick AUTOMATIC INSTALL on the serial GRUB ----
    t0 = time.time()
    vm = None if a.reuse_disk else VM("1-install", qemu_args(a, disk, a.iso, nvars, a.fwd_port), a.out)
    try:
        if vm:
            ok = vm.expect(r"(GNU GRUB|live system)", 120)
            step("grub-menu-reached", ok, f"after {int(time.time()-t0)}s")
            time.sleep(1); vm.shot("grub-menu")
            for _ in range(a.menu_down):
                vm.send("\x1b[B"); time.sleep(0.4)
            vm.send("\r")
            step("kernel-boot-started", vm.expect(r"(Linux version|Booting|OpenRC)", 90))
            step("live-userland-up", vm.expect(r"AUTOINSTALL-START", 420), "live system booted, autoinstall triggered")
            vm.shot("installing")
            ok = vm.expect(r"AUTOINSTALL-(OK|FAILED[^\n]*)", 1500, consume=False)
            step("installer-finished", ok and b"AUTOINSTALL-OK" in ok.group(0), ok.group(0).decode() if ok else "timeout")
            t = time.time()
            while vm.alive() and time.time() - t < 120:
                time.sleep(1)
            step("vm-halted-after-install", not vm.alive(), f"install phase {int(time.time()-t0)}s")
    finally:
        if vm and vm.alive():
            vm.p.kill()

    # ---- phase 2: boot the installed disk alone ----
    # fresh NVRAM: phase 1 left a BootOrder pointing at the (now absent) USB stick, which
    # OVMF does not rebuild; a real machine re-scans, so this models "first boot of the disk"
    if a.mode == "uefi":
        shutil.copy(find_ovmf()[1], nvars)
    t1 = time.time()
    vm2 = VM("2-installed", qemu_args(a, disk, None, nvars, a.fwd_port), a.out)
    try:
        ok = vm2.expect(r"login:", 360)
        if not ok:
            vm2.shot("login-timeout")
        step("installed-boots-to-login", ok, f"{int(time.time()-t1)}s")
        vm2.shot("login-prompt")
        logged = False
        for _ in range(6):   # getty may restart / swallow the first keystrokes
            vm2.send("\r")
            if not vm2.expect("login:", 20):
                continue
            vm2.send("root\r")
            if vm2.expect("Password", 15):
                vm2.send(f"{a.project}\r")
                if vm2.expect(r"(#|\$) ", 25):
                    logged = True
                    break
        step("serial-root-login", logged)
        P = PROJECTS[a.project]
        # web service comes up a little after login
        deadline = time.time() + 240; got = None
        while time.time() < deadline:
            sc, code, body = http(a.fwd_port, P["health"][0])
            if code == 200:
                got = (sc, code, body[:200]); break
            time.sleep(5)
        step("web-api-health", got, f"{got[0]} {P['health'][0]} -> 200 {got[2][:80]!r}" if got else "no 200 within 240s")
        sc, code, body = http(a.fwd_port, "/")
        step("web-ui-root", code == 200 and b"<" in body, f"{sc} / -> {code} ({len(body)} bytes)")
        vm2.send('echo "PW[$(' + P["pwcmd"] + ')]"\r')
        m = vm2.expect(r"PW\[([^\]\$\r\n]+)\]", 20)
        pw = m.group(1).decode() if m else ""
        path, mk = P["login"]
        sc, code, body = http(a.fwd_port, path, "POST", mk(P["user"], pw))
        step("web-login", code == 200 and (a.project == "patronus" or b"token" in body or b"ticket" in body), f"POST {path} -> {code} {body[:60]!r} (password length {len(pw)})")
        vm2.shot("installed-running")
        vm2.send("uname -a; systemctl --version >/dev/null 2>&1; rc-status -a 2>&1 | head -40; df -h /\r"); time.sleep(3)
    finally:
        vm2.send("halt -p\r") if vm2.alive() else None
        time.sleep(8)
        if vm2.alive():
            vm2.p.kill()
    res["ok"] = True
    json.dump(res, open(f"{a.out}/result.json", "w"), indent=1)
    print("ALL PASS", a.project, a.mode, a.media)


if __name__ == "__main__":
    main()
