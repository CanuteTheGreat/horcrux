#!/usr/bin/env python3
"""QEMU tests for the boot menu modes and the rescue / forensic / live / persistence modes of a live ISO.

  boot-modes-test.py --iso X.iso --project horcrux --mode uefi|bios --out DIR [--only menu,modes,forensic,rescue,live,persist,shots]

Every test runs in UEFI and BIOS (pick with --mode). Tests that need an installed system run the answer-file install
of qemu-test.py once (DIR/install), turn the disk into a raw image and work on throw-away copies, so "the disk hash is
unchanged" is a byte-for-byte statement about the whole disk.

  menu       GRUB menu on the serial port lists every entry; BIOS: the VGA text screen shows the same entries (QMP pmemsave
             of the text buffer); UEFI: VGA screenshot has the highlighted bar. Screenshots for docs/ISO-INSTALL.md.
  modes      boots every entry (serial-console submenu) and checks the mode the session announces
  forensic   forensic rescue: nothing mounted, no swap, disks read-only at the block layer (write attempts fail), a
             read-only mount + chroot cannot write, SHA-256 of the whole disk is identical before and after
  rescue     rescue mode: finds the installed system, restores a config backup, resets the "forgotten" admin password,
             reinstalls the boot loader, runs fsck; then the installed system boots and the web UI accepts ONLY the new password
  live       try it live: web UI up with the random session password, disks untouched (hash)
  persist    live + persistence on a second disk (encrypted: wrong passphrase refused, right one unlocks; plain too)
  shots      VGA-console screenshots of the rescue / forensic / live screens (docs)
Exit 0 = every step passed; result.json, serial logs and PNGs go to --out.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("qt", os.path.join(HERE, "qemu-test.py"))
qt = importlib.util.module_from_spec(spec)
sys.modules["qt"] = qt
spec.loader.exec_module(qt)
VM, Kbd, http = qt.VM, qt.Kbd, qt.http

T = qt.T          # timeouts scaled for emulation speed (1 with KVM)
PW2 = qt.PW2
PERSIST_PW = "Persist-Passw0rd!"


class Result:
    def __init__(self, a):
        self.a, self.res = a, {"project": a.project, "mode": a.mode, "steps": {}}

    def step(self, name, ok, info=""):
        self.res["steps"][name] = {"ok": bool(ok), "info": info}
        print(f"[{'PASS' if ok else 'FAIL'}] {name} {info}", flush=True)
        json.dump(self.res, open(f"{self.a.out}/result.json", "w"), indent=1)
        if not ok:
            sys.exit(1)


class Tests:
    def __init__(self, a):
        self.a = a
        self.R = Result(a)
        self.step = self.R.step
        self.P = qt.PROJECTS[a.project]
        self.title = a.project.capitalize()
        os.makedirs(a.out, exist_ok=True)
        self.nvars = f"{a.out}/OVMF_VARS.fd"
        self.ns = argparse.Namespace(project=a.project, mode=a.mode, media=a.media, flow="tui", fs="ext4", fs_default=False, install_only=False, zfs_raid="single", static_ip="", lan_ip="10.77.0.1",
                                     mem=3072, ssh_password_opt_in=False, console_open=False, out=a.out, disk_gb=12, arch=a.arch,
                                     cpu=a.cpu, restarts=0)
        self.serial = qt.ARCHS[a.arch]["serial"]

    # ---------------------------------------------------------------- plumbing
    def fresh_vars(self):
        if self.a.mode == "uefi":
            shutil.copy(qt.find_ovmf()[1], self.nvars)

    def args(self, iso, disks=(), extra_disks=()):
        r = qt.Runner(self.ns)
        r.disk = disks[0] if disks else "/dev/null"
        r.disk2, r.answers = "/dev/null", "/dev/null"
        args = r.qemu_args(iso, bool(disks), self.nvars)
        # the installed-disk drive of qemu-test is qcow2; ours are raw images (byte-exact hashes)
        out, skip = [], False
        for i, x in enumerate(args):
            if x == "-drive" and "if=virtio" in args[i + 1]:
                skip = True
                continue
            if skip:
                skip = False
                continue
            out.append(x)
        for d in list(disks) + list(extra_disks):
            out += ["-drive", f"file={d},if=virtio,format={'qcow2' if d.endswith('.qcow2') else 'raw'}"]
        return out

    def boot(self, name, iso=True, disks=(), extra_disks=()):
        self.fresh_vars()
        for attempt in range(3):                      # QEMU occasionally fails to start while another VM is shutting down
            try:
                vm = VM(name, self.args(self.a.iso if iso else None, disks, extra_disks), self.a.out)
                time.sleep(3)
                if vm.p.poll() is None:
                    return vm                          # still running after 3 s: QEMU started (a forwarded port may still be held by the last VM)
                vm.log.close()
            except SystemExit:
                pass
            time.sleep(8)
        raise SystemExit("QEMU would not start")

    def menu_ready(self, vm):
        ok = vm.expect_raw(r"Serial console \(" + self.serial, T(150))
        time.sleep(1.5)
        return ok

    def pick(self, vm, key, serial_submenu=True):
        """GRUB hotkeys: s = serial-console submenu (kernel console on the UART), then the entry's digit.
        GRUB's 6 s countdown leaves little room, and a key that arrives while the menu is still being drawn can be lost:
        repeat until GRUB prints 'Booting `<entry>`'."""
        for attempt in range(4):
            if serial_submenu:
                vm.send("s")
                time.sleep(0.8)
            vm.send(key)
            want = {"1": "Graphical install", "2": "Install \\(text mode", "3": "Advanced", "4": "Automated", "5": "Rescue mode", "6": "Rescue \\(forensic",
                    "7": "Try it live \\(no", "8": "Try it live with", "9": "Safe graphics", "0": "Memory test"}[key]
            if vm.expect_raw(r"Booting `\w+ - " + want, 4.0, consume=False):
                return True
        return False

    def shell(self, vm, timeout=240):
        timeout = T(timeout)
        """wait for a root shell on the serial line (autologin / after the menu)"""
        end = time.time() + timeout
        while time.time() < end:
            vm.send("\recho SHELL-READY-$((20+22))\r")
            if vm.expect_raw(r"SHELL-READY-42", 6):
                return True
        return False

    def run(self, vm, cmd, timeout=90):
        timeout = T(timeout)
        tag = secrets.token_hex(3)
        vm.send(f"{cmd}; echo RC-{tag}-$?\r")
        m = vm.expect_raw(rf"RC-{tag}-(\d+)\r?\n", timeout, consume=False)
        if not m:
            return 255, "(timeout) " + vm.raw[-300:].decode(errors="replace")
        out = vm.raw[:m.start()].decode(errors="replace")
        with vm.lock:
            vm.raw = vm.raw[m.end():]
        return int(m.group(1)), out

    def finish(self, vm, poweroff=True):
        if poweroff and vm.alive():
            vm.send("\rpoweroff\r")
            end = time.time() + 90
            while vm.alive() and time.time() < end:
                time.sleep(1)
        if vm.alive():
            vm.p.kill()
        try:
            vm.p.wait(timeout=20)                     # reaped and its forwarded ports released before the next VM starts
        except Exception:  # noqa
            pass
        time.sleep(1)

    def sha(self, path):
        h = hashlib.sha256()
        with open(path, "rb") as f:
            while True:
                b = f.read(8 << 20)
                if not b:
                    break
                h.update(b)
        return h.hexdigest()

    # ---------------------------------------------------------------- installed disk
    def install_base(self):
        base = f"{self.a.out}/base.qcow2"
        if os.path.exists(base):
            return base
        a = self.a
        d = f"{a.out}/install"
        cmd = [sys.executable, "-u", os.path.join(HERE, "qemu-test.py"), "--iso", a.iso, "--project", a.project, "--mode", a.mode, "--media", a.media,
               "--flow", "auto", "--fs", "ext4", "--disk-gb", "12", "--keep-disks", "--arch", a.arch, "--out", d]
        t = time.time()
        r = subprocess.run(cmd, capture_output=True, text=True)
        open(f"{a.out}/install.log", "w").write(r.stdout + r.stderr)
        self.step("install-base-system", r.returncode == 0, f"answer-file install ({int(time.time()-t)}s)")
        os.rename(f"{d}/target.qcow2", base)
        return base

    def disk_copy(self, name, raw=False):
        """raw=True: a full sparse copy (byte-exact SHA-256); otherwise a copy-on-write overlay of the base image (cheap)"""
        base = self.install_base()
        if raw:
            dst = f"{self.a.out}/{name}.raw"
            subprocess.check_call(["qemu-img", "convert", "-O", "raw", base, dst])
        else:
            dst = f"{self.a.out}/{name}.qcow2"
            subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", "-b", base, "-F", "qcow2", dst])
        return dst

    def untouched(self, overlay):
        """True when the guest wrote NOTHING to an overlay disk: no allocated cluster and identical virtual content"""
        m = json.loads(subprocess.check_output(["qemu-img", "map", "--output=json", overlay]))
        written = sum(e["length"] for e in m if e.get("data") and not e.get("zero") and e.get("depth", 0) == 0)
        r = subprocess.run(["qemu-img", "compare", "-f", "qcow2", "-F", "qcow2", overlay, f"{self.a.out}/base.qcow2"], capture_output=True, text=True)
        return written == 0 and r.returncode == 0, written

    # ---------------------------------------------------------------- tests
    def t_menu(self):
        vm = self.boot("menu")
        try:
            self.step("grub-menu-reached", self.menu_ready(vm))
            scr = vm.screen()
            vm.shot("grub-menu")
            want = ["Graphical install", "Install (text mode)", "Advanced (expert) install", "Automated install", "Rescue mode",
                    "Rescue (forensic, no-touch", "Try it live (no install", "Try it live with persistence", "Safe graphics (nomodeset)", "Serial console"]
            if self.a.project and not self.a.no_memtest:
                want.append("Memory test")
            norm = lambda s: re.sub(r"[^a-z0-9]", "", s.lower())
            missing = [w for w in want if norm(w) not in norm(scr)]
            self.step("serial-menu-lists-every-entry", not missing, f"missing: {missing}" if missing else f"{len(want)} entries on the serial console")
            self.step("menu-is-branded", self.title in scr, f"'{self.title} - ...' entries")
            self.step("graphical-install-is-first-and-default", re.search(r"Graphical install", scr) is not None and scr.find("Graphical install") < scr.find("Install (text mode)"))
            ppm = vm.grab() or b""
            m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", ppm)
            px = ppm[m.end():] if m else b""
            teal = sum(1 for i in range(0, len(px) - 2, 3) if px[i] < 60 and px[i + 1] > 120 and px[i + 2] > 120)
            lit = sum(1 for i in range(0, len(px) - 2, 3 * 7) if px[i] + px[i + 1] + px[i + 2] > 60)
            text = ""
            if self.a.mode == "bios":
                dump = f"{self.a.out}/vgatext.bin"
                vm.qmp("pmemsave", val=0xB8000, size=4000, filename=os.path.abspath(dump))
                time.sleep(1)
                raw = open(dump, "rb").read()
                text = "".join(chr(c) if 32 <= c < 127 else " " for c in raw[0::2])
            if len(norm(text)) > 200:                      # BIOS text mode: the VGA buffer holds the same entries as the serial menu
                missing = [w for w in want if norm(w) not in norm(text)]
                self.step("vga-text-screen-shows-the-same-entries", not missing, f"missing on VGA: {missing}" if missing else "VGA text buffer == serial menu entries")
            else:                                          # gfxterm with the product splash: the text is drawn as pixels
                self.step("vga-menu-drawn-with-splash", teal > 3000 or lit > 20000, f"graphical menu on the screen ({teal} highlight px, {lit} lit samples); the serial mirror lists the same entries")
        finally:
            self.finish(vm, False)

    MODES = [("1", "gui", None, "Graphical install -> GTK installer running in the cage kiosk on tty1 (the UART gets a root shell)"),
             ("2", "text", r"boot mode: text[\s\S]*(Language, keyboard|installer)", "Install (text mode)"),
             ("3", "expert", r"boot mode: expert", "Advanced (expert) install"),
             ("9", "safe", r"boot mode: text", "Safe graphics (nomodeset) -> text installer"),
             ("5", "rescue", r"RESCUE MODE", "Rescue mode"),
             ("6", "forensic", r"FORENSIC RESCUE \(no-touch\)", "Forensic rescue"),
             ("7", "live", r"LIVE-SESSION-READY", "Try it live"),
             ("8", "livepersist", r"No persistence partition", "Try it live with persistence (asks to create one)")]

    def t_modes(self):
        for key, name, pat, what in self.MODES:
            vm = self.boot(f"mode-{name}")
            try:
                if not self.menu_ready(vm):
                    self.step(f"mode-{name}", False, "no menu")
                self.pick(vm, key)
                if pat is None:                                 # graphical install: the UART is a root shell, the GUI runs on tty1
                    ok = self.shell(vm)
                    end = time.time() + T(180)
                    out = ""
                    while ok and time.time() < end:
                        rc, out = self.run(vm, "echo MODE=$(cat /run/live-installer/mode) $(pgrep -f 'cage -s -- /usr/bin/live-installer' | head -1 | sed 's/^/CAGE=/')")
                        if re.search(r"CAGE=\d+", out):
                            break
                        time.sleep(3)
                    self.step(f"mode-{name}", ok and re.search(r"MODE=gui\b", out) and re.search(r"CAGE=\d+", out), what)
                else:
                    ok = vm.expect_raw(pat, T(300))
                    self.step(f"mode-{name}", ok, what)
            finally:
                self.finish(vm, False)
        if not self.a.no_memtest:
            vm = self.boot("mode-memtest")
            try:
                self.menu_ready(vm)
                vm.send("0")                                   # memtest86+ (BIOS: serial console parameter given)
                if self.a.mode == "bios":
                    ok = vm.expect_raw(r"(?i)memtest86\+", T(120))
                    self.step("mode-memtest", ok, "memtest86+ banner on the serial console")
                else:
                    time.sleep(25)
                    ppm = vm.grab() or b""
                    m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", ppm)
                    blue = 0
                    if m:
                        px = ppm[m.end():]
                        blue = sum(1 for i in range(0, len(px) - 2, 3 * 997) if px[i + 2] > 120 and px[i] < 60)
                    vm.shot("memtest")
                    self.step("mode-memtest", blue > 20 and vm.cyan_pixels(ppm) < 50, f"memtest86+ screen (blue samples {blue}) replaced the GRUB menu")
            finally:
                self.finish(vm, False)

    def t_forensic(self):
        disk = self.disk_copy("forensic-disk", raw=True)
        before = self.sha(disk)
        self.step("disk-hash-before", True, before[:16])
        vm = self.boot("forensic", disks=[disk])
        try:
            self.menu_ready(vm)
            self.pick(vm, "6")
            self.step("forensic-banner", vm.expect_raw(r"FORENSIC RESCUE \(no-touch\)", T(300)), "all disks read-only message")
            self.step("forensic-menu", vm.expect_raw(r"Enter an option", T(60)))
            # 0 -> the live shell: inspect the state the session start left behind
            vm.send("0\r")
            self.step("live-shell", self.shell(vm))
            rc, o = self.run(vm, "echo \"M=$(grep -E '^/dev/(vd|sd|nvme)' /proc/mounts | grep -v iso9660 | wc -l) S=$(cat /proc/swaps | wc -l) RO=$(blockdev --getro /dev/vda /dev/vda1 /dev/vda4 | tr -d '\\n')\"")
            mm = re.search(r"M=(\d+) S=(\d+) RO=(\d+)", o)
            nums = list(mm.groups()[:2]) if mm else []
            ro = mm and mm.group(3) == "111"
            detail = ""
            if not nums or nums[0] != "0":
                detail = re.sub(r"\s+", " ", self.run(vm, "grep -E '^/dev/(vd|sd|nvme)' /proc/mounts")[1])[:200]
            self.step("nothing-auto-mounted", nums and nums[0] == "0", f"disk mounts at session start: {nums[0] if nums else '?'} {detail}")
            self.step("swap-never-enabled", len(nums) > 1 and nums[1] == "1", "/proc/swaps has only its header line")
            self.step("block-layer-read-only", bool(ro), "blockdev --getro: 1 1 1 for the whole disk and its partitions")
            rc, o = self.run(vm, "dd if=/dev/zero of=/dev/vda4 bs=512 count=8 conv=notrunc 2>&1; echo; mount -o rw /dev/vda4 /mnt 2>&1; touch /mnt/x 2>&1")
            self.step("write-attempts-fail", ("Operation not permitted" in o or "Permission denied" in o) and "write-protected" in o and "Read-only file system" in o, "dd to the device refused; mount -o rw falls back to read-only (write-protected); touch fails")
            self.run(vm, "umount /mnt 2>/dev/null")
            vm.send("live-rescue forensic\r")
            vm.expect_raw(r"Enter an option", T(60))
            vm.send("2\r")
            m = vm.expect_raw(r"(\d+)\) (/dev/vda4)\s+ext4", T(30), consume=False)
            num = m.group(1).decode() if m else "1"
            vm.expect_raw(r"Mount which one read-only", 20)
            vm.send(num + "\r")
            self.step("readonly-mount", vm.expect_raw(r"Mounted READ-ONLY at (/mnt/rescue/\S+)", T(60)), "only on request")
            vm.expect_raw(r"Enter an option", 20)
            vm.send("3\r")
            vm.expect_raw(r"read-only chroot|Shell inside", T(30))
            time.sleep(1)
            rc, o = self.run(vm, "touch /should-fail 2>&1; ls /etc/os-release")
            self.step("read-only-root-in-chroot", "Read-only file system" in o and "/etc/os-release" in o, "touch in the chroot: Read-only file system")
            vm.send("exit\r")
            vm.expect_raw(r"Enter an option", T(30))
            vm.send("5\r")
            vm.expect_raw(r"Device to hash", 20)
            vm.send("/dev/vda4\r")
            h = vm.expect_raw(r"SHA256 /dev/vda4 ([0-9a-f]{64})", T(600))
            self.step("device-sha256", bool(h), "chain-of-custody hash of /dev/vda4 printed")
            vm.expect_raw(r"Enter an option", 20)
            vm.send("0\r")
            self.shell(vm)
            rc, o = self.run(vm, "cat /proc/mounts | grep -c 'vda' ; true")
            vm.send("\x03")
        finally:
            self.finish(vm)
        after = self.sha(disk)
        self.step("disk-hash-unchanged", after == before, f"{before[:16]} == {after[:16]}" if after == before else f"CHANGED {before[:16]} -> {after[:16]}")
        os.unlink(disk)

    def web_code(self, pw, wait=240, port=None):
        end = time.time() + wait
        port = port or qt.FWD_WEB
        while True:
            sc, code, resp = http(port, "/api/auth/login", "POST", {"username": "admin", "password": pw})
            if code in (200, 401, 403) or time.time() >= end:
                return code
            time.sleep(5)

    def t_rescue(self):
        disk = self.disk_copy("rescue-disk")
        vm = self.boot("rescue", disks=[disk])
        try:
            self.menu_ready(vm)
            self.pick(vm, "5")
            self.step("rescue-banner", vm.expect_raw(r"RESCUE MODE", T(300)))
            m = vm.expect_raw(r"Installed systems found:\s*\r?\n\s*1\) ([^\r\n]*)\r?\n", T(120))
            self.step("finds-the-installed-system", bool(m) and b"/dev/vda4" in m.group(0), (m.group(1).decode()[:100] if m else "none"))
            self.step("mounted-for-work", vm.expect_raw(r"Mounted .* at /mnt/rescue/\S+", T(60)))
            vm.expect_raw(r"Enter an option", T(30))
            # --- backup + restore: make a config backup inside the chroot, change the config, restore the backup
            vm.send("1\r")
            vm.expect_raw(r"Shell inside the installed system", T(30))
            time.sleep(1)
            cfgdir = "/etc/" + self.a.project
            bcmd = (f"echo ORIGINAL > {cfgdir}/rescue-test.conf && python3 -c 'import sys; sys.path.insert(0,\"/usr/lib/live-installer\"); "
                    f"from liveinst import system as S, consolemenu as C; print(\"BACKUP\", C.Console(S.load_profile()).make_backup(\"rescuetest\"))' && "
                    f"echo CHANGED > {cfgdir}/rescue-test.conf")
            for attempt in (1, 2):                      # the first command typed into a freshly started chroot shell is sometimes lost on a loaded host
                rc, o = self.run(vm, bcmd, 120)
                if "BACKUP /var/backups" in o:
                    break
                time.sleep(3)
            self.step("backup-made-in-chroot", "BACKUP /var/backups" in o,
                      "config backup written inside the installed system by its own console-menu code" if "BACKUP /var/backups" in o
                      else f"rc={rc} attempt={attempt} output tail: {o[-600:]!r}")
            vm.send("exit\r")
            vm.expect_raw(r"Enter an option", T(30))
            vm.send("3\r")
            m = vm.expect_raw(r"(\d+)\) (\S*rescuetest\S*\.tar\.gz)", T(30), consume=False)
            self.step("restore-lists-the-backup", bool(m))
            vm.expect_raw(r"Backup to restore", 20)
            vm.send((m.group(1).decode() if m else "1") + "\r")
            vm.expect_raw(r"Type RESTORE", 20)
            vm.send("RESTORE\r")
            self.step("config-restored", vm.expect_raw(r"RESTORE-DONE", T(60)))
            vm.expect_raw(r"Enter an option", 20)
            vm.send("1\r")
            vm.expect_raw(r"Shell inside", T(30))
            time.sleep(1)
            rc, o = self.run(vm, f"cat {cfgdir}/rescue-test.conf")
            self.step("restore-brought-back-the-saved-file", "ORIGINAL" in o and "CHANGED" not in o, "the file changed after the backup has its backed-up content again")
            vm.send("exit\r")
            vm.expect_raw(r"Enter an option", T(30))
            # --- the forgotten admin password
            vm.send("2\r")
            vm.expect_raw(r"New admin password", 20)
            vm.send(PW2 + "\r")
            vm.expect_raw(r"Confirm", 20)
            vm.send(PW2 + "\r")
            vm.expect_raw(r"same password for the root", 20)
            vm.send("y\r")
            self.step("admin-password-reset", vm.expect_raw(r"PASSWORD-RESET-DONE", T(90)), "argon2id hash written into the product DB")
            vm.expect_raw(r"Enter an option", 20)
            # --- boot loader
            vm.send("4\r")
            self.step("bootloader-plan", vm.expect_raw(r"Firmware: (UEFI|BIOS)", T(30)))
            if self.a.mode == "bios":
                vm.expect_raw(r"Install GRUB to disk", 20)
                vm.send("\r")
            vm.expect_raw(r"Reinstall the boot loader", 20)
            vm.send("y\r")
            self.step("bootloader-reinstalled", vm.expect_raw(r"BOOTLOADER-DONE", T(240)), "grub-install + grub-mkconfig in the chroot")
            vm.expect_raw(r"Enter an option", 20)
            # --- fsck
            vm.send("5\r")
            self.step("fsck-ran", vm.expect_raw(r"FSCK-DONE", T(300)), "e2fsck on the unmounted root")
            vm.expect_raw(r"Mount the system again", 20)
            vm.send("n\r")
            vm.expect_raw(r"Enter an option", 20)
            vm.send("0\r")
            self.step("menu-exit-unmounts", vm.expect_raw(r"everything is unmounted", T(60)))
        finally:
            self.finish(vm)
        # ---- boot the repaired system from its own disk and log in to the web UI
        self.fresh_vars()
        vm = VM("rescue-installed", self.args(None, [disk]), self.a.out)
        try:
            self.step("installed-system-boots-after-rescue", vm.expect_raw(r"login:", T(420)), "bootloader works (menu)" if True else "")
            self.step("web-login-with-the-new-password", self.web_code(PW2) == 200, "POST /api/auth/login -> 200")
            self.step("web-login-with-a-wrong-password-refused", self.web_code("not-the-password", 5) in (401, 403), "refused")
            vm.send("root\r")
            if vm.expect_raw("Password", 20):
                vm.send(PW2 + "\r")
                self.step("root-console-login-with-the-new-password", bool(vm.expect_raw(r"(Enter an option|# |\$ )", T(40))))
        finally:
            self.finish(vm, False)
        os.unlink(disk)

    def t_live(self):
        disk = self.disk_copy("live-disk")
        vm = self.boot("live", disks=[disk])
        try:
            self.menu_ready(vm)
            self.pick(vm, "7")
            self.step("live-session-ready", vm.expect_raw(r"LIVE-SESSION-READY", T(360), consume=False))
            raw = vm.raw.decode(errors="replace")
            pw = re.search(r"Admin password : (\S+)", raw)
            self.step("random-session-password-shown", bool(pw) and len(pw.group(1)) >= 12, "per-session password on the console, none baked in")
            # Patronus: the live system has no static LAN address, its WAN-side (DHCP) port is the one QEMU forwards
            lp = qt.FWD_WAN if self.a.project == "patronus" else qt.FWD_WEB
            code = self.web_code(pw.group(1) if pw else "x", port=lp)
            self.step("web-ui-up-in-live-session", code == 200, f"full product served from RAM: login -> {code}")
            self.step("default-passwords-refused", self.web_code(self.P["old_pw"], 5, port=lp) in (401, 403), "the product's old default is not accepted")
            self.step("live-sshd-off", not qt.ssh_banner(qt.FWD_SSH).startswith("SSH-"), "no sshd in the live session")
            self.step("live-shell", self.shell(vm))
            rc, o = self.run(vm, "echo \"V=$(grep -c vda /proc/mounts) W=$(cat /proc/swaps | wc -l)\"")
            mm = re.search(r"V=(\d+) W=(\d+)", o)
            self.step("live-disks-not-mounted-no-swap", bool(mm) and mm.groups() == ("0", "1"), f"disk mounts {mm.group(1) if mm else '?'}, swap lines {mm.group(2) if mm else '?'} (1 = header only)")
        finally:
            self.finish(vm)
        ok, written = self.untouched(disk)
        self.step("live-disk-unchanged", ok, f"{written} bytes written to the disk; virtual content identical to the installed image")
        os.unlink(disk)

    def t_persist(self):
        modes = [True] + ([False] if self.a.mode == "bios" else [])
        for enc in modes:
            tag = "enc" if enc else "plain"
            pdisk = f"{self.a.out}/persist-{tag}.raw"
            subprocess.check_call(["qemu-img", "create", "-q", "-f", "raw", pdisk, "512M"])
            marker = secrets.token_hex(6)
            vm = self.boot(f"persist-{tag}-1", extra_disks=[pdisk])
            try:
                self.menu_ready(vm)
                self.pick(vm, "8")
                self.step(f"persist-{tag}-offers-to-create", vm.expect_raw(r"Create one now", T(300)))
                vm.send("y\r")
                vm.expect_raw(r"Encrypt it", 20)
                vm.send(("y" if enc else "n") + "\r")
                vm.expect_raw(r"Device \(empty", 20)
                vm.send("/dev/vda\r")
                if enc:
                    vm.expect_raw(r"New passphrase", T(60))
                    vm.send(PERSIST_PW + "\r")
                    vm.expect_raw(r"Repeat", 20)
                    vm.send(PERSIST_PW + "\r")
                self.step(f"persist-{tag}-created", vm.expect_raw(r"PERSISTENCE-CREATED", T(240)), "LUKS2 + ext4" if enc else "ext4")
                self.step(f"persist-{tag}-shell", self.shell(vm))
                rc, o = self.run(vm, f"echo {marker} > /root/keep.txt; echo {marker} > /etc/keep.conf; mkdir -p /var/lib/keepme && echo {marker} > /var/lib/keepme/f; sync; "
                                     f"blkid | grep -c -i 'persist'")
                self.step(f"persist-{tag}-files-written", rc == 0)
            finally:
                self.finish(vm)
            if enc:
                head = open(pdisk, "rb").read(4 << 20)          # the partition starts at 1 MiB: its first sector is the LUKS header
                self.step("persist-enc-disk-is-luks-not-plaintext", b"LUKS\xba\xbe" in head, "LUKS2 header on the partition")
                body = open(pdisk, "rb").read()
                self.step("persist-enc-marker-not-in-plaintext", marker.encode() not in body, "marker string absent from the raw device")
            vm = self.boot(f"persist-{tag}-2", extra_disks=[pdisk])
            try:
                self.menu_ready(vm)
                self.pick(vm, "8")
                if enc:
                    self.step("persist-enc-asks-passphrase", vm.expect_raw(r"Passphrase for the encrypted persistence", T(300)))
                    vm.send("wrong-passphrase\r")
                    self.step("persist-enc-wrong-passphrase-refused", vm.expect_raw(r"Wrong passphrase", T(60)))
                    vm.expect_raw(r"Passphrase for the encrypted persistence", T(30))
                    vm.send(PERSIST_PW + "\r")
                    self.step("persist-enc-unlocked", vm.expect_raw(r"PERSISTENCE-UNLOCKED", T(120)))
                else:
                    self.step("persist-plain-no-prompt", vm.expect_raw(r"LIVE-SESSION-READY", T(360), consume=False))
                self.step(f"persist-{tag}-shell-2", self.shell(vm))
                rc, o = self.run(vm, "cat /root/keep.txt /etc/keep.conf /var/lib/keepme/f | sort -u")
                self.step(f"persist-{tag}-files-survived-reboot", o.count(marker) >= 1 and rc == 0, "/root /etc /var/lib survive")
                rc, o = self.run(vm, "live-persist status")
                self.step(f"persist-{tag}-state-active", "active" in o)
            finally:
                self.finish(vm)
            os.unlink(pdisk)

    def settle(self, vm, minimum=6, timeout=120):
        """wait for the VGA picture to stop changing"""
        t0, prev, same = time.time(), None, 0
        while time.time() - t0 < timeout:
            g = vm.grab()
            if g and prev == g and time.time() - t0 > minimum:
                same += 1
                if same >= 2:
                    return True
            else:
                same = 0
            prev = g
            time.sleep(2)
        return False

    def t_shots(self):
        """VGA screenshots of the text menu and the rescue/forensic/live screens (VGA console, no serial)"""
        disk = self.disk_copy("shots-disk")
        for key, name in (("5", "rescue"), ("6", "forensic"), ("7", "live")):
            vm = self.boot(f"shot-{name}", disks=[disk])
            try:
                time.sleep(9 if self.a.mode == "uefi" else 4)
                kb = Kbd(vm, True)
                kb.text(key, 0.1)
                self.settle(vm, minimum=45 if name != "rescue" else 60, timeout=420)
                vm.shot(f"{name}-vga")
                self.step(f"screenshot-{name}", True, f"{self.a.out}/shot-{name}-{name}-vga.png")
            finally:
                self.finish(vm, False)
        os.unlink(disk)


def main():
    qt.arm_deadline()
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--project", required=True, choices=list(qt.PROJECTS))
    ap.add_argument("--mode", choices=["uefi", "bios"], default="uefi")
    ap.add_argument("--media", choices=["usb", "cdrom"], default="usb")
    ap.add_argument("--arch", choices=list(qt.ARCHS), default=qt.HOST_ARCH)
    ap.add_argument("--slow", type=float, default=0)
    ap.add_argument("--cpu", default=os.environ.get("QEMU_TEST_CPU", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16"))
    ap.add_argument("--only", default="menu,modes,forensic,rescue,live,persist")
    ap.add_argument("--no-memtest", action="store_true", help="the ISO carries no memtest86+ (not packaged for this arch)")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    qt.ARCH = a.arch
    kvm = os.path.exists("/dev/kvm") and a.arch == qt.HOST_ARCH
    qt.SLOW = a.slow or float(os.environ.get("QEMU_SLOW", 0) or 0) or (1.0 if kvm else 3.0 if a.arch == qt.HOST_ARCH else 10.0)
    if a.mode == "bios" and not qt.ARCHS[a.arch]["bios"]:
        ap.error(f"{a.arch} has no legacy BIOS: use --mode uefi")
    if a.arch != "x86_64":
        a.no_memtest = True                         # memtest86+ is x86 only
    t = Tests(a)
    for name in a.only.split(","):
        print(f"=== {name} ({a.mode})", flush=True)
        getattr(t, "t_" + name)()
    print("ALL PASS", a.project, a.mode, a.only)


if __name__ == "__main__":
    main()
