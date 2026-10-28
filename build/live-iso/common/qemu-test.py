#!/usr/bin/env python3
"""QEMU test for a live/installer ISO: guided installer (serial TUI + VGA TUI), unattended install,
reboot, and security/web assertions on the INSTALLED system.

  qemu-test.py --iso X.iso --project horcrux --mode uefi --media usb --flow auto|tui|tui-vga \
               [--fs ext4|xfs|zfs] [--zfs-raid single|mirror] [--ssh-password-opt-in] --out DIR

flows
  auto     GRUB entry "Automated install" + an answer file on a volume labelled <PROJECT>-ANSWERS
           (root_password_hash + ssh key), unattended, powers off.
  tui      GRUB entry "Install (serial console)": the curses installer is driven over the serial
           line with scripted keystrokes (expect-style; a small VT100 emulator reads the screen).
  fallback "Automated install" entry WITHOUT an answer file: must refuse (no default install) and start the
           guided installer on the VGA console instead (cheap: boots, checks, no install).
  tui-vga  default GRUB entry, VGA console only: keystrokes through QMP send-key, PNG screenshots of
           every installer screen (docs/ISO-INSTALL.md) via QMP screendump.

Asserts on the installed system: password root login over SSH is refused (unless the run opts in),
the configured key is accepted, the web UI answers on the chosen IP (QEMU forwards to that guest
address), plus file system / host name / time zone / ZFS mirror state.
Python stdlib + qemu + openssh-client (+ sshpass, mke2fs) only.
Exit 0 = every step passed; evidence (serial logs, screens/, PNGs, result.json) goes to --out.
"""
import argparse, json, os, re, shutil, socket, ssl, struct, subprocess, sys, tempfile, threading, time, urllib.error, urllib.request, zlib

PROJECTS = {
    "horcrux": dict(port=8006, health=["/api/health", "/"], pwfile="/var/lib/horcrux/initial-admin-password", old_pw="horcrux",
                    seedfile="/etc/horcrux/admin-password"),
    "patronus": dict(port=8443, health=["/login"], pwfile="/var/lib/patronus/initial-admin-password", old_pw="patronus",
                     seedfile="/var/lib/patronus/initial-admin-password"),
}
PW = "Kx9!mQv2-tle-77Zp"          # >= 12 chars, mixed: passes the installer's strength rules
FWD_WEB, FWD_SSH, FWD_WAN = 18443, 12222, 18444


# ----------------------------------------------------------------------------- png / screens
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


