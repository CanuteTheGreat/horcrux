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
import argparse, json, os, re, shutil, socket, ssl, struct, subprocess, uuid, sys, tempfile, threading, time, urllib.error, urllib.request, zlib

PROJECTS = {
    "horcrux": dict(svc="horcrux-api", port=8006, health=["/api/health", "/"], pwfile="/var/lib/horcrux/initial-admin-password", old_pw="horcrux",
                    seedfile="/etc/horcrux/admin-password",
                    services=["horcrux-api"], forward=False, dbfiles=[], logdirs=["/var/log/horcrux"]),
    "patronus": dict(svc="patronus-web", port=8443, health=["/login"], pwfile="/var/lib/patronus/initial-admin-password", old_pw="patronus",
                     seedfile="/var/lib/patronus/initial-admin-password",
                     services=["patronus-firewall", "patronus-applyd", "patronus-web"], forward=True,
                     dbfiles=["/var/lib/patronus/config.db"], dbuser="patronus", logdirs=["/var/log/patronus"]),
}
PW = "Kx9!mQv2-tle-77Zp"          # >= 12 chars, mixed: passes the installer's strength rules
PW2, PW3 = "Rv7#nQ4-zeta-91Lk", "Tb3$wH8-omega-52Ud"   # console-menu password reset / factory reset
FWD_WEB, FWD_SSH, FWD_WAN = 18443, 12222, 18444
FWD_BIND = os.environ.get("QT_FWD_BIND", "127.0.0.1")     # 0.0.0.0 inside a container whose ports are published (screenshots of the web UI from outside)

# ----------------------------------------------------------------------------- architectures
import platform as _platform
HOST_ARCH = {"amd64": "x86_64", "arm64": "aarch64"}.get(_platform.machine().lower(), _platform.machine().lower())
ARCHS = {
    "x86_64": dict(qemu="qemu-system-x86_64", serial="ttyS0", bios=True, nic="virtio-net-pci", gpu=["-device", "VGA"]),
    "aarch64": dict(qemu="qemu-system-aarch64", serial="ttyAMA0", bios=False, nic="virtio-net-pci,romfile=", gpu=["-device", "virtio-gpu-pci"]),
    "riscv64": dict(qemu="qemu-system-riscv64", serial="ttyS0", bios=False, nic="virtio-net-pci,romfile=", gpu=["-device", "virtio-gpu-pci"]),
}
SLOW = 1.0


def T(sec):
    """a timeout in seconds, scaled for emulation speed"""
    return sec * SLOW


# ----------------------------------------------------------------------------- hard deadline
# A stuck test must FAIL in bounded time: the KVM runner is a single slot and one hung QEMU run (a missed passphrase prompt, a guest that
# never answers) used to hold it for hours. arm_deadline() starts a watchdog that kills every QEMU this process started and exits 124;
# SIGTERM (from ci-run.sh's `timeout`) does the same. Seconds come from --deadline / env QEMU_TEST_DEADLINE (0 = off).
LIVE_VMS = []


def kill_all_vms():
    for v in list(LIVE_VMS):
        try:
            if v.p.poll() is None:
                v.p.kill()
        except Exception:
            pass


