#!/usr/bin/env python3
"""QEMU test for the Horcrux *Surface* ISO: an old-tablet install, emulated.

  surface-qemu-test.py --iso horcrux-surface-*.iso --out DIR [--mem 4096] [--cpu IvyBridge]

What it proves (evidence: result.json, screens/*.png, serial logs in --out):
  * CPU model IvyBridge (AVX but NO AVX2/FMA/BMI) + 4 GiB RAM + UEFI: every binary on the ISO and the installed system runs
  * DMI is faked as the real SP1 (`-smbios type=1,manufacturer="Microsoft Corporation",product="Surface with Windows 8 Pro"`):
    the installer's hardware-profile autodetect must pick surface / surface-pro-1 by itself (no profile in the answer file)
  * unattended install of XFS-in-LUKS2 (argon2id, memory cost <= 1 GiB), no swap partition
  * the installed disk is booted with NO PS/2 keyboard (`i8042=off`): the LUKS passphrase can only be typed on a USB-HID
    keyboard, i.e. the initramfs really carries xhci + usbhid + hid_*; a wrong passphrase first, then the right one
  * after unlock: XFS root without `discard`, zram swap, fstrim works through dm-crypt, NetworkManager/nmtui, the
    `button.lid_init_state=open` quirk, sway + wvkbd + swaybar running for the autologin user, screenshots of the LUKS
    prompt, the desktop and the on-screen keyboard
Reuses the harness classes of build/live-iso/common/qemu-test.py. Python stdlib + qemu + ovmf + openssh-client + openssl + e2fsprogs.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
HARNESS = os.environ.get("HX_QEMU_TEST") or os.path.join(HERE, "..", "..", "..", "build", "live-iso", "common", "qemu-test.py")
spec = importlib.util.spec_from_file_location("hxqt", HARNESS)
Q = importlib.util.module_from_spec(spec)
spec.loader.exec_module(Q)

LUKS_PW = "Wq4#vN8-correct-horse-Zx2"
USER, USER_PW = "surface", "Bd6!hP3-tablet-user-Rk9"
DMI = "type=1,manufacturer=Microsoft Corporation,product=Surface with Windows 8 Pro,version=1,sku=Surface_Pro"


class T:
    def __init__(self, a):
        self.a, self.res = a, {"iso": os.path.basename(a.iso), "cpu": a.cpu, "mem": a.mem, "steps": {}}
        self.failed, self.soft = [], False         # soft = the post-boot checks all run even when one fails
        os.makedirs(a.out + "/screens", exist_ok=True)

    def step(self, name, ok, info=""):
        self.res["steps"][name] = {"ok": bool(ok), "info": info}
        print(f"[{'PASS' if ok else 'FAIL'}] {name} {info}", flush=True)
        json.dump(self.res, open(f"{self.a.out}/result.json", "w"), indent=1)
        if not ok:
            self.failed.append(name)
            if not self.soft:
                raise SystemExit(1)

    # --- QEMU command lines ------------------------------------------------------------------------------
    def base(self, installed, nvars):
        a = self.a
        kvm = os.path.exists("/dev/kvm")
        mach = "q35,accel=kvm" if kvm else "q35"
        if installed:
            mach += ",i8042=off"                      # NO PS/2 keyboard: only the USB keyboard below can type
        args = [Q.ARCHS["x86_64"]["qemu"], "-machine", mach, "-cpu", a.cpu, "-smp", "2", "-m", str(a.mem),
                "-smbios", DMI,
                "-drive", f"file={self.disk},if=none,id=hd0,format=qcow2,discard=unmap",
                "-device", "virtio-blk-pci,drive=hd0,discard=on",
                "-netdev", f"user,id=n0,hostfwd=tcp:127.0.0.1:{Q.FWD_SSH}-:22", "-device", "virtio-net-pci,netdev=n0",
                "-device", "VGA", "-no-reboot", "-device", "qemu-xhci,id=xhci", "-device", "usb-kbd,bus=xhci.0",
                "-device", "usb-tablet,bus=xhci.0"]
        code, _ = Q.find_ovmf("x86_64")
        args += ["-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={nvars}"]
        if not installed:
            args += ["-drive", f"if=none,id=stick,file={a.iso},format=raw,readonly=on", "-device", "usb-storage,bus=xhci.0,drive=stick,bootindex=0",
                     "-drive", f"if=none,id=ans,file={self.answers},format=raw", "-device", "usb-storage,bus=xhci.0,drive=ans"]
        return args

    def answer(self, pwhash, pub):
        return "\n".join([
            "[global]", 'keyboard = "us"', 'timezone = "Europe/Berlin"', 'hostname = "surface-t1.test.lan"', 'email = "ops@example.test"',
            f'root_password_hash = "{pwhash}"', f'ssh_keys = ["{pub}"]', "ssh_password_auth = false", "",
            "[disk]", 'filesystem = "xfs-luks"', f'luks_passphrase = "{LUKS_PW}"', 'disks = ["/dev/vda"]', "",
            "[desktop]", "enable = true", f'user = "{USER}"', f'password = "{USER_PW}"', "autologin = true", "",
            "[network.management]", 'mode = "dhcp"', ""])

    def key(self, vm, text):
        for ch in text:
            vm.qmp("send-key", keys=[{"type": "qcode", "data": k} for k in Q.qcodes(ch)])
            time.sleep(0.06)

    def settle(self, vm, label, timeout=240, quiet=3, poll=4):
        """wait until the set of text rows on the VGA screen stays the same `quiet` polls in a row (a prompt is idle); returns the PPM"""
        last, same, end = None, 0, time.time() + timeout
        while time.time() < end:
            ppm = vm.grab() or b""
            m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+\d+\s", ppm)
            if m:
                w, h = int(m.group(1)), int(m.group(2))
                px = ppm[m.end():]
                rows = tuple(y for y in range(0, h, 4) if any(px[(y * w + x) * 3] > 60 for x in range(0, min(w, 700), 3)))
                if rows and rows == last:
                    same += 1
                    if same >= quiet:
                        return ppm
                else:
                    same = 0
                last = rows
            time.sleep(poll)
        return None

    def ssh(self, cmd, timeout=60):
        return Q.ssh(Q.FWD_SSH, self.key_path, cmd, timeout=timeout)

    def user_sh(self, cmd, timeout=60):
        """run as the desktop user inside its sway session (XDG_RUNTIME_DIR + SWAYSOCK discovered)"""
        inner = ('export XDG_RUNTIME_DIR=/run/user/$(id -u); [ -d $XDG_RUNTIME_DIR ] || XDG_RUNTIME_DIR=/tmp/xdg-$(id -u); '
                 'export SWAYSOCK=$(ls $XDG_RUNTIME_DIR/sway-ipc.*.sock 2>/dev/null | head -1); '
                 'export WAYLAND_DISPLAY=$(ls $XDG_RUNTIME_DIR | grep -m1 "^wayland-[0-9]*$"); ' + cmd)
        return self.ssh(f"su - {USER} -c {json.dumps(inner)}", timeout=timeout)

    # --- phases ------------------------------------------------------------------------------------------------
    def run(self):
        a = self.a
        self.disk, self.answers, nvars = f"{a.out}/target.qcow2", f"{a.out}/answers.img", f"{a.out}/OVMF_VARS.fd"
        subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", self.disk, f"{a.disk_gb}G"])
        Q.ARCH = "x86_64"
        Q.pflash_vars(nvars)
        self.key_path = f"{a.out}/test_key"
        for f in (self.key_path, self.key_path + ".pub"):
            if os.path.exists(f):
                os.unlink(f)
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "qemu-test", "-f", self.key_path])
        pub = open(self.key_path + ".pub").read().strip()
        pwhash = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemutest1", Q.PW]).decode().strip()
        Q.make_answer_image(self.answers, "horcrux", self.answer(pwhash, pub))
        self.step("answer-file-built", True, "xfs-luks + desktop user, NO hardware profile named (autodetect must find it)")
        self.install(nvars)
        Q.pflash_vars(nvars)                                 # fresh NVRAM = the firmware rescans, like a real first boot
        self.boot_installed(nvars)

    def install(self, nvars):
        a, t0 = self.a, time.time()
        vm = Q.VM("1-install", self.base(False, nvars), a.out)
        try:
            ok = vm.expect_raw(r"safe graphics", 150)
            self.step("grub-menu-reached", ok, f"{int(time.time() - t0)}s, cpu={a.cpu}, {a.mem} MiB, DMI Microsoft Corporation / Surface with Windows 8 Pro")
            time.sleep(2)
            for _ in range(2):                               # entry 2 = automated install (serial console: we watch its log)
                vm.send("\x1b[B")
                time.sleep(0.6)
            vm.send("\r")
            self.step("live-userland-up", vm.expect_raw(r"AUTOINSTALL-START", 480), "answer file found")
            plan = vm.expect_raw(r"AUTOINSTALL-PLAN[^\n]*", 60, consume=False)
            self.step("plan-is-xfs-luks", bool(plan) and b"fs=xfs-luks" in plan.group(0), plan.group(0).decode() if plan else "no plan line")
            ok = vm.expect_raw(r"AUTOINSTALL-(OK|FAILED[^\n]*)", 1700, consume=False)
            log = open(f"{a.out}/1-install-serial.log", "rb").read().decode("latin1")
            self.step("hardware-profile-autodetected", "hardware profile: surface / surface-pro-1" in log, "installer log: 'hardware profile: surface / surface-pro-1'")
            m = re.search(r"memory cost (\d+) MiB \(RAM (\d+) MB\)", log)
            self.step("luks-argon2id-memory-sane", bool(m) and int(m.group(1)) <= 1024 and int(m.group(1)) * 4 <= int(m.group(2)), m.group(0) if m else "no memory-cost line")
            self.step("initramfs-has-keyboard-stack", "all reachable at the passphrase prompt" in log, "installer's lsinitrd check: cryptsetup, dm-crypt, usbhid, xhci, hid-multitouch, hid-microsoft")
            self.step("installer-finished", ok and b"AUTOINSTALL-OK" in ok.group(0), ok.group(0).decode() if ok else "timeout")
            vm.shot("install-done")
            end = time.time() + 240
            while vm.alive() and time.time() < end:
                time.sleep(1)
            self.step("vm-stopped-after-install", not vm.alive(), f"{int(time.time() - t0)}s total")
        finally:
            if vm.alive():
                vm.p.kill()

    def boot_installed(self, nvars):
        a, t0 = self.a, time.time()
        vm = Q.VM("2-installed", self.base(True, nvars), a.out)
        try:
            ppm = self.settle(vm, "luks", timeout=420)
            self.step("luks-prompt-idle-on-vga", ppm is not None, f"{int(time.time() - t0)}s: console text stable (passphrase prompt), no serial console on this boot")
            vm.shot("luks-prompt")
            self.key(vm, "wrong-passphrase-123\n")           # the prompt must come back (rd.luks.tries) and still accept USB keys
            time.sleep(12)
            vm.shot("luks-wrong-passphrase")
            ppm2 = self.settle(vm, "luks2", timeout=120)
            self.step("prompt-returns-after-wrong-passphrase", ppm2 is not None and not self.ssh_up(), "still locked, asks again")
            self.key(vm, LUKS_PW + "\n")
            t1 = time.time()
            up = False
            while time.time() - t1 < 420 and vm.alive():
                if self.ssh("true", timeout=15)[0] == 0:
                    up = True
                    break
                time.sleep(5)
            self.step("unlocked-with-usb-keyboard-and-booted", up, f"{int(time.time() - t0)}s after power-on; ssh key login works (PS/2 controller disabled)")
            self.checks(vm)
        finally:
            if vm.alive():
                self.ssh("poweroff", timeout=10)
                time.sleep(8)
            if vm.alive():
                vm.p.kill()

    def ssh_up(self):
        return self.ssh("true", timeout=8)[0] == 0

    def checks(self, vm):
        self.soft = True
        s = self.step
        rc, out = self.ssh("rc-status -a 2>&1; echo ---; grep -iE 'zram|swap' /var/log/messages 2>/dev/null | tail -5; echo ---; rc-service zram-swap start 2>&1; swapon --show; ls -la /etc/init.d/zram-swap; ls /etc/runlevels/default /etc/runlevels/boot")
        open(f"{self.a.out}/screens/services-diagnostics.txt", "w").write(out)
        rc, out = self.ssh("findmnt -no FSTYPE,SOURCE,OPTIONS /; findmnt -no FSTYPE /boot; cat /proc/cmdline; "
                           "cryptsetup status hxroot | head -8; cryptsetup luksDump $(cryptsetup status hxroot | sed -n 's/ *device: *//p') | grep -E 'Version|PBKDF|Memory|cipher:' | head -8")
        flat = out.replace("\r", "")
        s("root-is-xfs-on-dm-crypt", re.search(r"^xfs /dev/mapper/hxroot", flat, re.M) is not None, flat.splitlines()[0] if flat else "")
        mo = flat.splitlines()[0] if flat else ""
        s("xfs-mount-options-default-no-discard", "discard" not in mo, mo)
        s("boot-is-ext4-unencrypted", flat.splitlines()[1].strip() == "ext4", "/boot ext4 on its own partition")
        s("luks2-argon2id", "argon2id" in flat and re.search(r"Version:\s+2", flat) is not None, re.sub(r"\s+", " ", " ".join(l for l in flat.splitlines() if "Memory" in l or "PBKDF" in l or "Version" in l)))
        mem = re.search(r"Memory:\s+(\d+)", flat)
        s("luks-memory-cost-within-cap", bool(mem) and int(mem.group(1)) <= 1048576, f"{mem.group(1)} KiB" if mem else "not found")
        s("lid-quirk-on-cmdline", "button.lid_init_state=open" in flat, "SP1/SP2 buggy _LID workaround from the hardware profile")
        s("no-serial-console-carried-over", "ttyS0" not in flat, "installed cmdline is VGA-only: the passphrase prompt is on tty0")
        rc, out = self.ssh("grep -c avx2 /proc/cpuinfo; grep -m1 -o -w -E 'avx2|fma|bmi1|bmi2' /proc/cpuinfo | head -3; grep -m1 'model name' /proc/cpuinfo; "
                           "dmesg | grep -ciE 'invalid opcode|trap invalid'; nproc")
        s("cpu-has-no-avx2-fma-bmi", out.splitlines()[0].strip() == "0" and out.count("fma") == 0 and "bmi" not in out, re.sub(r"\s+", " ", out)[:160])
        s("no-sigill-in-dmesg", out.strip().splitlines()[-2].strip() == "0", "no 'invalid opcode' traps while running the whole stack")
        rc, out = self.ssh("swapon --show --noheadings; echo ---; zramctl --noheadings; echo ---; sysctl -n vm.swappiness vm.page-cluster; echo ---; free -m | sed -n 2p; "
                           "blkid | grep -ci swap; rc-service zram-swap status")
        swaps = out.split("---")[0].split()
        devs = [w for w in swaps if w.startswith("/")]
        s("zram-is-the-only-swap", devs == ["/dev/zram0"] and "started" in out.split("---")[-1] and "[SWAP]" in out, re.sub(r"\s+", " ", out)[:200])
        rc, out = self.ssh("ls /etc/cron.weekly/fstrim && rc-service cronie status; fstrim -v / 2>&1; fstrim -v /boot 2>&1 | tail -1")
        s("fstrim-works-through-dm-crypt", re.search(r"^/: [\d.]+ \w+ \(\d+ bytes\) trimmed", out, re.M) is not None, re.sub(r"\s+", " ", out)[:200])
        rc, nm = self.ssh("rc-service NetworkManager status")
        rc, other = self.ssh("rc-service iwd status 2>&1; rc-service dhcpcd status 2>&1")
        s("networkmanager-started-iwd-dhcpcd-off", "started" in nm and "started" not in other, re.sub(r"\s+", " ", nm + other))
        rc, out = self.ssh("command -v nmtui nmcli wvkbd-mobintl sway foot swaybar surface-hidpi surface-status; "
                           "cat /etc/NetworkManager/conf.d/10-surface.conf | grep powersave")
        s("networkmanager-and-desktop-tools", all(x in out for x in ("nmtui", "wvkbd-mobintl", "/sway", "foot", "swaybar")) and "wifi.powersave = 2" in out, re.sub(r"\s+", " ", out)[:240])
        rc, out = self.ssh("rc-service horcrux-api status")
        s("horcrux-api-runs-on-ivybridge", "started" in out, "horcrux-api service started under -cpu IvyBridge")
        rc, out = self.ssh("lsinitrd /boot/initramfs-*.img 2>/dev/null | grep -oE 'cryptsetup|dm-crypt|hid-multitouch|hid-microsoft' | sort -u | tr '\\n' ' '; echo; grep -E 'CONFIG_(USB_HID|USB_XHCI_PCI|HID_GENERIC)=' /boot/config-* | sed 's/.*://' | tr '\\n' ' '")
        s("initramfs-contains-hid-and-crypt", all(x in out for x in ("cryptsetup", "dm-crypt", "hid-multitouch", "hid-microsoft", "CONFIG_USB_HID=y", "CONFIG_USB_XHCI_PCI=y")), re.sub(r"\s+", " ", out))
        # desktop session (autologin on tty1 -> sway)
        end, ok = time.time() + 240, False
        while time.time() < end:
            rc, out = self.ssh("pgrep -u " + USER + " -x sway; pgrep -u " + USER + " -x wvkbd-mobintl; pgrep -u " + USER + " -x swaybar")
            if len(out.split()) >= 3:
                ok = True
                break
            time.sleep(5)
        if not ok:
            rc, dg = self.ssh("id " + USER + "; loginctl list-sessions 2>&1 | head; rc-service elogind status 2>&1; ls -la /run/user /dev/dri 2>&1; "
                              "grep -n c1: /etc/inittab; ps -ef | grep -E 'agetty|login|sway' | grep -v grep; cat /home/" + USER + "/.local/state/sway.log 2>&1 | tail -15; "
                              "grep -c elogind /etc/pam.d/system-login")
            open(f"{self.a.out}/screens/desktop-diagnostics.txt", "w").write(dg)
        s("desktop-session-running-for-autologin-user", ok, f"sway + wvkbd + swaybar pids: {out.split()}")
        time.sleep(6)
        vm.shot("desktop")
        rc, out = self.user_sh("swaymsg -t get_outputs -r | python3 -c \"import json,sys; o=json.load(sys.stdin)[0]; print(o['name'], o['current_mode']['width'], o['current_mode']['height'], o['scale'])\"; cat ~/.local/state/sway.log | tail -3")
        s("sway-has-an-output", re.search(r"\w+ \d+ \d+ [\d.]+", out) is not None, out.strip()[:200])
        rc, out = self.user_sh("surface-osk show; sleep 2; pgrep -x wvkbd-mobintl")
        time.sleep(3)
        vm.shot("desktop-onscreen-keyboard")
        s("onscreen-keyboard-toggles", rc == 0 and out.strip().splitlines()[-1].strip().isdigit(), "wvkbd shown via surface-osk (screenshot desktop-onscreen-keyboard.png)")
        rc, out = self.user_sh("surface-osk hide; sleep 1; echo ok")
        rc, out = self.ssh("free -m | sed -n 2p; ps -eo rss,comm --sort=-rss | head -6; cat /proc/loadavg")
        open(f"{self.a.out}/screens/memory-at-desktop.txt", "w").write(out)
        s("memory-at-desktop-recorded", True, re.sub(r"\s+", " ", out.splitlines()[0]))
        rc, out = self.ssh("sshd -T 2>/dev/null | grep -Ei '^(permitrootlogin|passwordauthentication) '")
        s("ssh-root-key-only", "prohibit-password" in out.lower() or "without-password" in out.lower(), re.sub(r"\s+", " ", out))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--cpu", default="IvyBridge")
    ap.add_argument("--disk-gb", type=int, default=24)
    ap.add_argument("--keep-disks", action="store_true")
    a = ap.parse_args()
    try:
        t = T(a)
        t.run()
        if t.failed:
            print("FAILED STEPS:", ", ".join(t.failed))
            raise SystemExit(1)
        t.res["ok"] = True
        json.dump(t.res, open(f"{a.out}/result.json", "w"), indent=1)
        print("ALL PASS surface xfs-luks")
    finally:
        if not a.keep_disks:
            for n in ("target.qcow2", "answers.img", "OVMF_VARS.fd"):
                try:
                    os.unlink(os.path.join(a.out, n))
                except OSError:
                    pass


if __name__ == "__main__":
    main()