class VT:
    """Minimal VT100 screen model: enough for ncurses output (cup, cuf/cub/cuu/cud, ed, el, scroll)."""

    def __init__(self, rows=24, cols=80):
        self.rows, self.cols = rows, cols
        self.g = [[" "] * cols for _ in range(rows)]
        self.r = self.c = 0
        self.st = 0
        self.buf = ""

    def scroll(self):
        self.g.pop(0)
        self.g.append([" "] * self.cols)

    def nl(self):
        if self.r == self.rows - 1:
            self.scroll()
        else:
            self.r += 1

    def put(self, ch):
        if self.c >= self.cols:
            self.c = 0
            self.nl()
        self.g[self.r][self.c] = ch
        self.c += 1

    def feed(self, data):
        for ch in data.decode("latin-1"):
            if self.st == 0:
                if ch == "\x1b":
                    self.st = 1
                elif ch == "\r":
                    self.c = 0
                elif ch == "\n":
                    self.nl()
                elif ch == "\b":
                    self.c = max(0, self.c - 1)
                elif ch == "\t":
                    self.c = min(self.cols - 1, (self.c // 8 + 1) * 8)
                elif ch >= " " and ch != "\x7f":
                    self.put(ch)
            elif self.st == 1:
                if ch == "[":
                    self.st, self.buf = 2, ""
                elif ch in "()#":
                    self.st = 3
                else:
                    self.st = 0
            elif self.st == 3:
                self.st = 0
            elif self.st == 2:
                if "\x40" <= ch <= "\x7e":
                    self.csi(self.buf, ch)
                    self.st = 0
                else:
                    self.buf += ch

    def csi(self, p, f):
        p = p.lstrip("?>")
        a = [int(x) if x.isdigit() else 0 for x in p.split(";")] if p else []
        n = a[0] if a and a[0] else 1
        if f in "Hf":
            self.r = min(self.rows - 1, (a[0] or 1) - 1) if a else 0
            self.c = min(self.cols - 1, ((a[1] if len(a) > 1 else 0) or 1) - 1)
        elif f == "A":
            self.r = max(0, self.r - n)
        elif f == "B":
            self.r = min(self.rows - 1, self.r + n)
        elif f == "C":
            self.c = min(self.cols - 1, self.c + n)
        elif f == "D":
            self.c = max(0, self.c - n)
        elif f == "G":
            self.c = min(self.cols - 1, n - 1)
        elif f == "d":
            self.r = min(self.rows - 1, n - 1)
        elif f == "J":
            m = a[0] if a else 0
            if m == 2:
                self.g = [[" "] * self.cols for _ in range(self.rows)]
            elif m == 0:
                for x in range(self.c, self.cols):
                    self.g[self.r][x] = " "
                for y in range(self.r + 1, self.rows):
                    self.g[y] = [" "] * self.cols
            elif m == 1:
                for x in range(0, self.c + 1):
                    self.g[self.r][x] = " "
                for y in range(0, self.r):
                    self.g[y] = [" "] * self.cols
        elif f == "K":
            m = a[0] if a else 0
            rng = range(self.c, self.cols) if m == 0 else range(0, self.c + 1) if m == 1 else range(self.cols)
            for x in rng:
                self.g[self.r][x] = " "
        elif f == "X":
            for x in range(self.c, min(self.cols, self.c + n)):
                self.g[self.r][x] = " "
        elif f == "P":
            row = self.g[self.r]
            del row[self.c:self.c + n]
            row.extend([" "] * (self.cols - len(row)))
        elif f == "@":
            row = self.g[self.r]
            row[self.c:self.c] = [" "] * n
            del row[self.cols:]

    def text(self):
        return "\n".join("".join(r).rstrip() for r in self.g)


# ----------------------------------------------------------------------------- vm
class VM:
    def __init__(self, name, args, out):
        self.name, self.out = name, out
        self.log = open(f"{out}/{name}-serial.log", "wb")
        self.vt = VT()
        self.raw = b""
        self.lock = threading.Lock()
        self.qmp_path = f"{out}/{name}.qmp"
        self.ser_path = f"{out}/{name}.serial"
        for pth in (self.qmp_path, self.ser_path):
            if os.path.exists(pth):
                os.unlink(pth)
        # The serial line is a unix socket, not `-serial stdio`: with stdio QEMU's 16550 TX stalled
        # until the next input byte whenever the guest sent a burst (curses repaints), freezing the TUI.
        self.p = subprocess.Popen(args + ["-qmp", f"unix:{self.qmp_path},server,nowait",
                                          "-chardev", f"socket,id=ser0,path={self.ser_path},server=on,wait=off", "-serial", "chardev:ser0",
                                          "-display", "none"],
                                  stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.sock = None
        for _ in range(100):
            try:
                s = socket.socket(socket.AF_UNIX)
                s.connect(self.ser_path)
                self.sock = s
                break
            except OSError:
                time.sleep(0.1)
        if self.sock is None:
            raise SystemExit("cannot connect to the QEMU serial socket")
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
                self.raw += b
                self.vt.feed(b)

    def send(self, s):
        try:
            self.sock.sendall(s.encode() if isinstance(s, str) else s)
        except OSError:
            pass

    def screen(self):
        with self.lock:
            return self.vt.text()

    def expect_raw(self, pat, timeout, consume=True):
        rx = re.compile(pat.encode() if isinstance(pat, str) else pat)
        end = time.time() + timeout
        while time.time() < end:
            with self.lock:
                m = rx.search(self.raw)
                if m:
                    if consume:
                        self.raw = self.raw[m.end():]
                    return m
            if self.p.poll() is not None and not self.raw:
                break
            time.sleep(0.25)
        return None

    def expect_screen(self, pat, timeout):
        rx = re.compile(pat)
        end = time.time() + timeout
        while time.time() < end:
            if rx.search(self.screen()):
                return True
            if self.p.poll() is not None:
                return bool(rx.search(self.screen()))
            time.sleep(0.2)
        return False

    def alive(self):
        return self.p.poll() is None

    def qmp(self, cmd, **kw):
        s = socket.socket(socket.AF_UNIX)
        s.connect(self.qmp_path)
        s.settimeout(15)
        f = s.makefile("rw")
        f.readline()
        f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n")
        f.flush()
        f.readline()
        f.write(json.dumps({"execute": cmd, "arguments": kw}) + "\n")
        f.flush()
        r = f.readline()
        s.close()
        return r

    def shot(self, label):
        ppm = os.path.abspath(f"{self.out}/{self.name}-{label}.ppm")
        try:
            self.qmp("screendump", filename=ppm)
            time.sleep(1)
            png_from_ppm(ppm, ppm[:-4] + ".png")
            os.unlink(ppm)
        except Exception as e:
            print("screenshot failed:", e)

    def grab(self):
        """raw PPM of the VGA screen (None on failure)"""
        ppm = os.path.abspath(f"{self.out}/{self.name}-grab.ppm")
        try:
            self.qmp("screendump", filename=ppm)
            time.sleep(0.6)
            d = open(ppm, "rb").read()
            os.unlink(ppm)
            return d
        except Exception:
            return None

    @staticmethod
    def cyan_pixels(ppm):
        """the installer's title is bright cyan: count such pixels (login/boot text is grey/white)"""
        m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", ppm or b"")
        if not m:
            return 0
        px = ppm[m.end():]
        return sum(1 for i in range(0, min(len(px), 3 * 1280 * 40), 3) if px[i] < 90 and px[i + 1] > 150 and px[i + 2] > 150)

    def snap(self, label):
        """text snapshot of the serial screen (evidence)"""
        d = f"{self.out}/screens"
        os.makedirs(d, exist_ok=True)
        open(f"{d}/{self.name}-{label}.txt", "w").write(self.screen() + "\n")


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


class Kbd:
    """Send keys the same way for serial (escape sequences) and VGA (QMP send-key)."""
    SER = {"UP": "\x1b[A", "DOWN": "\x1b[B", "RIGHT": "\x1b[C", "LEFT": "\x1b[D", "TAB": "\t", "ENTER": "\r", "ESC": "\x1b", "CLEAR": "\x15", "SPACE": " "}
    VGA = {"UP": ["up"], "DOWN": ["down"], "RIGHT": ["right"], "LEFT": ["left"], "TAB": ["tab"], "ENTER": ["ret"], "ESC": ["esc"],
           "CLEAR": ["ctrl", "u"], "SPACE": ["spc"]}

    def __init__(self, vm, vga):
        self.vm, self.vga = vm, vga

    def key(self, name, pause=0.4):
        if self.vga:
            self._qmp(self.VGA[name])
        else:
            self.vm.send(self.SER[name])
        time.sleep(pause)

    def text(self, s, pause=0.02):
        for ch in s:
            if self.vga:
                self._qmp(qcodes(ch))
                time.sleep(0.05)
            else:
                self.vm.send(ch)
                time.sleep(pause)
        time.sleep(0.3)

    def _qmp(self, keys):
        self.vm.qmp("send-key", keys=[{"type": "qcode", "data": k} for k in keys])


# ----------------------------------------------------------------------------- helpers
def find_ovmf():
    for code, vars_ in (("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
                        ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd")):
        if os.path.exists(code):
            return code, vars_
    raise SystemExit("no OVMF found")


def http(port, path, method="GET", body=None, timeout=10):
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    last = None
    for scheme in ("https", "http"):
        req = urllib.request.Request(f"{scheme}://127.0.0.1:{port}{path}", method=method,
                                     data=json.dumps(body).encode() if body is not None else None,
                                     headers={"Content-Type": "application/json"})
        try:
            r = urllib.request.urlopen(req, timeout=timeout, context=ctx)
            return scheme, r.status, r.read()
        except urllib.error.HTTPError as e:
            return scheme, e.code, e.read()
        except Exception as e:
            last = e
    return None, 0, str(last).encode()


def ssh_banner(port, timeout=6):
    """what a TCP client sees on a forwarded sshd port ('' = nothing listens behind the forward)"""
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=timeout)
        s.settimeout(timeout)
        try:
            return s.recv(64).decode(errors="replace")
        finally:
            s.close()
    except Exception:
        return ""


def ssh(port, key, cmd, extra=(), timeout=40):
    args = ["ssh", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "ConnectTimeout=10",
            "-o", "LogLevel=ERROR", "-p", str(port)] + list(extra)
    if key:
        args += ["-i", key, "-o", "IdentitiesOnly=yes"]
    args += ["root@127.0.0.1", cmd]
    try:
        r = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
        return r.returncode, r.stdout + r.stderr
    except subprocess.TimeoutExpired:
        return 255, "timeout"


def make_answer_image(path, proj, content):
    d = tempfile.mkdtemp()
    open(f"{d}/answer.toml", "w").write(content)
    subprocess.check_call(["mke2fs", "-q", "-t", "ext4", "-L", f"{proj.upper()}-ANSWERS", "-d", d, path, "8M"])
    shutil.rmtree(d)


def answer_toml(a, pubkey, pwhash):
    p = a.project
    lines = ["[global]", 'keyboard = "us"', 'timezone = "Europe/Berlin"', f'hostname = "{p}-t1.test.lan"', 'email = "ops@example.test"',
             f'root_password_hash = "{pwhash}"', f'ssh_keys = ["{pubkey}"]', f"ssh_password_auth = {'true' if a.ssh_password_opt_in else 'false'}",
             "", "[disk]", f'filesystem = "{a.fs}"']
    if a.fs == "zfs":
        lines += [f'zfs_raid = "{a.zfs_raid}"', 'disks = ["/dev/vda", "/dev/vdb"]' if a.zfs_raid == "mirror" else 'disks = ["/dev/vda"]']
    else:
        lines += ['disks = ["/dev/vda"]']
    lines += [""]
    if p == "horcrux":
        lines += ["[network.management]"]
        if a.static_ip:
            lines += ['mode = "static"', f'address = "{a.static_ip}/24"', 'gateway = "10.0.2.2"', 'dns = ["10.0.2.3"]']
        else:
            lines += ['mode = "dhcp"']
    else:
        lines += ["[network.wan]", 'mode = "dhcp"', "", "[network.lan]", f'address = "{a.lan_ip}/24"']
    return "\n".join(lines) + "\n"


class Runner:
    def __init__(self, a):
        self.a = a
        self.res = {"project": a.project, "mode": a.mode, "media": a.media, "flow": a.flow, "fs": a.fs, "steps": {}}
        self.P = PROJECTS[a.project]
        self.finished_vga = False

    def step(self, name, ok, info=""):
        self.res["steps"][name] = {"ok": bool(ok), "info": info}
        print(f"[{'PASS' if ok else 'FAIL'}] {name} {info}", flush=True)
        json.dump(self.res, open(f"{self.a.out}/result.json", "w"), indent=1)
        if not ok:
            sys.exit(1)

    def guest_ip(self, installed):
        a = self.a
        if a.project == "patronus":
            return a.lan_ip
        return (a.static_ip or "10.0.2.15") if installed else "10.0.2.15"

    def qemu_args(self, iso, installed, nvars):
        a = self.a
        kvm = os.path.exists("/dev/kvm")
        args = ["qemu-system-x86_64", "-machine", "q35,accel=kvm" if kvm else "q35", "-cpu", "host" if kvm else "max",
                "-smp", "2", "-m", str(a.mem), "-drive", f"file={self.disk},if=virtio,format=qcow2"]
        if a.fs == "zfs" and a.zfs_raid == "mirror":
            args += ["-drive", f"file={self.disk2},if=virtio,format=qcow2"]
        if a.project == "horcrux":
            ip = self.guest_ip(installed)
            args += ["-netdev", f"user,id=n0,hostfwd=tcp:127.0.0.1:{FWD_WEB}-{ip}:{self.P['port']},hostfwd=tcp:127.0.0.1:{FWD_SSH}-{ip}:22",
                     "-device", "virtio-net-pci,netdev=n0,mac=52:54:00:00:00:01"]
        else:
            # n0 = WAN (QEMU user net, DHCP), n1 = LAN (own user net; the firewall's LAN address is static)
            wan = f"hostfwd=tcp:127.0.0.1:{FWD_WAN}-10.0.2.15:{self.P['port']},hostfwd=tcp:127.0.0.1:{FWD_SSH + 1}-10.0.2.15:22"
            args += ["-netdev", f"user,id=n0,{wan}", "-device", "virtio-net-pci,netdev=n0,mac=52:54:00:00:00:01",
                     "-netdev", f"user,id=n1,net=10.77.0.0/24,host=10.77.0.2,hostfwd=tcp:127.0.0.1:{FWD_WEB}-{a.lan_ip}:{self.P['port']},hostfwd=tcp:127.0.0.1:{FWD_SSH}-{a.lan_ip}:22",
                     "-device", "virtio-net-pci,netdev=n1,mac=52:54:00:00:00:02"]
        args += ["-device", "VGA", "-no-reboot", "-device", "qemu-xhci,id=xhci"]
        if a.mode == "uefi":
            code, _ = find_ovmf()
            args += ["-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={nvars}"]
        if iso:
            if a.media == "usb":
                args += ["-drive", f"if=none,id=stick,file={iso},format=raw,readonly=on", "-device", "usb-storage,bus=xhci.0,drive=stick,bootindex=0"]
            else:
                args += ["-drive", f"if=none,id=cd,file={iso},format=raw,media=cdrom,readonly=on", "-device", "ide-cd,bus=ide.0,drive=cd,bootindex=0"]
            if a.flow == "auto":
                args += ["-drive", f"if=none,id=ans,file={self.answers},format=raw", "-device", "usb-storage,bus=xhci.0,drive=ans"]
        return args

    def run(self):
        a = self.a
        os.makedirs(a.out, exist_ok=True)
        self.disk, self.disk2, self.answers = f"{a.out}/target.qcow2", f"{a.out}/target2.qcow2", f"{a.out}/answers.img"
        nvars = f"{a.out}/OVMF_VARS.fd"
        for d in (self.disk,) + ((self.disk2,) if a.fs == "zfs" and a.zfs_raid == "mirror" else ()):
            subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", d, f"{a.disk_gb}G"])
        if a.mode == "uefi":
            shutil.copy(find_ovmf()[1], nvars)
        self.key = f"{a.out}/test_key"
        for f in (self.key, self.key + ".pub"):
            if os.path.exists(f):
                os.unlink(f)
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "qemu-test", "-f", self.key])
        self.pub = open(self.key + ".pub").read().strip()
        if a.flow == "auto":
            pwhash = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemutest1", PW]).decode().strip()
            make_answer_image(self.answers, a.project, answer_toml(a, self.pub, pwhash))
            self.step("answer-file-built", True, f"{a.project.upper()}-ANSWERS (root_password_hash + ssh key)")
        if a.flow == "fallback":
            self.fallback(nvars)
            self.res["ok"] = True
            json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
            print("ALL PASS", a.project, a.mode, a.media, a.flow)
            return
        self.phase1(nvars)
        self.phase2(nvars)
        self.res["ok"] = True
        json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
        print("ALL PASS", a.project, a.mode, a.media, a.flow, a.fs)

    # ---- phase 1: boot the ISO and install
    def phase1(self, nvars):
        a = self.a
        t0 = time.time()
        vm = VM("1-install", self.qemu_args(a.iso, False, nvars), a.out)
        kb = Kbd(vm, a.flow == "tui-vga")
        try:
            if a.flow == "tui-vga":
                time.sleep(9 if a.mode == "uefi" else 3)         # firmware, then the GRUB menu (5 s countdown)
                vm.shot("00-grub-menu")
                # no key press: the default entry (guided installer on the VGA console) must boot by itself
                self.step("grub-default-entry-boots-installer", True, "waiting for the default GRUB entry (guided installer, VGA)")
            else:
                ok = vm.expect_raw(r"safe graphics", 120)          # the LAST menu entry: the menu is fully drawn
                self.step("grub-menu-reached", ok, f"after {int(time.time()-t0)}s")
                time.sleep(2)
                vm.shot("grub-menu")
                for _ in range(1 if a.flow == "tui" else 2):    # entry 1 = serial installer, 2 = automated
                    vm.send("\x1b[B")
                    time.sleep(0.6)
                vm.send("\r")
                self.step("kernel-boot-started", vm.expect_raw(r"(Linux version|Booting|OpenRC)", 90))
            if a.flow == "auto":
                ok = vm.expect_raw(r"AUTOINSTALL-START", 420)
                self.step("live-userland-up", ok, "live system booted, answer file found")
                b = ssh_banner(FWD_SSH if a.project == "horcrux" else FWD_SSH + 1)
                self.step("live-sshd-off", not b.startswith("SSH-"), f"banner on the live system: {b!r}")
                bad = vm.expect_raw(r"(ANSWER-FILE-ERROR[^\n]*|No answer file found[^\n]*)", 3, consume=False)
                self.step("answer-file-accepted", not bad, bad.group(0).decode() if bad else "validated")
                ok = vm.expect_raw(r"AUTOINSTALL-(OK|FAILED[^\n]*)", 1700, consume=False)
                self.step("installer-finished", ok and b"AUTOINSTALL-OK" in ok.group(0), ok.group(0).decode() if ok else "timeout")
                vm.shot("auto-finished")
            elif a.flow == "tui":
                self.drive_tui(vm, kb)
            else:
                self.drive_tui_vga(vm, kb)
            t = time.time()
            while vm.alive() and time.time() - t < 180:
                time.sleep(1)
            self.step("vm-stopped-after-install", not vm.alive(), f"install phase {int(time.time()-t0)}s")
        finally:
            if vm.alive():
                vm.p.kill()

    def fallback(self, nvars):
        a = self.a
        vm = VM("1-fallback", self.qemu_args(a.iso, False, nvars), a.out)
        try:
            self.step("grub-menu-reached", vm.expect_raw(r"safe graphics", 120))
            time.sleep(2)
            for _ in range(2):                              # entry 2 = automated install
                vm.send("\x1b[B")
                time.sleep(0.4)
            vm.send("\r")
            self.step("no-answer-file-detected", vm.expect_raw(r"No answer file found[^\n]*", 480),
                      "the automated entry refuses to install without an answer file")
            self.step("falls-back-to-guided-installer", vm.expect_raw(r"INSTALLER-FALLBACK-INTERACTIVE", 60))
            end = time.time() + 150
            while time.time() < end and vm.cyan_pixels(vm.grab()) < 50:
                time.sleep(3)
            vm.shot("fallback-installer")
            self.step("guided-installer-on-vga-after-fallback", time.time() < end, "installer title visible on the VGA console")
            self.step("nothing-was-written-to-the-disk", os.stat(self.disk).st_blocks * 512 < 5_000_000, "target disk untouched")
        finally:
            if vm.alive():
                vm.p.kill()

    # keystroke script shared by the serial and the VGA run ----------------------------
    def screens(self, vm, kb, wait):
        a, p = self.a, self.a.project
        vga = a.flow == "tui-vga"
        mirror = a.fs == "zfs" and a.zfs_raid == "mirror"
        shot = (lambda n: vm.shot(n)) if vga else (lambda n: None)
        wait(r"Language, keyboard and time zone", "01-language-keyboard-timezone")
        kb.key("TAB"); kb.key("TAB")                       # -> time zone
        kb.key("ENTER", 0.8)                               # popup list
        kb.text("Berlin")
        if vga:
            time.sleep(1); vm.shot("01b-timezone-picker")
        kb.key("ENTER")                                    # picks Europe/Berlin, focus moves to the buttons
        shot("01c-language-keyboard-timezone-filled")
        kb.key("ENTER", 1)
        if p == "horcrux":
            wait(r"\] File system", "02-filesystem")
            if a.fs != "ext4":
                kb.key("RIGHT")                            # ext4 -> xfs
                if a.fs == "zfs":
                    kb.key("RIGHT"); kb.key("TAB")         # -> zfs, then the layout row
                    if mirror:
                        kb.key("RIGHT")
                    shot("02b-filesystem-zfs-mirror")
            if a.fs == "zfs":
                kb.key("TAB")
            else:
                kb.key("TAB")
            kb.key("ENTER", 1)
        wait(r"\] Target disks?", "03-target-disk")
        kb.key("SPACE")
        if mirror:
            kb.key("DOWN"); kb.key("SPACE")
        shot("03b-target-disk-selected")
        kb.key("TAB"); kb.key("ENTER", 1)
        if p == "horcrux":
            wait(r"\] Management network", "04-management-network")
            kb.key("TAB"); kb.key("CLEAR"); kb.text(f"{p}-t1.test.lan")
            kb.key("TAB")                                  # -> IP configuration
            if a.static_ip:
                kb.key("RIGHT")                            # static
                kb.key("TAB"); kb.key("CLEAR"); kb.text(f"{a.static_ip}/24")
                kb.key("TAB"); kb.key("CLEAR"); kb.text("10.0.2.2")
                kb.key("TAB"); kb.key("CLEAR"); kb.text("10.0.2.3")
            shot("04b-management-network-filled")
            kb.key("TAB"); kb.key("ENTER", 1)
        else:
            wait(r"\] Interface assignment", "04-interface-assignment")
            shot("04b-interface-assignment-filled")
            kb.key("TAB"); kb.key("TAB"); kb.key("ENTER", 1)       # defaults: first NIC = WAN, second = LAN
            wait(r"WAN / LAN addressing", "05-wan-lan-addressing")
            kb.key("CLEAR"); kb.text(f"{p}-t1.test.lan")        # host name is the first field here
            kb.key("TAB"); kb.key("TAB")                           # WAN stays DHCP; -> LAN address
            kb.key("CLEAR"); kb.text(f"{a.lan_ip}/24")
            shot("05b-wan-lan-addressing-filled")
            kb.key("TAB"); kb.key("ENTER", 1)
        wait(r"Administrator account", "06-administrator-account")
        kb.text(PW); kb.key("TAB"); kb.text(PW)
        kb.key("TAB"); kb.text("ops@example.test")
        kb.key("TAB"); kb.text(self.pub)
        kb.key("TAB")                                      # -> allow SSH password login
        if a.ssh_password_opt_in:
            kb.key("SPACE")
        shot("06b-administrator-account-filled")
        kb.key("TAB"); kb.key("ENTER", 1)
        wait(r"Summary - ready to install", "07-summary")
        kb.text("ERASE")
        shot("07b-summary-confirmed")
        kb.key("TAB"); kb.key("ENTER", 1)

    def drive_tui(self, vm, kb):
        a = self.a

        def wait(pat, label):
            ok = vm.expect_screen(pat, 60)
            vm.snap(label)
            self.step(f"tui-screen-{label}", ok, "reached" if ok else "NOT reached; screen:\n" + vm.screen())
        ok = vm.expect_screen(r"Language, keyboard and time zone", 480)
        self.step("tui-launched-on-serial", ok, "guided installer is on ttyS0" if ok else vm.screen())
        b = ssh_banner(FWD_SSH if a.project == "horcrux" else FWD_SSH + 1)
        self.step("live-sshd-off", not b.startswith("SSH-"), f"banner on the live system: {b!r}")
        self.screens(vm, kb, wait)
        ok = vm.expect_screen(r"Installing", 120)
        vm.snap("08-installing")
        self.step("tui-installing", ok)
        ok = vm.expect_screen(r"Installation complete", 1700)
        vm.snap("09-finished")
        self.step("tui-finished", ok, "finish screen reached" if ok else vm.screen())
        url = re.search(r"https://\S+", vm.screen())
        self.step("finish-screen-shows-web-url", bool(url), url.group(0) if url else "no URL")
        kb.key("ENTER", 1)                                  # < Reboot >

    def drive_tui_vga(self, vm, kb):
        st = {"h": None}

        def stable(timeout):
            """wait until the VGA screen stops changing (two identical grabs 1.5 s apart)"""
            end, prev = time.time() + timeout, None
            while time.time() < end:
                g = vm.grab()
                h = hash(g) if g else None
                if h is not None and h == prev:
                    return h
                prev = h
                time.sleep(1.5)
            return prev

        def wait(pat, label):
            if st["h"] is None:                     # first screen: wait until the installer's cyan title shows up
                end = time.time() + 400
                while time.time() < end and vm.cyan_pixels(vm.grab()) < 50:
                    time.sleep(3)
                self.step("vga-installer-started", time.time() < end, "installer title visible on the VGA console")
                st["h"] = stable(30)
            else:                                   # later screens: the picture must change, then settle
                end = time.time() + 40
                while time.time() < end:
                    g = vm.grab()
                    if g and hash(g) != st["h"]:
                        break
                    time.sleep(1)
                st["h"] = stable(20)
            vm.shot(label)
        self.screens(vm, kb, wait)
        time.sleep(20)
        vm.shot("08-installing")
        t = time.time()
        while vm.alive() and time.time() - t < 1700:
            time.sleep(20)
            if time.time() - t > 120 and self.disk_idle():
                vm.shot("09-finished")
                self.finished_vga = True
                kb.key("ENTER", 1)                          # < Reboot >
                break
        self.step("vga-installer-finished", self.finished_vga, "finish screen reached (target disk written and idle), Reboot pressed")

    def disk_idle(self):
        """finish-screen heuristic for the VGA run: >1 GB written to the target (ZFS compresses) and nothing written for 75 s"""
        try:
            st = os.stat(self.disk)
            return st.st_blocks * 512 > 1_000_000_000 and time.time() - st.st_mtime > 75
        except OSError:
            return False

    # ---- phase 2: boot the installed system and assert
    def phase2(self, nvars):
        a, P = self.a, self.P
        if a.mode == "uefi":
            shutil.copy(find_ovmf()[1], nvars)              # fresh NVRAM = first boot of the disk, firmware re-scans
        t1 = time.time()
        vm = VM("2-installed", self.qemu_args(None, True, nvars), a.out)
        serial = a.flow != "tui-vga"
        try:
            if serial:
                ok = vm.expect_raw(r"login:", 420)
                if not ok:
                    vm.shot("login-timeout")
                self.step("installed-boots-to-login", ok, f"{int(time.time()-t1)}s")
                vm.shot("login-prompt")
                self.serial_login(vm, P["old_pw"], expect_ok=False)
                self.step("serial-root-login-with-chosen-password", self.serial_login(vm, PW, expect_ok=True))
            deadline = time.time() + 300
            got = None
            while time.time() < deadline:
                sc, code, body = http(FWD_WEB, P["health"][0])
                if code == 200:
                    got = (sc, code, body[:200])
                    break
                time.sleep(5)
            self.step("web-api-health", got, f"{self.guest_ip(True)}:{P['port']} {got[0]} {P['health'][0]} -> 200" if got else "no 200 within 300s")
            sc, code, body = http(FWD_WEB, "/")
            self.step("web-ui-root", code == 200 and b"<" in body, f"{sc} / -> {code} ({len(body)} bytes)")
            self.ssh_checks()
            if a.flow == "auto":
                rc, pw = ssh(FWD_SSH, self.key, f"cat {P['pwfile']}")
                webpw = pw.strip().splitlines()[-1] if rc == 0 and pw.strip() else ""
                how = f"generated first-boot password from {P['pwfile']} (length {len(webpw)})"
            else:
                webpw = PW
                how = "the password typed in the installer"
            sc, code, resp = http(FWD_WEB, "/api/auth/login", "POST", {"username": "admin", "password": webpw})
            self.step("web-login", code == 200 and (a.project == "patronus" or b"token" in resp or b"ticket" in resp), f"{how}: POST /api/auth/login -> {code} {resp[:60]!r}")
            if a.flow != "auto":
                rc, o = ssh(FWD_SSH, self.key, f"sleep 15; for f in {P['seedfile']} {P['pwfile']}; do test -e $f && echo present:$f || echo gone:$f; done")
                self.step("first-boot-password-files-removed", "present" not in o and o.count("gone") == 2, o.strip().replace("\n", " "))
            if a.project == "patronus":
                sc, code, _ = http(FWD_WAN, "/login", timeout=6)
                self.step("patronus-wan-web-blocked", code == 0, f"WAN 10.0.2.15:8443 -> {code or 'no answer (dropped/refused)'}")
            vm.shot("installed-running")
        finally:
            if vm.alive():
                if serial:
                    vm.send("halt -p\r")
                else:
                    ssh(FWD_SSH, self.key, "poweroff", timeout=10)
                time.sleep(8)
            if vm.alive():
                vm.p.kill()

    def serial_login(self, vm, pw, expect_ok):
        for _ in range(6):
            vm.send("\r")
            if not vm.expect_raw("login:", 20):
                continue
            vm.send("root\r")
            if vm.expect_raw("Password", 15):
                vm.send(f"{pw}\r")
                if expect_ok:
                    if vm.expect_raw(r"(#|\$) ", 25):
                        return True
                else:
                    r = vm.expect_raw(r"(Login incorrect|#|\$ )", 20)
                    self.step("old-default-root-password-rejected", bool(r) and b"incorrect" in r.group(0), "Login incorrect" if r else "no answer")
                    return False
        return False

    def ssh_checks(self):
        a = self.a
        rc, out = ssh(FWD_SSH, None, "true", extra=["-o", "PreferredAuthentications=none", "-o", "PubkeyAuthentication=no", "-o", "BatchMode=yes"])
        methods = re.search(r"Permission denied \(([^)]*)\)", out)
        m = methods.group(1) if methods else out.strip()[:80]
        if a.ssh_password_opt_in:
            self.step("ssh-password-auth-offered-after-opt-in", "password" in m or "keyboard-interactive" in m, f"server offers: {m}")
        else:
            self.step("ssh-password-auth-not-offered", methods is not None and "password" not in m and "keyboard-interactive" not in m, f"server offers: {m}")
        if shutil.which("sshpass"):
            r = subprocess.run(["sshpass", "-p", PW, "ssh", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "PubkeyAuthentication=no",
                                "-o", "PreferredAuthentications=password,keyboard-interactive", "-o", "ConnectTimeout=10", "-p", str(FWD_SSH), "root@127.0.0.1", "echo PWLOGIN-OK"],
                               capture_output=True, text=True, timeout=40)
            ok = "PWLOGIN-OK" in r.stdout
            if a.ssh_password_opt_in:
                self.step("ssh-root-password-login-works-after-opt-in", ok, (r.stdout + r.stderr).strip()[-80:])
            else:
                self.step("ssh-root-password-login-refused", not ok and "Permission denied" in r.stderr, (r.stdout + r.stderr).strip()[-100:])
        else:
            print("[SKIP] sshpass not installed: the password attempt itself was not made (offered auth methods were checked)")
        rc, out = ssh(FWD_SSH, self.key, "echo KEYLOGIN-OK; sshd -T 2>/dev/null | grep -Ei '^(permitrootlogin|passwordauthentication) '; hostname; cat /etc/timezone; findmnt -no FSTYPE /; id -u")
        flat = out.strip().replace("\n", " | ")[:220]
        self.step("ssh-key-login-accepted", rc == 0 and "KEYLOGIN-OK" in out, flat)
        low = out.lower()
        # OpenSSH prints prohibit-password as either that or the old alias without-password
        rootlogin = re.search(r"permitrootlogin (\S+)", low)
        pwauth = re.search(r"passwordauthentication (\S+)", low)
        got = (rootlogin.group(1) if rootlogin else "?", pwauth.group(1) if pwauth else "?")
        ok = got == ("yes", "yes") if a.ssh_password_opt_in else (got[0] in ("prohibit-password", "without-password") and got[1] == "no")
        self.step("sshd-effective-config", ok, f"PermitRootLogin={got[0]} PasswordAuthentication={got[1]}")
        self.step("hostname-and-timezone", f"{a.project}-t1" in out and "Europe/Berlin" in out, "host name + Europe/Berlin applied")
        self.step("root-filesystem", a.fs in out.replace("\r", "").split("\n"), f"/ is {a.fs}")
        if a.fs == "zfs":
            rc, z = ssh(FWD_SSH, self.key, "zpool status rpool; zpool list -H -o name,health rpool")
            ok = "ONLINE" in z and (a.zfs_raid != "mirror" or ("mirror-0" in z and z.count("vd") >= 2))
            self.step("zfs-pool-online", ok, re.sub(r"\s+", " ", z)[:220])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--project", required=True, choices=PROJECTS)
    ap.add_argument("--mode", choices=["uefi", "bios"], default="uefi")
    ap.add_argument("--media", choices=["usb", "cdrom"], default="usb")
    ap.add_argument("--flow", choices=["auto", "tui", "tui-vga", "fallback"], default="auto")
    ap.add_argument("--fs", choices=["ext4", "xfs", "zfs"], default="ext4")
    ap.add_argument("--zfs-raid", choices=["single", "mirror"], default="mirror")
    ap.add_argument("--ssh-password-opt-in", action="store_true")
    ap.add_argument("--static-ip", default="", help="horcrux management address (empty = DHCP)")
    ap.add_argument("--lan-ip", default="10.77.0.1", help="patronus LAN address")
    ap.add_argument("--out", required=True)
    ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--disk-gb", type=int, default=24)
    Runner(ap.parse_args()).run()


if __name__ == "__main__":
    main()