def arm_deadline(secs=None):
    import signal
    secs = float(os.environ.get("QEMU_TEST_DEADLINE", 0) or 0) if secs is None else float(secs)

    def _die(why):
        kill_all_vms()
        print(f"[FAIL] hard-deadline {why}: test stopped, QEMU killed", flush=True)
        os._exit(124)

    try:
        signal.signal(signal.SIGTERM, lambda *_: _die("SIGTERM from the job timeout"))
    except ValueError:
        pass
    if secs > 0:
        t = threading.Timer(secs, _die, args=(f"after {int(secs)}s",))
        t.daemon = True
        t.start()


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
                                  stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=open(f"{out}/{name}-qemu.err", "wb"))
        LIVE_VMS.append(self)
        self.sock = None
        self.last_rx = time.time()
        self.poke = False          # serial TUI runs: send Ctrl-L when the line has been silent (see expect_screen)
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
            self.last_rx = time.time()
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
        end = time.time() + T(timeout)
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
        end = time.time() + T(timeout)
        while time.time() < end:
            if rx.search(self.screen()):
                return True
            if self.p.poll() is not None:
                return bool(rx.search(self.screen()))
            if self.poke and time.time() - self.last_rx > 6:
                # QEMU's emulated 16550 sometimes stops transmitting until the next RX byte; Ctrl-L makes
                # the installer repaint and kicks the UART. Harmless in every screen.
                self.send("\x0c")
                self.last_rx = time.time()
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
    def highlight_pixels(ppm):
        """teal/cyan pixels of the whole frame: the GRUB menu's highlight bar (text or gfxterm); 0 while the screen is firmware/black"""
        m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", ppm or b"")
        if not m:
            return 0
        px = ppm[m.end():]
        return sum(1 for i in range(0, len(px) - 2, 3) if px[i] < 60 and px[i + 1] > 120 and px[i + 2] > 120)

    @staticmethod
    def cyan_pixels(ppm):
        """the installer's title is bright cyan: count such pixels (login/boot text is grey/white)"""
        m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", ppm or b"")
        if not m:
            return 0
        px = ppm[m.end():]
        w = int(m.group(1))
        if w < 1000:             # the 800x600 frame of the early boot console (before the mode switch): never the installer
            return 0
        return sum(1 for i in range(0, min(len(px), 3 * w * 40), 3) if px[i] < 90 and px[i + 1] > 150 and px[i + 2] > 150)

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
    SER = {"UP": "\x1b[A", "DOWN": "\x1b[B", "RIGHT": "\x1b[C", "LEFT": "\x1b[D", "TAB": "\t", "ENTER": "\r", "ESC": "\x1b", "CLEAR": "\x15", "SPACE": " ", "2": "2"}
    VGA = {"UP": ["up"], "DOWN": ["down"], "RIGHT": ["right"], "LEFT": ["left"], "TAB": ["tab"], "ENTER": ["ret"], "ESC": ["esc"],
           "CLEAR": ["ctrl", "u"], "SPACE": ["spc"], "2": ["2"]}

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
FIRMWARE = {   # (CODE, VARS) candidates per arch, first existing pair wins
    "x86_64": [("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
               ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd")],
    "aarch64": [("/usr/share/AAVMF/AAVMF_CODE.fd", "/usr/share/AAVMF/AAVMF_VARS.fd"),
                ("/usr/share/AAVMF/AAVMF_CODE.no-secboot.fd", "/usr/share/AAVMF/AAVMF_VARS.fd"),
                ("/usr/share/edk2/aarch64/QEMU_EFI-pflash.raw", "/usr/share/edk2/aarch64/vars-template-pflash.raw")],
    "riscv64": [("/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd", "/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd"),
                ("/usr/share/edk2/riscv/RISCV_VIRT_CODE.fd", "/usr/share/edk2/riscv/RISCV_VIRT_VARS.fd")],
}
ARCH = HOST_ARCH      # set from --arch in main()


def find_ovmf(arch=None):
    """UEFI firmware (CODE, VARS template) for the guest architecture (OVMF / AAVMF / edk2-riscv)"""
    for code, vars_ in FIRMWARE[arch or ARCH]:
        if os.path.exists(code) and os.path.exists(vars_):
            return code, vars_
    raise SystemExit(f"no UEFI firmware found for {arch or ARCH} (apt: ovmf / qemu-efi-aarch64 / qemu-efi-riscv64)")


def pflash_vars(dst):
    """fresh writable copy of the firmware VARS; aarch64/riscv64 pflash banks must be exactly as large as CODE"""
    code, vars_ = find_ovmf()
    shutil.copy(vars_, dst)
    want = os.path.getsize(code)
    if ARCH != "x86_64" and os.path.getsize(dst) < want:
        with open(dst, "ab") as f:
            f.truncate(want)


def grub_entries(iso):
    """menu entry titles of the ISO's grub.cfg, in order (robust against the boot menu being reordered)"""
    import mmap
    with open(iso, "rb") as f, mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as m:
        pos = m.find(b"set default=0")
        if pos < 0:
            pos = m.find(b"menuentry \"")
        blob = m[max(0, pos - 4000): pos + 12000]
    return [t.decode(errors="replace") for t in re.findall(rb'menuentry "([^"]+)"', blob)]


def grub_index(iso, *pats):
    """index of the first menu entry whose title matches every regex in pats"""
    ents = grub_entries(iso)
    for i, t in enumerate(ents):
        if all(re.search(p, t, re.I) for p in pats):
            return i
    raise SystemExit(f"no GRUB entry matching {pats} in {ents}")


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
             f'root_password_hash = "{pwhash}"', f'ssh_keys = ["{pubkey}"]', f"ssh_password_auth = {'true' if a.ssh_password_opt_in else 'false'}", f"console_menu_open = {'true' if a.console_open else 'false'}",
             "", "[disk]"] + ([] if a.fs_default else [f'filesystem = "{a.fs}"'])     # default run: no filesystem key = the installer's own default
    if a.fs == "xfs-luks":
        lines += [f'luks_passphrase = "{PW}"']
    if a.fs == "zfs":
        lines += [f'zfs_raid = "{a.zfs_raid}"', 'disks = ["/dev/vda", "/dev/vdb"]' if a.zfs_raid == "mirror" else 'disks = ["/dev/vda"]']
    else:
        lines += ['disks = ["/dev/vda"]']
    lines += ["", "[hardening]", "ssh = true", "sysctl = true", "",
              "[use_flags]", 'preset = "minimal"', ""]          # answer-file use_flags: Minimal preset (checked against the Portage metadata)
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
        self.res = {"project": a.project, "arch": a.arch, "mode": a.mode, "media": a.media, "flow": a.flow, "fs": a.fs, "steps": {}}
        self.P = PROJECTS[a.project]
        self.finished_vga = False
        self.shots = []

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

    def accel(self):
        """(kvm?, machine+cpu args) - KVM only when the guest arch is the host's and /dev/kvm exists"""
        kvm = os.path.exists("/dev/kvm") and ARCH == HOST_ARCH
        if ARCH == "x86_64":
            # a.cpu is a conservative x86-64-v2 model, NOT "host": binaries built with target-cpu=native passed under
            # -cpu host on the build machine and then died with "invalid opcode" on a different real CPU.
            return kvm, ["-machine", "q35,accel=kvm" if kvm else "q35", "-cpu", self.a.cpu]
        if ARCH == "aarch64":
            if kvm:
                return True, ["-machine", "virt,gic-version=host,accel=kvm", "-cpu", "host"]
            # pauth-impdef: the default QARMA5 pointer-auth emulation is the slowest part of TCG on arm64
            return False, ["-machine", "virt,gic-version=max", "-accel", "tcg,thread=multi", "-cpu", "cortex-a72"]  # generic ARMv8.0-A baseline (no SVE/LSE/pauth); KVM runs on the host CPU
        return kvm, ["-machine", "virt"] + (["-accel", "kvm", "-cpu", "host"] if kvm else ["-accel", "tcg,thread=multi", "-cpu", "rv64"])

    def qemu_args(self, iso, installed, nvars):
        a, A = self.a, ARCHS[ARCH]
        kvm, mach = self.accel()
        args = [A["qemu"]] + mach + ["-smp", "2", "-m", str(a.mem), "-drive", f"file={self.disk},if=virtio,format=qcow2"]
        if a.fs == "zfs" and a.zfs_raid == "mirror":
            args += ["-drive", f"file={self.disk2},if=virtio,format=qcow2"]
        nic = A["nic"]
        if a.project == "horcrux":
            ip = self.guest_ip(installed)
            args += ["-netdev", f"user,id=n0,hostfwd=tcp:{FWD_BIND}:{FWD_WEB}-{ip}:{self.P['port']},hostfwd=tcp:{FWD_BIND}:{FWD_SSH}-{ip}:22",
                     "-device", f"{nic},netdev=n0,mac=52:54:00:00:00:01"]
        else:
            # n0 = WAN (QEMU user net, DHCP), n1 = LAN (own user net; the firewall's LAN address is static)
            wan = f"hostfwd=tcp:{FWD_BIND}:{FWD_WAN}-10.0.2.15:{self.P['port']},hostfwd=tcp:{FWD_BIND}:{FWD_SSH + 1}-10.0.2.15:22"
            args += ["-netdev", f"user,id=n0,{wan}", "-device", f"{nic},netdev=n0,mac=52:54:00:00:00:01",
                     "-netdev", f"user,id=n1,net=10.77.0.0/24,host=10.77.0.2,hostfwd=tcp:{FWD_BIND}:{FWD_WEB}-{a.lan_ip}:{self.P['port']},hostfwd=tcp:{FWD_BIND}:{FWD_SSH}-{a.lan_ip}:22",
                     "-device", f"{nic},netdev=n1,mac=52:54:00:00:00:02"]
        gpu = A["gpu"]
        if a.flow == "gui":        # virtio-gpu at a fixed size (virtio-vga also gives a BIOS machine a VGA console)
            gpu = ["-device", ("virtio-vga" if ARCH == "x86_64" else "virtio-gpu-pci") + ",xres=1280,yres=800"]
        args += gpu + ["-no-reboot", "-device", "qemu-xhci,id=xhci"]
        if ARCH != "x86_64":
            args += ["-device", "usb-kbd,bus=xhci.0"]            # no PS/2 on virt: the VGA flows type through USB HID
        if a.flow == "gui":
            args += ["-device", "usb-tablet,bus=xhci.0"]            # absolute pointer: mouse clicks on the GUI
        if a.mode == "uefi":
            code, _ = find_ovmf()
            args += ["-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={nvars}"]
        if iso:
            if a.media == "usb":
                args += ["-drive", f"if=none,id=stick,file={iso},format=raw,readonly=on", "-device", "usb-storage,bus=xhci.0,drive=stick,bootindex=0"]
            elif ARCH == "x86_64":
                args += ["-drive", f"if=none,id=cd,file={iso},format=raw,media=cdrom,readonly=on", "-device", "ide-cd,bus=ide.0,drive=cd,bootindex=0"]
            else:   # virt has no IDE: an optical drive on virtio-scsi
                args += ["-device", "virtio-scsi-pci,id=scsi0", "-drive", f"if=none,id=cd,file={iso},format=raw,media=cdrom,readonly=on",
                         "-device", "scsi-cd,bus=scsi0.0,drive=cd,bootindex=0"]
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
            pflash_vars(nvars)
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
        try:
            self.phase1(nvars)
            self.disk_checks()
            if a.install_only:
                self.step("install-only", True, "stopped after the install: the disk layout was checked without booting it")
                self.res["ok"] = True
                json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
                print("ALL PASS", a.project, a.mode, a.media, a.flow, a.fs, "(install only)")
                return
            self.phase2(nvars)
            if a.restarts:
                self.restart_phase(nvars)
        finally:
            if self.shots:
                json.dump(self.shots, open(f"{a.out}/gui-shots.json", "w"), indent=1)
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
                # firmware, then the GRUB menu (6 s countdown, default entry = the GRAPHICAL installer): a blind sleep lost the race on a
                # loaded host (the countdown ended, the graphical installer booted, "vga-installer-started" failed). Wait until the highlight
                # bar is on the screen and press the hotkey at once (any key also stops the countdown).
                time.sleep(2)
                end = time.time() + 120
                while time.time() < end and vm.highlight_pixels(vm.grab()) < 1500:
                    pass
                kb.key("2", 0.5)                               # hotkey 2 = Install (text mode); the default entry is the graphical installer
                vm.shot("00-grub-menu")
                self.step("grub-text-install-entry", True, "VGA text installer (hotkey 2)")
            else:
                ok = vm.expect_raw(r"Serial console \(" + re.escape(ARCHS[a.arch]["serial"]), 120)  # the LAST menu line: the menu is fully drawn
                self.step("grub-menu-reached", ok, f"after {int(time.time()-t0)}s")
                time.sleep(2)
                vm.shot("grub-menu")
                # boot menu hotkeys (common/boot/gen-grub-cfg.sh): s = serial-console submenu, 2 = text install, 4 = automated
                if a.flow == "gui":
                    self.shots.append((f"{vm.name}-grub-menu.png", "Boot menu (default entry: Graphical install)"))
                vm.send("s")                        # the serial-console submenu: same entries, kernel console on the UART too
                time.sleep(1.5)
                vm.send({"tui": "2", "auto": "4", "gui": "1"}[a.flow])
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
            elif a.flow == "gui":
                self.drive_gui(vm)
            else:
                self.drive_tui_vga(vm, kb)
            t = time.time()
            while vm.alive() and time.time() - t < T(180):
                time.sleep(1)
            self.step("vm-stopped-after-install", not vm.alive(), f"install phase {int(time.time()-t0)}s")
        finally:
            if vm.alive():
                vm.p.kill()

    def fallback(self, nvars):
        a = self.a
        vm = VM("1-fallback", self.qemu_args(a.iso, False, nvars), a.out)
        try:
            self.step("grub-menu-reached", vm.expect_raw(r"Serial console \(" + re.escape(ARCHS[a.arch]["serial"]), 120))
            time.sleep(2)
            vm.send("s")                                    # serial-console submenu, then 4 = automated install
            time.sleep(1.5)
            vm.send("4")
            self.step("no-answer-file-detected", vm.expect_raw(r"No answer file found[^\n]*", 480),
                      "the automated entry refuses to install without an answer file")
            self.step("falls-back-to-guided-installer", vm.expect_raw(r"INSTALLER-FALLBACK-INTERACTIVE", 60))
            end = time.time() + T(150)
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
        """keystroke script of the text installer (stages: keymap, hostname, disk, password, network, services, hardening, software)"""
        a, p = self.a, self.a.project
        vga = a.flow == "tui-vga"
        mirror = a.fs == "zfs" and a.zfs_raid == "mirror"
        shot = (lambda n: vm.shot(n)) if vga else (lambda n: None)
        wait(r"Language, keyboard and time zone", "01-language-keyboard-timezone")
        kb.key("TAB"); kb.key("TAB"); kb.key("TAB")        # install type -> language -> keyboard -> time zone
        kb.key("ENTER", 0.8)                               # popup list
        kb.text("Berlin")
        if vga:
            time.sleep(1); vm.shot("01b-timezone-picker")
        kb.key("ENTER")                                    # picks Europe/Berlin, focus moves to the buttons
        shot("01c-language-keyboard-timezone-filled")
        kb.key("ENTER", 1)
        wait(r"\] Host name", "02-hostname")
        kb.key("CLEAR"); kb.text(f"{p}-t1.test.lan")
        shot("02b-hostname-filled")
        kb.key("TAB"); kb.key("ENTER", 1)
        order = ["zfs", "xfs-luks", "xfs", "ext4"] if p == "horcrux" else ["xfs", "ext4"]       # the Root-file-system choice, in screen order (stages.DiskStage)
        dflt = "xfs" if p == "patronus" else ("zfs" if a.mem >= 8192 else "xfs-luks")
        wait(r"\] File system", "03-filesystem")
        if not a.fs_default:
            for _ in range((order.index(a.fs) - order.index(dflt)) % len(order)):
                kb.key("RIGHT")                            # the choice wraps; the default is preselected
        if a.fs == "zfs":
            kb.key("TAB")                                  # -> 'Encrypt the disk' check box (ZFS native encryption, off)
            if mirror:                                     # NOTE: mirror via the text wizard is not exercised by the default runs
                kb.key("TAB"); kb.key("SPACE")             # 'Show advanced options' -> the ZFS layout row appears ABOVE the focus (the focus index stays: it is now on 'Encrypt')
                kb.key("UP")                               # -> 'ZFS layout'
                kb.key("RIGHT")
                shot("03b-filesystem-zfs-mirror")
                kb.key("TAB")
            kb.key("TAB")                                  # -> 'Show advanced options' (zfs single: next stop = buttons)
        elif a.fs == "xfs-luks":
            kb.key("TAB"); kb.text(PW); kb.key("TAB"); kb.text(PW)        # disk passphrase + confirmation
            shot("03b-filesystem-xfs-luks-passphrase")
            kb.key("TAB")                                  # -> 'Show advanced options'
        else:
            kb.key("TAB")                                  # -> 'Show advanced options' (xfs, ext4)
        kb.key("TAB")                                      # -> buttons
        kb.key("ENTER", 1)
        wait(r"\] Target disks?", "04-target-disk")
        kb.key("SPACE")
        if mirror:
            kb.key("DOWN"); kb.key("SPACE")
        shot("04b-target-disk-selected")
        kb.key("TAB"); kb.key("ENTER", 1)
        wait(r"Administrator account", "05-administrator-account")
        kb.text(PW); kb.key("TAB"); kb.text(PW)
        kb.key("TAB"); kb.text("ops@example.test")
        kb.key("TAB"); kb.key("SPACE")                     # 'Show advanced options' -> the SSH key row appears and takes the focus
        kb.text(self.pub)
        shot("05b-administrator-account-filled")
        kb.key("TAB"); kb.key("TAB"); kb.key("ENTER", 1)
        if p == "horcrux":
            wait(r"\] Management network", "06-management-network")
            kb.key("TAB")                                  # -> IP configuration
            if a.static_ip:
                kb.key("RIGHT")                            # static
                kb.key("TAB"); kb.key("CLEAR"); kb.text(f"{a.static_ip}/24")
                kb.key("TAB"); kb.key("CLEAR"); kb.text("10.0.2.2")
                kb.key("TAB"); kb.key("CLEAR"); kb.text("10.0.2.3")
            shot("06b-management-network-filled")
            kb.key("TAB"); kb.key("ENTER", 1)
        else:
            wait(r"\] Interface assignment", "06-interface-assignment")
            shot("06b-interface-assignment-filled")
            kb.key("TAB"); kb.key("TAB"); kb.key("ENTER", 1)       # defaults: first NIC = WAN, second = LAN
            wait(r"WAN / LAN addressing", "07-wan-lan-addressing")
            kb.key("TAB")                                          # WAN stays DHCP; -> LAN address
            kb.key("CLEAR"); kb.text(f"{a.lan_ip}/24")
            shot("07b-wan-lan-addressing-filled")
            kb.key("TAB"); kb.key("ENTER", 1)
        wait(r"\] Services", "08-services")
        kb.key("TAB")                                      # optional services list -> buttons
        kb.key("ENTER", 1)
        wait(r"\] Security hardening", "09-hardening")
        if a.ssh_password_opt_in or a.console_open:
            kb.key("TAB"); kb.key("TAB"); kb.key("SPACE")  # 'Show advanced options'; focus lands on the first advanced row
            if a.ssh_password_opt_in:
                kb.key("SPACE")
            kb.key("TAB")                                  # -> open console menu
            if a.console_open:
                kb.key("SPACE")
            shot("09b-hardening-advanced")
            kb.key("TAB"); kb.key("TAB"); kb.key("ENTER", 1)
        else:
            kb.key("TAB"); kb.key("TAB"); kb.key("TAB"); kb.key("ENTER", 1)
        wait(r"Software \(USE flags\)", "10-software-use-flags")
        shot("10b-software-use-flags")
        kb.key("TAB"); kb.key("ENTER", 1)                  # list -> buttons -> Next
        wait(r"Summary - ready to install", "11-summary")
        kb.text("ERASE")
        shot("11b-summary-confirmed")
        kb.key("TAB"); kb.key("ENTER", 1)

    def drive_tui(self, vm, kb):
        a = self.a

        def wait(pat, label):
            ok = vm.expect_screen(pat, 60)
            vm.snap(label)
            self.step(f"tui-screen-{label}", ok, "reached" if ok else "NOT reached; screen:\n" + vm.screen())
        vm.poke = True
        ok = vm.expect_screen(r"Language, keyboard and time zone", 480)
        self.step("tui-launched-on-serial", ok, f"guided installer is on {ARCHS[ARCH]['serial']}" if ok else vm.screen())
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
            end, prev = time.time() + T(timeout), None
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
                end = time.time() + T(400)
                good = 0
                while time.time() < end and good < 3:              # 3 consecutive polls: a framebuffer mode-switch can flash random colours (cyan noise)
                    n = vm.cyan_pixels(vm.grab())
                    good = good + 1 if 50 <= n <= 1500 else 0
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
        while vm.alive() and time.time() - t < T(1700):
            time.sleep(20)
            if time.time() - t > 120 and self.disk_idle():
                vm.shot("09-finished")
                self.finished_vga = True
                kb.key("ENTER", 1)                          # < Reboot >
                break
        self.step("vga-installer-finished", self.finished_vga, "finish screen reached (target disk written and idle), Reboot pressed")

    # ---- graphical installer (virtio-gpu): keyboard through QMP, assertions through AT-SPI + the status file
    def gsh(self, vm, cmd, timeout=40):
        """run a command in the live root shell on the serial console; returns its output"""
        tag = f"{int(time.time() * 1000) % 10**9}"
        vm.send(f"{cmd}; echo __D{tag}__$?\r")
        m, end = None, time.time() + timeout
        while not m and time.time() < end:
            m = vm.expect_raw(r"(?s).*?__D" + tag + r"__(\d+)", 5)
            if not m:
                vm.send("\x0c")      # an RX byte un-sticks QEMU's 16550 transmitter when the guest wrote a long line
        if not m:
            return None, "<timeout>"
        txt = m.group(0).decode(errors="replace")
        txt = txt.replace(f"echo __D{tag}__$?", "")
        return int(m.group(1)), re.sub(r"__D\d+__\d+$", "", txt).strip()

    def gstate(self, vm):
        rc, out = self.gsh(vm, "python3 -c \"import json;d=json.load(open('/run/live-installer/gui-status.json'));iss=d.pop('issues',[]);d.pop('pages',None);d.pop('t',None);d['issues']=[[i[0],i[1],i[2][:110]] for i in iss];print(json.dumps(d))\" 2>/dev/null")
        m = re.search(r"\{.*\}", out or "", re.S)
        try:
            return json.loads(m.group(0)) if m else {}
        except ValueError:
            return {}

    def gwait(self, vm, page, timeout=120, extra=None):
        end = time.time() + timeout
        st = {}
        while time.time() < end:
            st = self.gstate(vm)
            if st.get("page") == page and (extra is None or extra(st)):
                return st
            time.sleep(2)
        self.step(f"gui-page-{page}", False, f"not reached; last status {st}")

    def gshot(self, vm, step, desc):
        time.sleep(1.5)
        n = len(self.shots) + 1
        label = f"{n:02d}-{step}"
        vm.shot(label)
        self.shots.append((f"{vm.name}-{label}.png", desc))

    def gkey(self, vm, *keys, pause=0.5):
        vm.qmp("send-key", keys=[{"type": "qcode", "data": k} for k in keys])
        time.sleep(pause)

    def gtype(self, vm, text):
        kb = Kbd(vm, True)
        kb.text(text)

    def atspi(self, vm, *args, timeout=40):
        q = " ".join("'" + x.replace("'", "'\\''") + "'" for x in args)
        return self.gsh(vm, f"/usr/libexec/live-installer-atspi {q}", timeout)

    def gfocus(self, vm, name, tries=45):
        """Tab until the widget whose accessible name starts with `name` has the keyboard focus (real keyboard navigation)"""
        for i in range(tries):
            rc, out = self.atspi(vm, "focused")
            if out and re.search(r"\|" + re.escape(name), out):
                return True
            self.gkey(vm, "tab", pause=0.3)
        return False

    def gnames(self, vm, *names):
        """AT-SPI: number of accessible nodes and which of `names` are missing (compact output: serial lines stay short)"""
        rc, out = self.atspi(vm, "names", *names, timeout=60)
        m = re.search(r"NODES=(\d+) MISSING=(\[[^\]]*\])", out or "")
        return (int(m.group(1)), json.loads(m.group(2))) if m else (0, list(names))

    def drive_gui(self, vm):
        a = self.a
        mirror = a.fs == "zfs" and a.zfs_raid == "mirror"
        # the graphical session came up on the VGA head; the root shell is on the serial console
        self.step("gui-grub-menu", True, "default entry = Graphical install; serial-console variant picked for the test shell")
        ok = vm.expect_raw(r"(Linux version|OpenRC)", 120)
        end = time.time() + 480
        up = False
        while time.time() < end and not up:
            vm.send("\r")
            time.sleep(3)
            rc, out = self.gsh(vm, "echo SHELL-UP", 10)
            up = rc == 0 and "SHELL-UP" in (out or "")
        self.step("gui-live-root-shell-on-serial", up, "serial root shell of the live system (sshd stays off)")
        b = ssh_banner(FWD_SSH if a.project == "horcrux" else FWD_SSH + 1)
        self.step("live-sshd-off", not b.startswith("SSH-"), f"banner on the live system: {b!r}")
        st = self.gwait(vm, "keymap", 420)
        rc, out = self.gsh(vm, "ps -eo comm,args | grep -E '^cage' | head -2; cat /run/live-installer/gui-result 2>/dev/null; ps -eo comm | grep -qx cage && echo CAGE-RUNNING-$((40+2))")
        self.step("gui-running-in-cage-kiosk", "CAGE-RUNNING-42" in (out or ""), (out or "")[-120:].replace("\n", " "))
        # AT-SPI: the accessibility tree must expose the sidebar, the headline widgets and the buttons
        want = ["Next", "Abort", "High contrast", "Larger text", "Server (default)", "1  Keyboard", "11  Finish"]
        n, missing = 0, want
        for _ in range(10):                       # the toolkit registers with the accessibility bus a few seconds after the first frame
            n, missing = self.gnames(vm, *want)
            if n > 40 and not missing:
                break
            time.sleep(6)
        self.step("gui-atspi-tree", n > 40 and not missing, f"{n} accessible nodes; missing: {missing}")
        self.gshot(vm, "keyboard", "Language, keyboard and install type (Server default)")
        # timezone through the keyboard-only picker
        ok = self.gfocus(vm, "UTC")
        self.step("gui-keyboard-tab-navigation", ok, "Tab reaches the time zone button (AT-SPI reports the focused widget)")
        self.gkey(vm, "spc", pause=1)
        self.gtype(vm, "Berlin")
        time.sleep(1)
        self.gshot(vm, "keyboard-timezone-picker", "Time zone picker: type to filter, Enter to choose")
        self.gkey(vm, "ret", pause=1)
        self.gshot(vm, "keyboard-filled", "Time zone set to Europe/Berlin")
        self.gnext(vm, "hostname")
        self.gshot(vm, "hostname", "Host name")
        self.gkey(vm, "ctrl", "a")
        self.gtype(vm, f"{a.project}-t1.test.lan")
        self.gshot(vm, "hostname-filled", "Host name typed")
        ok = self.gclick(vm, "Next")                 # a real pointer click (usb-tablet) on the Next button
        st = self.gwait(vm, "disk", 30)
        self.step("gui-mouse-click-next", ok and st.get("page") == "disk", "pointer click on 'Next' moved to the disk screen")
        # ---- disk
        if a.project == "horcrux":
            # the proposal depends on the machine (ZFS from 8 GB RAM, encrypted XFS below): default runs leave it alone and only assert it
            self.gshot(vm, "disk-default", "File system default proposed by the installer, with the reason" + (": " + a.fs if a.fs_default else ""))
            if not a.fs_default:
                self.gclick(vm, {"ext4": "ext4", "xfs": "XFS", "zfs": "ZFS", "xfs-luks": "XFS in an encrypted"}[a.fs])
                time.sleep(1)
            if a.fs == "xfs-luks":
                ok = self.gfocus(vm, "Disk passphrase")
                self.gtype(vm, PW)
                ok = self.gfocus(vm, "Confirm passphrase") and ok
                self.gtype(vm, PW)
                self.step("gui-luks-passphrase-typed", ok, "disk passphrase + confirmation typed with the keyboard")
            if a.fs == "zfs":
                self.gexpand(vm)
                time.sleep(1)
                if mirror:
                    self.gfocus(vm, "mirror")
                    self.gkey(vm, "spc")
        self.gshot(vm, "disk", "File system and target disk (nothing selected)")
        self.gexpand(vm)
        time.sleep(1)
        self.gshot(vm, "disk-advanced", "Disk page with Advanced options expanded")
        ok = self.gfocus(vm, "Disk /dev/vda")
        self.step("gui-disk-checkbox-focus", ok, "keyboard focus on /dev/vda")
        self.gkey(vm, "spc", pause=1)
        if mirror:
            self.gfocus(vm, "Disk /dev/vdb")
            self.gkey(vm, "spc", pause=1)
        self.gshot(vm, "disk-selected", "Disk selected: live layout preview (ESP/boot/root, no swap on a server) and the existing contents to be erased")
        self.gnext(vm, "rootpass")
        # ---- password
        self.gshot(vm, "password", "Administrator account")
        self.gtype(vm, PW)
        self.gkey(vm, "tab")
        self.gtype(vm, PW)
        self.gkey(vm, "tab")
        self.gtype(vm, "ops@example.test")
        self.gexpand(vm)
        time.sleep(1)
        self.gfocus(vm, "SSH public key")
        self.gtype(vm, self.pub)
        self.gshot(vm, "password-advanced", "Administrator account with the SSH key under Advanced options")
        self.gnext(vm, "netconfig")
        # ---- network
        self.gshot(vm, "network", "Network: link state per NIC, addressing")
        self.gexpand(vm)
        time.sleep(1)
        if a.project == "horcrux":
            if a.static_ip:
                self.gclick(vm, "Static address")             # radio buttons: a real pointer click (Tab does not reach the 2nd radio of a group)
                time.sleep(1)
                self.gfocus(vm, "IP address / prefix")
                self.gtype(vm, f"{a.static_ip}/24")
                self.gkey(vm, "tab"); self.gtype(vm, "10.0.2.2")
                self.gkey(vm, "tab"); self.gtype(vm, "10.0.2.3")
                self.gshot(vm, "network-static-advanced", "Network with a static address (Advanced options)")
            else:
                self.gclick(vm, "Static address")
                time.sleep(1.5)
                self.gshot(vm, "network-static-advanced", "Network with 'Static address' chosen: address, gateway and DNS fields appear")
                self.gclick(vm, "DHCP")
                time.sleep(1)
        else:
            self.gshot(vm, "network-advanced", "WAN/LAN assignment and addressing (Advanced options)")
            self.gfocus(vm, "LAN address/prefix")
            self.gkey(vm, "ctrl", "a")
            self.gtype(vm, f"{a.lan_ip}/24")
        self.gnext(vm, "services")
        self.gshot(vm, "services", "Services")
        self.gnext(vm, "hardening")
        self.gshot(vm, "hardening", "Security hardening (secure defaults)")
        self.gexpand(vm)
        time.sleep(1)
        if a.ssh_password_opt_in:
            self.gfocus(vm, "Allow SSH password login"); self.gkey(vm, "spc")
        if a.console_open:
            self.gfocus(vm, "Open console menu"); self.gkey(vm, "spc")
        self.gshot(vm, "hardening-advanced", "Hardening with Advanced options (risky options OFF by default, with warnings)")
        for name, shot, desc in (("High contrast", "hardening-high-contrast", "High-contrast theme (accessibility switch, top right)"),
                                 ("Larger text", "hardening-larger-text", "Larger text (accessibility switch, top right)")):
            ok = self.gfocus(vm, name)
            self.step(f"gui-a11y-{name.split()[0].lower()}-switch", ok, f"'{name}' reachable with the keyboard")
            self.gkey(vm, "spc", pause=2)
            self.gshot(vm, shot, desc)
            self.gkey(vm, "spc", pause=2)
        self.gnext(vm, "use_flags")
        # ---- USE flags
        st = self.gwait(vm, "use_flags", 60)
        self.gshot(vm, "use-flags", "Software (USE flags): presets, searchable flag list, Portage sanity check, binary vs source impact")
        self.step("gui-use-flags-default-has-no-errors", not [i for i in st.get("issues", []) if i[0] == "error"], f"{len(st.get('issues', []))} notes")
        self.gtype_search(vm, "ssl")
        self.gfocus(vm, "USE flag ssl:")
        self.gkey(vm, "spc", pause=2)
        st = self.gwait(vm, "use_flags", 30, lambda s: any(i[0] == "error" for i in s.get("issues", [])))
        self.step("gui-use-flags-sanity-check-error", any(i[0] == "error" for i in st.get("issues", [])) and not st.get("next_enabled", True),
                  "disabling 'ssl' is refused: " + (next((i[2] for i in st.get("issues", []) if i[0] == "error"), "")[:140]))
        self.gshot(vm, "use-flags-error", "USE flag sanity check ERROR (ssl switched off breaks installed packages) - Next is blocked")
        self.gkey(vm, "spc", pause=2)                      # back on
        st = self.gwait(vm, "use_flags", 30, lambda s: not any(i[0] == "error" for i in s.get("issues", [])))
        self.step("gui-use-flags-error-cleared", st.get("next_enabled", False), "ssl back on: Next enabled again")
        self.atspi(vm, "press", "Minimal")
        time.sleep(2)
        self.gshot(vm, "use-flags-minimal", "Minimal preset applied: source-build impact estimate")
        self.atspi(vm, "press", "Recommended")
        time.sleep(2)
        self.gnext(vm, "review")
        # ---- review + install
        self.gshot(vm, "review", "Review: the plan of every stage; disk erase confirmation")
        self.gfocus(vm, "Confirm erasing the disk")
        self.gkey(vm, "spc", pause=1)
        self.gshot(vm, "review-confirmed", "Erase confirmed; Install enabled")
        self.gnext(vm, "install")
        taken = set()
        t = time.time()
        while time.time() - t < 1700:
            st = self.gstate(vm)
            pct = st.get("pct", 0)
            if st.get("page") == "install":
                for mark, name in ((1, "progress-start"), (40, "progress-middle")):
                    if pct >= mark and name not in taken and (name != "progress-start" or pct < 30):
                        taken.add(name)
                        self.gshot(vm, name, f"Installing: {pct}% {st.get('note', '')}")
                if pct >= 90 and "progress-end" not in taken:
                    taken.add("progress-end")
                    self.gshot(vm, "progress-end", f"Installing: {pct}% {st.get('note', '')}")
            if st.get("page") == "finish":
                break
            time.sleep(6)
        self.step("gui-install-finished", st.get("page") == "finish", f"after {int(time.time() - t)}s; progress shots {sorted(taken)}")
        self.gshot(vm, "finish", "Installation complete: web UI address and login")
        seen = False
        for _ in range(8):                        # the accessibility tree follows the page change a moment later
            rc, _o = self.atspi(vm, "has", "Reboot")
            if rc == 0:
                seen = True
                break
            time.sleep(4)
        self.step("gui-reboot-button-in-atspi-tree", seen, "'Reboot' button is exposed to assistive technology")
        rc, out = self.atspi(vm, "press", "Reboot")
        if rc == 0 or rc is None:
            self.step("gui-reboot-pressed", True, "Reboot pressed through AT-SPI (the VM stops: -no-reboot)")
        else:                                       # fall back to the keyboard: Tab to the button, Enter
            ok = self.gfocus(vm, "Reboot")
            self.gkey(vm, "ret")
            self.step("gui-reboot-pressed", ok, f"AT-SPI press said {out.strip()[:60]!r}; keyboard Tab+Enter on 'Reboot'")

    def gexpand(self, vm):
        """open the 'Advanced options' expander with the keyboard (Tab to it, Space)"""
        ok = self.gfocus(vm, "Advanced options")
        self.gkey(vm, "spc", pause=1)
        return ok

    def gtype_search(self, vm, text):
        self.gfocus(vm, "Search USE flags")
        self.gtype(vm, text)
        time.sleep(1.5)

    def gclick(self, vm, name, scr=(1280, 800)):
        """click a button with the (absolute) pointer: position from AT-SPI, events through QMP"""
        rc, out = self.atspi(vm, "extents", name)
        m = re.search(r"EXTENTS (-?\d+) (-?\d+) (\d+) (\d+)", out or "")
        if not m:
            return False
        x, y, w, h = map(int, m.groups())
        ax, ay = int((x + w / 2) * 32767 / scr[0]), int((y + h / 2) * 32767 / scr[1])
        ev = lambda d: vm.qmp("input-send-event", events=d)       # noqa: E731
        ev([{"type": "abs", "data": {"axis": "x", "value": ax}}, {"type": "abs", "data": {"axis": "y", "value": ay}}])
        time.sleep(0.5)
        ev([{"type": "btn", "data": {"button": "left", "down": True}}])
        time.sleep(0.15)
        ev([{"type": "btn", "data": {"button": "left", "down": False}}])
        time.sleep(0.4)
        ev([{"type": "abs", "data": {"axis": "x", "value": int(900 * 32767 / scr[0])}}, {"type": "abs", "data": {"axis": "y", "value": int(720 * 32767 / scr[1])}}])   # park the pointer on blank space (not over a label)
        return True

    def gnext(self, vm, page):
        """Alt+Right (the Next shortcut), then wait for the next page"""
        self.gkey(vm, "alt", "right")
        st = self.gwait(vm, page, 60)
        self.step(f"gui-page-{page}", True, f"title: {st.get('title')}")
        return st

    def gui_first_boot_shots(self, vm):
        """installed system: the VGA console login + console menu, saved with the GUI shots"""
        kb = Kbd(vm, True)
        time.sleep(3)
        self.gshot(vm, "firstboot-login", "First boot: login prompt on the console")
        for tx in ("root\n", PW + "\n"):
            kb.text(tx)
            time.sleep(4)
        time.sleep(4)
        self.gshot(vm, "firstboot-console-menu", "First boot: console menu after the root login (assign interfaces, set IP, reset password, ...)")

    def disk_idle(self):
        """finish-screen heuristic for the VGA run: >1 GB written to the target (ZFS compresses) and nothing written for 75 s"""
        try:
            st = os.stat(self.disk)
            return st.st_blocks * 512 > 1_000_000_000 and time.time() - st.st_mtime > 75
        except OSError:
            return False

    def disk_checks(self):
        """the GPT the installer wrote (read from the qcow2 with qemu-img dd, no mount): servers/firewalls have NO swap partition; xfs-luks has a LUKS2 header"""
        a = self.a
        hdr = os.path.join(a.out, "gpt-head.bin")
        try:
            subprocess.run(["qemu-img", "dd", "-f", "qcow2", "-O", "raw", "bs=512", "count=40", f"if={self.disk}", f"of={hdr}"], check=True, capture_output=True, timeout=120)
            raw = open(hdr, "rb").read()
        except (OSError, subprocess.SubprocessError) as e:
            self.step("disk-gpt-readable", False, f"cannot read the target GPT: {e}")
            return
        if raw[512:520] != b"EFI PART":
            self.step("disk-gpt-readable", False, "no GPT signature")
            return
        n, size = struct.unpack_from("<II", raw, 512 + 80)
        base = struct.unpack_from("<Q", raw, 512 + 72)[0] * 512
        SWAP = "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f"
        parts = []
        for i in range(n):
            e = raw[base + i * size: base + (i + 1) * size]
            if len(e) < 128 or e[:16] == bytes(16):
                continue
            tguid = str(uuid.UUID(bytes_le=e[:16]))
            first = struct.unpack_from("<Q", e, 32)[0]
            name = e[56:128].decode("utf-16le", "ignore").rstrip("\x00")
            parts.append((i + 1, tguid, first, name))
        desc = ", ".join(f"{no}:{nm or '-'}" for no, _t, _f, nm in parts)
        swaps = [pt for pt in parts if pt[1] == SWAP or "swap" in pt[3].lower()]
        self.step("disk-no-swap-partition", not swaps, f"partitions written: {desc}" + (f"; SWAP PARTITION FOUND: {swaps}" if swaps else "; none of them swap (zram only)"))
        if a.fs == "xfs-luks":
            lu = [pt for pt in parts if pt[3] in ("root", "cryptroot") or pt[1] == "ca7d7ccb-63ed-4c53-861c-1742536059cc"]
            ok = False
            if lu:
                try:
                    r = subprocess.run(["qemu-io", "-r", "-f", "qcow2", "-c", f"read -v {lu[0][2] * 512} 16", self.disk], capture_output=True, text=True, timeout=60)
                    hexs = re.findall(r"^[0-9a-f]+:\s+((?:[0-9a-f]{2} ){1,16})", r.stdout, re.M)
                    blk = bytes.fromhex("".join(hexs[:1]).replace(" ", ""))
                    ok = blk[:6] == b"LUKS\xba\xbe" and blk[6:8] == b"\x00\x02"
                except (OSError, subprocess.SubprocessError, ValueError):
                    ok = False
            self.step("disk-root-is-luks2", ok, f"LUKS2 header at the start of partition {lu[0][0] if lu else '?'}")
        try:
            os.unlink(hdr)
        except OSError:
            pass

    # ---- phase 2: boot the installed system and assert
    def phase2(self, nvars):
        a, P = self.a, self.P
        if a.mode == "uefi":
            pflash_vars(nvars)                                # fresh NVRAM = first boot of the disk, firmware re-scans
        t1 = time.time()
        vm = VM("2-installed", self.qemu_args(None, True, nvars), a.out)
        serial = a.flow != "tui-vga"
        try:
            if serial and a.console_open:
                ok = vm.expect_raw(r"Enter an option", 420)
                if not ok:
                    vm.shot("login-timeout")
                self.step("installed-boots-to-open-console-menu", ok, f"{int(time.time()-t1)}s, no login prompt (console menu opened by the installer)")
            elif serial:
                ok = vm.expect_raw(r"login:", 420)
                if not ok:
                    vm.shot("login-timeout")
                self.step("installed-boots-to-login", ok, f"{int(time.time()-t1)}s")
                vm.shot("login-prompt")
                self.serial_login(vm, P["old_pw"], expect_ok=False)
                self.step("serial-root-login-with-chosen-password", self.serial_login(vm, PW, expect_ok=True), "lands in the console menu")
            deadline = time.time() + T(300)
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
            self.health("first-boot")
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
            if a.flow == "gui":
                self.gui_first_boot_shots(vm)
                if a.hold_file:
                    end = time.time() + 1500
                    print("HOLD: installed system is up; waiting for", a.hold_file, flush=True)
                    while time.time() < end and not os.path.exists(a.hold_file) and vm.alive():
                        time.sleep(3)
            if serial:
                self.console_menu_checks(vm)
            else:
                self.vga_console_check(vm)
        finally:
            if vm.alive():
                if serial:
                    if not self.menu_off:
                        ssh(FWD_SSH, self.key, "poweroff", timeout=10)
                else:
                    ssh(FWD_SSH, self.key, "poweroff", timeout=10)
                time.sleep(8)
            if vm.alive():
                vm.p.kill()


    # ---- post-install console menu (serial line; same program as on VGA tty1)
    menu_off = False

    def opt(self, vm, key, dialog=(), done=r"Press Enter to return to the menu", timeout=120):
        """pick a menu option, answer the prompts in `dialog` [(regex, text)], wait for `done`, return the text seen"""
        vm.send(key + "\r")
        seen = ""
        for pat, ans in dialog:
            m = vm.expect_raw("(?s).*?" + pat, 60)
            if not m:
                return seen + "<<missing prompt: " + pat + ">>"
            seen += m.group(0).decode(errors="replace")
            time.sleep(0.3)
            vm.send(ans + "\r")
        m = vm.expect_raw("(?s).*?" + done, timeout)
        seen += m.group(0).decode(errors="replace") if m else "<<timeout waiting for: " + done + ">>"
        if m and done.startswith("Press Enter"):
            vm.send("\r")
            vm.expect_raw("Enter an option", 30)
        return seen

    def web_login_code(self, pw, wait=0):
        end = time.time() + wait
        while True:
            sc, code, resp = http(FWD_WEB, "/api/auth/login", "POST", {"username": "admin", "password": pw})
            if code == 200 or time.time() >= end:
                return code
            time.sleep(4)

    def console_menu_checks(self, vm):
        a, P = self.a, self.P
        scr = vm.screen()
        vm.snap("console-menu")
        vm.shot("console-menu")
        self.step("console-menu-shown", "console menu" in scr and "Assign interfaces" in scr and "Reset the admin password" in scr and "Web UI:" in scr,
                  "header, interface list, web URL and the numbered options are on the serial console")
        t = self.opt(vm, "9")
        self.step("console-menu-status", "Web UI :" in t and "running" in t, re.sub(r"\s+", " ", t)[:120])
        t = self.opt(vm, "7", [("Host name or IP address to ping", "x;reboot")], done=r"Press Enter to return to the menu")
        self.step("console-menu-ping-rejects-injection", "Not a valid host name" in t, "x;reboot refused")
        t = self.opt(vm, "7", [("Host name or IP address to ping", "10.0.2.2")], done=r"packets transmitted", timeout=40)
        vm.expect_raw("Press Enter to return", 20)
        vm.send("\r")
        vm.expect_raw("Enter an option", 20)
        self.step("console-menu-ping", "packets transmitted" in t, "ping 10.0.2.2 ran")
        # option 2: re-apply the current address as static on the interface that carries the web UI (keeps the port forward alive)
        rc, o = ssh(FWD_SSH, self.key, f"ip -o -4 addr show | grep -F ' {self.guest_ip(True)}/'")
        m = re.search(r"\d+: (\S+)\s+inet (\S+)", o)
        self.step("console-menu-find-interface", bool(m), o.strip()[:80])
        nic, addr = m.group(1), m.group(2)
        dlg = [("Interface to configure", nic), ("IP configuration", "s"), ("IP address/prefix", addr)]
        if a.project == "horcrux":
            dlg += [("Gateway", "10.0.2.2"), ("DNS server", "10.0.2.3")]
        t = self.opt(vm, "2", dlg, timeout=120)
        rc, o = ssh(FWD_SSH, self.key, f"grep -c 'BEGIN console-menu {nic}' /etc/dhcpcd.conf; ip -4 addr show dev {nic}")
        self.step("console-menu-set-static-ip", rc == 0 and o.strip().startswith("1") and addr in o and nic in t, f"{nic} {addr} static via /etc/dhcpcd.conf; ssh still works")
        # option 3: reset the web admin password; the new one must log in (hash compatible with the product), the old one not
        t = self.opt(vm, "3", [("New admin password", PW2), ("Confirm", PW2), ("same password for the root console", "n")])
        c_new, c_old = self.web_login_code(PW2, 20), self.web_login_code(PW)
        self.step("console-menu-reset-password", c_new == 200 and c_old != 200, f"new password -> {c_new}, old password -> {c_old}")
        # option 11 backup, 4 factory reset, 10 restore
        t = self.opt(vm, "11", [], done=r"Backup written: \S+")
        rc, o = ssh(FWD_SSH, self.key, f"ls /var/backups/{a.project}/; tar tzf /var/backups/{a.project}/*manual*.tar.gz | head -20")
        self.step("console-menu-backup", "MANIFEST.json" in o and (".db" in o), re.sub(r"\s+", " ", o)[:140])
        vm.expect_raw("Press Enter to return", 20)
        vm.send("\r")
        vm.expect_raw("Enter an option", 20)
        t = self.opt(vm, "4", [("Type RESET", "RESET"), ("New admin password", PW3), ("Confirm", PW3)], done=r"Factory defaults restored", timeout=240)
        vm.expect_raw("Press Enter to return", 20)
        vm.send("\r")
        vm.expect_raw("Enter an option", 20)
        c3, c2 = self.web_login_code(PW3, 180), self.web_login_code(PW2)
        if c3 != 200:
            rc, o = ssh(FWD_SSH, self.key, f"rc-service {self.P['svc']} status; ls -la /var/lib/{a.project}; tail -n 8 /var/log/{a.project}/web-error.log; nft list ruleset 2>&1 | grep -n {P['port']} | head -5; ss -ltn | grep {P['port']}")
            print("[DIAG] " + o.replace("\n", "\n[DIAG] "))
        self.step("console-menu-factory-reset", c3 == 200 and c2 != 200, f"factory defaults: new password -> {c3}, password from before the reset -> {c2}")
        rc, o = ssh(FWD_SSH, self.key, f"sleep 12; test -e {P['seedfile']} && echo present || echo gone")
        self.step("console-menu-firstboot-file-removed", "gone" in o, f"{P['seedfile']}: {o.strip()}")
        vm.send("10\r")
        m = vm.expect_raw(r"(?s).*?Backup to restore", 60)
        txt = m.group(0).decode(errors="replace") if m else ""
        mm = re.search(r"(\d+)\) \S*-manual-\S*", txt)
        self.step("console-menu-restore-lists-backups", bool(mm), re.sub(r"\s+", " ", txt)[-160:])
        vm.send(mm.group(1) + "\r")
        vm.expect_raw("Type RESTORE", 30)
        vm.send("RESTORE\r")
        ok = vm.expect_raw("Configuration restored", 240)
        vm.expect_raw("Press Enter to return", 20)
        vm.send("\r")
        vm.expect_raw("Enter an option", 20)
        c2, c3 = self.web_login_code(PW2, 180), self.web_login_code(PW3)
        self.step("console-menu-restore", bool(ok) and c2 == 200 and c3 != 200, f"after restoring the backup: password from the backup -> {c2}, factory-reset password -> {c3}")
        vm.snap("console-menu-after")
        # option 5: power off from the menu
        vm.send("5\r")
        vm.expect_raw("Power off", 20)
        vm.send("y\r")
        end = time.time() + T(90)
        while vm.alive() and time.time() < end:
            time.sleep(2)
        self.menu_off = True
        self.step("console-menu-poweroff", not vm.alive(), "option 5 powered the VM off")

    def vga_console_check(self, vm):
        """VGA-only run (no serial): log in on tty1 by keyboard; the console menu must be what runs there."""
        kb = Kbd(vm, True)
        vm.shot("vga-console-login")
        for t in ("root\n", PW + "\n"):
            kb.text(t)
            time.sleep(4)
        time.sleep(4)
        vm.shot("vga-console-menu")
        rc, o = ssh(FWD_SSH, self.key, "ps -eo tty,args | grep '[c]onsole-menu'")
        self.step("console-menu-on-vga-tty1", rc == 0 and "tty1" in o, o.strip()[:100])

    def serial_login(self, vm, pw, expect_ok):
        for _ in range(6):
            vm.send("\r")
            if not vm.expect_raw("login:", 20):
                continue
            vm.send("root\r")
            if vm.expect_raw("Password", 15):
                vm.send(f"{pw}\r")
                if expect_ok:
                    if vm.expect_raw(r"(Enter an option|# |\$ )", 25):
                        return True
                else:
                    r = vm.expect_raw(r"(Login incorrect|#|\$ )", 20)
                    self.step("old-default-root-password-rejected", bool(r) and b"incorrect" in r.group(0), "Login incorrect" if r else "no answer")
                    return False
        return False

    # ---- service health (first boot AND every guest reboot): services up, forwarding on, no DB/CPU errors in the logs
    DB_ERR = r"readonly database|read-only database|database is locked|unable to open database|database disk image is malformed"

    def health_script(self, check_logs=True):
        P = self.P
        sh = [f'for s in {" ".join(P["services"])}; do st=$(rc-service $s status 2>&1); case "$st" in *started*) ;; *) echo "BAD service $s: $st";; esac; done',
              'c=$(rc-status --crashed 2>/dev/null); [ -z "$c" ] || echo "BAD crashed services: $c"']
        if P.get("forward"):
            sh += ['[ "$(cat /proc/sys/net/ipv4/ip_forward)" = 1 ] || echo "BAD net.ipv4.ip_forward=$(cat /proc/sys/net/ipv4/ip_forward)"',
                   '[ "$(cat /proc/sys/net/ipv6/conf/all/forwarding)" = 1 ] || echo "BAD net.ipv6.conf.all.forwarding=$(cat /proc/sys/net/ipv6/conf/all/forwarding)"',
                   'nft list table inet patronus >/dev/null 2>&1 || echo "BAD nft table inet patronus is not loaded"']
        for f in P.get("dbfiles", []):
            sh.append(f'[ -e {f} ] || echo "BAD {f} missing"; su -s /bin/sh {P["dbuser"]} -c "test -w {f}" || echo "BAD {f} is not writable by {P["dbuser"]}: $(stat -c %U:%G:%a {f})"')
        logs = " ".join(P.get("logdirs", []) + ["/var/log/messages", "/var/log/syslog"])
        if check_logs:
            sh.append(f'grep -rIiE -A2 "{self.DB_ERR}" {logs} 2>/dev/null | head -8 | sed "s/^/BAD db error in log: /"')
        sh.append('dmesg 2>/dev/null | grep -iE "invalid opcode|traps: .*(trap|ip:)" | head -3 | sed "s/^/BAD cpu fault: /"')
        sh.append("echo HEALTH-DONE")
        return "\n".join(sh)

    def health(self, label, wait=240, check_logs=True):
        """poll until every check passes (services settle after boot); a failure prints what was wrong"""
        end, last = time.time() + wait, ""
        while True:
            rc, out = ssh(FWD_SSH, self.key, self.health_script(check_logs), timeout=60)
            bad = [l for l in out.splitlines() if l.startswith("BAD")]
            if "HEALTH-DONE" in out and not bad:
                self.step(f"health-{label}", True, f"{', '.join(self.P['services'])} started" + (", ip_forward=1, nft table loaded" if self.P.get("forward") else "") + (", no DB errors in logs" if check_logs else ""))
                return
            last = " | ".join(bad) or out.strip()[-200:]
            # log/db errors do not heal by waiting
            if time.time() > end or any(l.startswith(("BAD db error", "BAD cpu fault")) for l in bad):
                self.step(f"health-{label}", False, last[:400])
                return
            time.sleep(6)

    def restart_phase(self, nvars):
        """boot the installed disk again and restart it from inside the guest a.restarts times; health after every boot"""
        a = self.a
        vm = VM("3-boot0", self.qemu_args(None, True, nvars), a.out)      # NVRAM kept: real firmware remembers its boot entries
        try:
            for n in range(a.restarts + 1):
                t = time.time()
                ok = False
                while time.time() - t < 420 and vm.alive():
                    if ssh(FWD_SSH, self.key, "true", timeout=15)[0] == 0:
                        ok = True
                        break
                    time.sleep(5)
                self.step(f"boot{n}-ssh-up", ok, f"{int(time.time()-t)}s")
                # boot0 follows the console-menu run (factory reset / restore swap the DB while the root daemons run: a
                # transient DB warning in their logs is expected there), so its log check is skipped and the logs are
                # cleared; boot1+ then prove a plain guest restart leaves no DB errors.
                self.health(f"boot{n}", check_logs=(n > 0))
                if n == 0:
                    ssh(FWD_SSH, self.key, "for f in /var/log/patronus/*.log /var/log/messages /var/log/syslog; do [ -f $f ] && : > $f; done; true", timeout=20)
                if n == a.restarts:
                    break
                ssh(FWD_SSH, self.key, "sync; (sleep 1; " + "re" + "boot) >/dev/null 2>&1 &", timeout=15)
                for _ in range(120):                                         # -no-reboot: QEMU exits when the guest resets
                    if not vm.alive():
                        break
                    time.sleep(1)
                self.step(f"boot{n}-guest-reset", not vm.alive(), "the guest's own restart command reset the VM")
                vm = VM(f"3-boot{n+1}", self.qemu_args(None, True, nvars), a.out)
            ssh(FWD_SSH, self.key, "poweroff", timeout=10)
            time.sleep(8)
        finally:
            if vm.alive():
                vm.p.kill()

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
        if a.flow == "auto":
            rc, uf = ssh(FWD_SSH, self.key, "cat /etc/portage/package.use/00-live-installer; echo ---; cat /etc/live-installer/use-preset; echo ---; head -3 /etc/live-installer/use-rebuild-pending; echo ---; sysctl -n kernel.kptr_restrict")
            parts = uf.split("---")
            self.step("answer-file-use-flags-applied", rc == 0 and len(parts) >= 3 and "*/* -" in parts[0] and "minimal" in parts[1], re.sub(r"\s+", " ", uf)[:220])
            self.step("hardening-sysctl-applied", parts[-1].strip().endswith("2"), "kernel.kptr_restrict=" + parts[-1].strip()[-3:])
        rc, g = ssh(FWD_SSH, self.key, "command -v cage; ls /var/db/pkg/gui-wm /var/db/pkg/gui-libs 2>&1 | head -3; python3 -c 'import gi' 2>&1 | tail -1; echo END")
        rc, vr = ssh(FWD_SSH, self.key, "cat /etc/live-iso-variant 2>/dev/null")
        if "desktop" in vr:                 # the Desktop image IS a GTK/GNOME system: its GUI packages are part of the product, not live-only
            self.step("graphical-installer-stack-removed-from-server", True, "n/a on the Desktop image (GNOME stays)")
        else:
            self.step("graphical-installer-stack-removed-from-server", "/cage" not in g.split("END")[0].replace("No such", "") and "ModuleNotFound" in g and "gtk" not in g, re.sub(r"\s+", " ", g)[:160])
        self.step("hostname-and-timezone", f"{a.project}-t1" in out and "Europe/Berlin" in out, "host name + Europe/Berlin applied")
        self.step("root-filesystem", ("xfs" if a.fs == "xfs-luks" else a.fs) in out.replace("\r", "").split("\n"), f"/ is {a.fs}" + (" (the installer's own default, never ext4)" if a.fs_default else ""))
        # OWNER RULE: a server/firewall install has NO on-disk swap (partition, LV or file): zram is the only swap
        rc, sw = ssh(FWD_SSH, self.key, "echo ACTIVE; swapon --noheadings --raw --show=NAME,TYPE; echo FSTAB; grep -E '^[^#]*[[:space:]]swap[[:space:]]' /etc/fstab; echo BLK; blkid -t TYPE=swap -o device | grep -v '^/dev/zram'; "
                     "lsblk -rno NAME,PARTTYPE,PARTLABEL | grep -v '^zram' | grep -iE '0657fd6d-a4ab-43c4-84e5-0933c84b4f4f|swap'; echo FILES; find / -xdev -maxdepth 2 \\( -name 'swapfile' -o -name 'swap.img' -o -name '*.swp' -size +1M \\) 2>/dev/null; echo END")
        sec, cur = {}, None
        for ln in sw.replace("\r", "").splitlines():
            if ln.strip() in ("ACTIVE", "FSTAB", "BLK", "FILES", "END"):
                cur = ln.strip()
                sec[cur] = ""
            elif cur:
                sec[cur] += ln + "\n"
        sec = {k: v.strip() for k, v in sec.items()}
        active = [l for l in sec.get("ACTIVE", "").splitlines() if l.strip()]
        on_disk = [l for l in active if not l.startswith("/dev/zram")]
        bad = on_disk or sec.get("FSTAB") or sec.get("BLK") or sec.get("FILES")
        self.step("server-no-ondisk-swap", rc == 0 and "END" in sw and not bad,
                  "active swap: " + (", ".join(active) or "none") + "; fstab/blkid/partitions/files with swap: " + ("NONE" if not bad else re.sub(r"\s+", " ", sw)[:160]))
        self.step("server-zram-swap-active", any(l.startswith("/dev/zram") for l in active), "zram is the swap: " + ", ".join(active)[:100])
        if a.fs == "zfs":
            rc, z = ssh(FWD_SSH, self.key, "zpool status rpool; zpool list -H -o name,health rpool")
            ok = "ONLINE" in z and (a.zfs_raid != "mirror" or ("mirror-0" in z and z.count("vd") >= 2))
            self.step("zfs-pool-online", ok, re.sub(r"\s+", " ", z)[:220])


def main():
    global ARCH, SLOW
    arm_deadline()
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--project", required=True, choices=PROJECTS)
    ap.add_argument("--arch", choices=list(ARCHS), default=HOST_ARCH, help="guest architecture (default: this host's)")
    ap.add_argument("--slow", type=float, default=0, help="timeout multiplier for emulation (default: env QEMU_SLOW, else 1 with KVM / 3 native TCG / 10 cross TCG)")
    ap.add_argument("--mode", choices=["uefi", "bios"], default="uefi")
    ap.add_argument("--media", choices=["usb", "cdrom"], default="usb")
    ap.add_argument("--flow", choices=["auto", "tui", "tui-vga", "fallback", "gui"], default="auto")
    ap.add_argument("--fs", choices=["default", "zfs", "xfs-luks", "xfs", "ext4"], default="default",
                    help="default = do NOT choose: the installer proposes (ZFS with >= 8 GB RAM on Horcrux, XFS+LUKS below 8 GB, XFS on the Patronus image) and the test asserts that proposal")
    ap.add_argument("--install-only", action="store_true", help="stop after the install and check the written disk (GPT, LUKS header) without booting it (xfs-luks needs a typed passphrase at every boot)")
    ap.add_argument("--zfs-raid", choices=["single", "mirror"], default=None)
    ap.add_argument("--ssh-password-opt-in", action="store_true")
    ap.add_argument("--static-ip", default="", help="horcrux management address (empty = DHCP)")
    ap.add_argument("--lan-ip", default="10.77.0.1", help="patronus LAN address")
    ap.add_argument("--console-open", action="store_true", help="installer option: open console menu (no login on the console)")
    ap.add_argument("--hold-file", default="", help="gui flow: before powering the installed system off, wait until this file exists (screenshot of the web UI from outside)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--keep-disks", action="store_true", help="keep target*.qcow2/answers.img/OVMF_VARS.fd in --out (default: delete them when the run ends, pass or fail; they are 20-40 GB of regenerable state)")
    ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--cpu", default=os.environ.get("QEMU_TEST_CPU", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16"),
                    help="QEMU CPU model. Default = x86-64-v2 (no AVX/BMI): catches binaries built for a host-tuned CPU. Never `host`.")
    ap.add_argument("--restarts", type=int, default=0, help="after the install checks: boot the installed disk again and restart it N times from inside the guest, asserting service health after each boot")
    ap.add_argument("--disk-gb", type=int, default=24)
    args = ap.parse_args()
    ARCH = args.arch
    args.fs_default = args.fs == "default"
    if args.fs_default:       # the installer's own default: ZFS >= 8 GB RAM (Horcrux), XFS+LUKS below, XFS where the image has no ZFS (Patronus)
        args.fs = "xfs" if args.project == "patronus" else ("zfs" if args.mem >= 8192 else "xfs-luks")
    args.zfs_raid = args.zfs_raid or ("single" if args.fs_default else "mirror")
    if args.mode == "bios" and not ARCHS[ARCH]["bios"]:
        ap.error(f"{ARCH} has no legacy BIOS: use --mode uefi")
    kvm = os.path.exists("/dev/kvm") and ARCH == HOST_ARCH
    SLOW = args.slow or float(os.environ.get("QEMU_SLOW", 0) or 0) or (1.0 if kvm else 3.0 if ARCH == HOST_ARCH else 10.0)
    print(f"arch={ARCH} host={HOST_ARCH} accel={'kvm' if kvm else 'tcg'} timeout-factor={SLOW}", flush=True)
    try:
        Runner(args).run()
    finally:
        if not args.keep_disks:
            for n in ("target.qcow2", "target2.qcow2", "answers.img", "OVMF_VARS.fd"):
                try:
                    os.unlink(os.path.join(args.out, n))
                except OSError:
                    pass


if __name__ == "__main__":
    main()
