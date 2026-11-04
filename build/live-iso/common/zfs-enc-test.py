#!/usr/bin/env python3
"""QEMU test: erase-disk install with ZFS NATIVE ENCRYPTION, passphrase at boot, boot environments and rollback.

  zfs-enc-test.py --iso X.iso --out DIR [--desktop] [--t2-fake]

Phases (UEFI, USB stick, unattended answer file with encryption=true):
  1. install (qemu-test.py's auto flow), disk is ENCRYPTED zfs: ESP + /boot ext4 + rpool
  2. boot #1 on the serial console: the initramfs asks for the passphrase, the test types it, SSH comes up
     asserts: encryption=aes-256-gcm keyformat=passphrase keylocation=prompt keystatus=available, ashift 12, zstd, autotrim,
     datasets (ROOT/horcrux home var/log var/lib/libvirt recordsize 64K), /boot + ESP unencrypted, zram swap (no ZFS swap),
     the passphrase is nowhere on the unencrypted partitions or in the logs, no Apple firmware on the public image,
     pre-upgrade snapshot hook, `horcrux-be snapshot` + change + `horcrux-be rollback`
  3. boot #2 with NO serial console and NO PS/2 keyboard (q35 i8042=off): the passphrase is typed on the emulated USB keyboard
     (QMP send-key) at the VGA prompt (screenshot saved), the system boots into the ROLLED-BACK boot environment: the change is gone.
  4. --desktop: GDM + GNOME Wayland session processes are running and the login screen is on the VGA console (screenshot).
Evidence in --out; exit 0 = all steps passed.
"""
import argparse, hashlib, importlib.util, json, os, re, subprocess, sys, time, types

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("qt", os.path.join(HERE, "qemu-test.py"))
qt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qt)

PASSPHRASE = "correct horse battery staple 2026"
SNI_TEST = "aW1wb3J0IGdpCmdpLnJlcXVpcmVfdmVyc2lvbigiR2lvIiwgIjIuMCIpCmZyb20gZ2kucmVwb3NpdG9yeSBpbXBvcnQgR2lvLCBHTGliClhNTCA9ICIiIjxub2RlPjxpbnRlcmZhY2UgbmFtZT0ib3JnLmtkZS5TdGF0dXNOb3RpZmllckl0ZW0iPgo8cHJvcGVydHkgbmFtZT0iQ2F0ZWdvcnkiIHR5cGU9InMiIGFjY2Vzcz0icmVhZCIvPjxwcm9wZXJ0eSBuYW1lPSJJZCIgdHlwZT0icyIgYWNjZXNzPSJyZWFkIi8+PHByb3BlcnR5IG5hbWU9IlRpdGxlIiB0eXBlPSJzIiBhY2Nlc3M9InJlYWQiLz4KPHByb3BlcnR5IG5hbWU9IlN0YXR1cyIgdHlwZT0icyIgYWNjZXNzPSJyZWFkIi8+PHByb3BlcnR5IG5hbWU9Ikljb25OYW1lIiB0eXBlPSJzIiBhY2Nlc3M9InJlYWQiLz48cHJvcGVydHkgbmFtZT0iTWVudSIgdHlwZT0ibyIgYWNjZXNzPSJyZWFkIi8+Cjxwcm9wZXJ0eSBuYW1lPSJJdGVtSXNNZW51IiB0eXBlPSJiIiBhY2Nlc3M9InJlYWQiLz48L2ludGVyZmFjZT48L25vZGU+IiIiClAgPSB7IkNhdGVnb3J5IjogKCJzIiwgIkFwcGxpY2F0aW9uU3RhdHVzIiksICJJZCI6ICgicyIsICJoeC10ZXN0IiksICJUaXRsZSI6ICgicyIsICJoeCB0cmF5IHRlc3QiKSwgIlN0YXR1cyI6ICgicyIsICJBY3RpdmUiKSwKICAgICAiSWNvbk5hbWUiOiAoInMiLCAibmV0d29yay13aXJlZCIpLCAiTWVudSI6ICgibyIsICIvTk9fREJVU01FTlUiKSwgIkl0ZW1Jc01lbnUiOiAoImIiLCBGYWxzZSl9Ck5BTUUgPSAib3JnLmtkZS5TdGF0dXNOb3RpZmllckl0ZW0tNDI0Mi0xIgpsb29wID0gR0xpYi5NYWluTG9vcCgpCnJlcyA9IHt9CmRlZiBnZXRwKGNvbm4sIHNlbmRlciwgcGF0aCwgaWZhY2UsIG5hbWUpOgogICAgdCwgdiA9IFBbbmFtZV0KICAgIHJldHVybiBHTGliLlZhcmlhbnQodCwgdikKZGVmIG9uX2J1cyhjb25uLCBuYW1lKToKICAgIGNvbm4ucmVnaXN0ZXJfb2JqZWN0KCIvU3RhdHVzTm90aWZpZXJJdGVtIiwgR2lvLkRCdXNOb2RlSW5mby5uZXdfZm9yX3htbChYTUwpLmludGVyZmFjZXNbMF0sIE5vbmUsIGdldHAsIE5vbmUpCmRlZiBvbl9uYW1lKGNvbm4sIG5hbWUpOgogICAgdHJ5OgogICAgICAgIGNvbm4uY2FsbF9zeW5jKCJvcmcua2RlLlN0YXR1c05vdGlmaWVyV2F0Y2hlciIsICIvU3RhdHVzTm90aWZpZXJXYXRjaGVyIiwgIm9yZy5rZGUuU3RhdHVzTm90aWZpZXJXYXRjaGVyIiwgIlJlZ2lzdGVyU3RhdHVzTm90aWZpZXJJdGVtIiwKICAgICAgICAgICAgICAgICAgICAgICBHTGliLlZhcmlhbnQoIihzKSIsIChOQU1FLCkpLCBOb25lLCBHaW8uREJ1c0NhbGxGbGFncy5OT05FLCA1MDAwLCBOb25lKQogICAgICAgIEdMaWIudGltZW91dF9hZGQoMTUwMCwgbGFtYmRhOiBjaGVjayhjb25uKSkKICAgIGV4Y2VwdCBFeGNlcHRpb24gYXMgZToKICAgICAgICByZXNbImVyciJdID0gc3RyKGUpCiAgICAgICAgbG9vcC5xdWl0KCkKZGVmIGNoZWNrKGNvbm4pOgogICAgZm9yIHAgaW4gKCJSZWdpc3RlcmVkU3RhdHVzTm90aWZpZXJJdGVtcyIsICJJc1N0YXR1c05vdGlmaWVySG9zdFJlZ2lzdGVyZWQiKToKICAgICAgICByID0gY29ubi5jYWxsX3N5bmMoIm9yZy5rZGUuU3RhdHVzTm90aWZpZXJXYXRjaGVyIiwgIi9TdGF0dXNOb3RpZmllcldhdGNoZXIiLCAib3JnLmZyZWVkZXNrdG9wLkRCdXMuUHJvcGVydGllcyIsICJHZXQiLAogICAgICAgICAgICAgICAgICAgICAgICAgICBHTGliLlZhcmlhbnQoIihzcykiLCAoIm9yZy5rZGUuU3RhdHVzTm90aWZpZXJXYXRjaGVyIiwgcCkpLCBOb25lLCBHaW8uREJ1c0NhbGxGbGFncy5OT05FLCA1MDAwLCBOb25lKQogICAgICAgIHJlc1twXSA9IHIudW5wYWNrKClbMF0KICAgIGxvb3AucXVpdCgpCkdpby5idXNfb3duX25hbWUoR2lvLkJ1c1R5cGUuU0VTU0lPTiwgTkFNRSwgR2lvLkJ1c05hbWVPd25lckZsYWdzLk5PTkUsIG9uX2J1cywgb25fbmFtZSwgbGFtYmRhICphOiAocmVzLnNldGRlZmF1bHQoImVyciIsICJuYW1lIGxvc3QiKSwgbG9vcC5xdWl0KCkpKQpHTGliLnRpbWVvdXRfYWRkKDE1MDAwLCBsb29wLnF1aXQpCmxvb3AucnVuKCkKcHJpbnQoIlNOSS1SRVNVTFQiLCByZXMpCg=="      # registers a StatusNotifierItem with the watcher the AppIndicator extension provides
# every installed .desktop file's TryExec/Exec binary must exist, or GNOME/XFCE hides the app (hogshead: brave-browser-stable was missing)
DESKTOP_EXEC_CHECK = "echo aW1wb3J0IGdsb2IsIG9zLCByZSwgc2hsZXgsIHNodXRpbCwgc3lzCm1pc3MgPSBbXQpmb3IgZiBpbiBzb3J0ZWQoZ2xvYi5nbG9iKCIvdXNyL3NoYXJlL2FwcGxpY2F0aW9ucy8qKi8qLmRlc2t0b3AiLCByZWN1cnNpdmU9VHJ1ZSkpOgogICAgdHJ5OgogICAgICAgIHR4dCA9IG9wZW4oZiwgZXJyb3JzPSJyZXBsYWNlIikucmVhZCgpLnNwbGl0KCJbRGVza3RvcCBBY3Rpb24iKVswXQogICAgZXhjZXB0IE9TRXJyb3I6CiAgICAgICAgY29udGludWUKICAgIGt2ID0gZGljdChsLnNwbGl0KCI9IiwgMSkgZm9yIGwgaW4gdHh0LnNwbGl0bGluZXMoKSBpZiAiPSIgaW4gbCBhbmQgbm90IGwuc3RhcnRzd2l0aCgiIyIpKQogICAgaWYga3YuZ2V0KCJOb0Rpc3BsYXkiLCAiIikubG93ZXIoKSA9PSAidHJ1ZSIgb3Iga3YuZ2V0KCJIaWRkZW4iLCAiIikubG93ZXIoKSA9PSAidHJ1ZSIgb3Iga3YuZ2V0KCJUeXBlIiwgIkFwcGxpY2F0aW9uIikgIT0gIkFwcGxpY2F0aW9uIjoKICAgICAgICBjb250aW51ZQogICAgZm9yIGtleSBpbiAoIlRyeUV4ZWMiLCAiRXhlYyIpOgogICAgICAgIHYgPSBrdi5nZXQoa2V5KQogICAgICAgIGlmIG5vdCB2OgogICAgICAgICAgICBjb250aW51ZQogICAgICAgIHRyeToKICAgICAgICAgICAgdCA9IHNobGV4LnNwbGl0KHYpCiAgICAgICAgZXhjZXB0IFZhbHVlRXJyb3I6CiAgICAgICAgICAgIHQgPSB2LnNwbGl0KCkKICAgICAgICB3aGlsZSB0IGFuZCAodFswXSA9PSAiZW52IiBvciAiPSIgaW4gdFswXSk6CiAgICAgICAgICAgIHQgPSB0WzE6XQogICAgICAgIGlmIHQgYW5kIHRbMF0gbm90IGluICgic2giLCAiYmFzaCIpIGFuZCBub3Qgc2h1dGlsLndoaWNoKHRbMF0pIGFuZCBub3QgKHRbMF0uc3RhcnRzd2l0aCgiLyIpIGFuZCBvcy5hY2Nlc3ModFswXSwgb3MuWF9PSykpOgogICAgICAgICAgICBtaXNzLmFwcGVuZChmIntvcy5wYXRoLmJhc2VuYW1lKGYpfTp7a2V5fT17dFswXX0iKQpwcmludCgiREVTS1RPUC1GSUxFUy1DSEVDS0VEIiwgbGVuKGdsb2IuZ2xvYigiL3Vzci9zaGFyZS9hcHBsaWNhdGlvbnMvKiovKi5kZXNrdG9wIiwgcmVjdXJzaXZlPVRydWUpKSkKcHJpbnQoIkRFU0tUT1AtRklMRVMtTUlTU0lORyIsICIgIi5qb2luKG1pc3MpIGlmIG1pc3MgZWxzZSAibm9uZSIpCg== | base64 -d | python3"
USER_PW = "Xv5#kP2-lima-48Qr"


class ZRun(qt.Runner):
    def __init__(self, a):
        ns = types.SimpleNamespace(iso=a.iso, project="horcrux", arch=qt.HOST_ARCH, mode="uefi", media="usb", flow="auto", fs="zfs",
                                   zfs_raid="single", ssh_password_opt_in=False, static_ip="", lan_ip="10.77.0.1", console_open=False,
                                   out=a.out, mem=a.mem, cpu=a.cpu, restarts=0, disk_gb=a.disk_gb)
        super().__init__(ns)
        self.z = a
        self.no_ps2 = False
        self.vga_only = False

    def qemu_args(self, iso, installed, nvars):
        args = super().qemu_args(iso, installed, nvars)
        if self.no_ps2 and not iso:
            i = args.index("-machine")
            args[i + 1] = args[i + 1] + ",i8042=off"
            args += ["-device", "usb-kbd,bus=xhci.0"]     # the ONLY keyboard: like the T2 Mac, whose keyboard is a USB device
        if (self.z.desktop or self.z.server) and getattr(self.z, "rtc_base", ""):
            # a MONTHS-OFF hardware clock on every boot, like gringotts' first real boot (RTC in July, installed in November): the installer must write a
            # good time to the RTC (checked in the install log) and the installed system must get its clock stepped by chrony once the network is up
            args += ["-rtc", f"base={self.z.rtc_base}"]
        if self.z.desktop and not iso:
            args += ["-audiodev", "none,id=hxaudio", "-device", "intel-hda", "-device", "hda-duplex,audiodev=hxaudio"]   # a real ALSA card: wpctl/gsd-sound/volume keys need a sink
        return args

    def step(self, name, ok, info=""):
        """record + print; a failed step does NOT stop the run once the installed system is up (all findings in one pass)"""
        self.res["steps"][name] = {"ok": bool(ok), "info": info}
        print(f"[{'PASS' if ok else 'FAIL'}] {name} {info}", flush=True)
        json.dump(self.res, open(f"{self.a.out}/result.json", "w"), indent=1)
        if not ok:
            self.failed = getattr(self, "failed", []) + [name]
            if name in ("answer-file-built", "installer-finished", "boot1-passphrase-prompt", "boot1-ssh-after-passphrase", "vm-stopped-after-install"):
                sys.exit(1)

    # --- answers
    def toml(self, pubkey, pwhash):
        t = ["[global]", 'keyboard = "us"', 'timezone = "UTC"', 'hostname = "zfs-enc.test.lan"', 'email = "ops@example.test"',
             f'root_password_hash = "{pwhash}"', f'ssh_keys = ["{pubkey}"]', "ssh_password_auth = false", f't2 = "{self.z.t2}"']
        if self.z.desktop:
            t.append('install_type = "desktop"')
        t += ["", "[disk]", 'filesystem = "zfs"', 'zfs_raid = "single"', 'disks = ["/dev/vda"]', "encryption = true"]
        if self.z.hibernate:
            t.append('swap = "hibernate"')
        t += [
              f'passphrase = "{PASSPHRASE}"', "", "[network.management]", 'mode = "dhcp"', ""]
        if self.z.desktop:
            uh = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemuuser1", USER_PW]).decode().strip()
            t += ["[user]", 'name = "owner"', f'password_hash = "{uh}"', ""]
        return "\n".join(t)

    def run(self):
        a = self.z
        os.makedirs(a.out, exist_ok=True)
        self.disk, self.disk2, self.answers = f"{a.out}/target.qcow2", f"{a.out}/target2.qcow2", f"{a.out}/answers.img"
        nvars = f"{a.out}/OVMF_VARS.fd"
        subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", self.disk, f"{a.disk_gb}G"])
        qt.pflash_vars(nvars)
        self.key = f"{a.out}/test_key"
        for f in (self.key, self.key + ".pub"):
            if os.path.exists(f):
                os.unlink(f)
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "qemu-test", "-f", self.key])
        self.pub = open(self.key + ".pub").read().strip()
        pwhash = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemutest1", qt.PW]).decode().strip()
        qt.make_answer_image(self.answers, "horcrux", self.toml(self.pub, pwhash))
        self.step("answer-file-built", True, "encryption=true, passphrase typed by the harness only")
        if a.fake_t2 and a.expect_t2_refusal:
            return self.t2_guard(nvars)
        self.phase1(nvars)
        self.boot1(nvars)
        if a.server:
            self.boot2_server(nvars)      # reboot into the installed system on the serial console (no VGA / USB keyboard)
        else:
            self.boot2(nvars)
        if a.hibernate:
            self.hibernate_phase(nvars)
        self.res["ok"] = not getattr(self, "failed", [])
        json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
        if getattr(self, "failed", []):
            print("FAILED STEPS:", ", ".join(self.failed))
            sys.exit(1)
        print("ALL PASS zfs-encrypted", "desktop" if a.desktop else "server")

    def t2_guard(self, nvars):
        """public image has no T2 kernel: on a (fake) T2 Mac the installer must refuse an ENCRYPTED install instead of
        producing a system whose passphrase prompt has no keyboard"""
        a = self.z
        vm = qt.VM("1-t2guard", self.qemu_args(a.iso, False, nvars), a.out)
        try:
            self.step("grub-menu-reached", vm.expect_raw(r"Serial console \(ttyS0", 120))
            time.sleep(2)
            vm.send("s")                       # boot menu hotkeys (common/boot/gen-grub-cfg.sh): serial-console submenu, 4 = automated
            time.sleep(1.5)
            vm.send("4")
            self.step("live-userland-up", vm.expect_raw(r"AUTOINSTALL-START", 420), "live system booted, answer file found")
            m = vm.expect_raw(r"AUTOINSTALL-(OK|FAILED[^\n]*)", 900, consume=False)
            txt = m.group(0).decode(errors="replace") if m else "timeout"
            self.step("t2-detected-and-install-refused-without-t2-kernel", m and b"FAILED" in m.group(0) and b"T2 kernel" in m.group(0), txt[:200])
            seen = vm.expect_raw(r"Apple T2 Mac detected[^\n]*", 5, consume=False)
            self.step("t2-profile-announced", seen, seen.group(0).decode(errors="replace") if seen else "no announcement")
        finally:
            if vm.alive():
                vm.p.kill()
        self.res["ok"] = not getattr(self, "failed", [])
        json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
        if getattr(self, "failed", []):
            sys.exit(1)
        print("ALL PASS t2 guard")

    # --- helpers
    def wait_ssh(self, timeout=420):
        end = time.time() + qt.T(timeout)
        while time.time() < end:
            if qt.ssh_banner(qt.FWD_SSH).startswith("SSH-"):
                time.sleep(3)
                rc, o = qt.ssh(qt.FWD_SSH, self.key, "echo up")
                if rc == 0 and "up" in o:
                    return True
            time.sleep(4)
        return False

    def sh(self, cmd, timeout=60):
        return qt.ssh(qt.FWD_SSH, self.key, cmd, timeout=timeout)

    def vga_stable(self, vm, label, timeout=360):
        """wait until the VGA console shows something and then stops changing (= the passphrase prompt)"""
        end = time.time() + qt.T(timeout)
        prev, still, seen_text = None, 0, False
        while time.time() < end:
            ppm = vm.grab()
            if ppm:
                m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", ppm)
                px = ppm[m.end():] if m else b""
                nonblack = sum(1 for i in range(0, len(px), 97) if px[i] > 40)
                seen_text = seen_text or nonblack > 40
                if prev is not None and len(px) == len(prev):
                    diff = sum(1 for i in range(0, len(px), 97) if px[i] != prev[i])
                    still = still + 1 if (seen_text and diff < 6) else 0
                prev = px
                if still >= 4:
                    vm.shot(label)
                    return True
            time.sleep(2.5)
        vm.shot(label + "-timeout")
        return False

    # --- boot 1: serial passphrase
    def phase1(self, nvars):
        super().phase1(nvars)
        if (self.z.desktop or self.z.server) and getattr(self.z, "rtc_base", ""):
            self.install_rtc_check(f"{self.z.out}/1-install-serial.log")

    def boot1(self, nvars):
        a = self.z
        qt.pflash_vars(nvars)
        vm = qt.VM("2-boot1", self.qemu_args(None, True, nvars), a.out)
        try:
            m = vm.expect_raw(r"(?i)(encrypted zfs password|enter passphrase|password for rpool|disk passphrase)[^\n]*", 420)
            self.step("boot1-passphrase-prompt", m, m.group(0).decode(errors="replace").strip() if m else "no prompt (system booted without asking?)")
            time.sleep(1)
            vm.send(PASSPHRASE + "\r")
            self.step("boot1-ssh-after-passphrase", self.wait_ssh(), "system came up after the passphrase")
            self.checks()
            self.change_and_rollback()
            self.sh("sync; reboot", timeout=15)
            t = time.time()
            while vm.alive() and time.time() - t < 90:
                time.sleep(1)
        finally:
            if vm.alive():
                vm.p.kill()

    def checks(self):
        a = self.z
        rc, o = self.sh("zfs get -H -o property,value encryption,keyformat,keylocation,keystatus,compression rpool; "
                        "zpool get -H -o property,value ashift,autotrim,bootfs rpool")
        self.res["zfs_props"] = o
        want = ["encryption\taes-256-gcm", "keyformat\tpassphrase", "keylocation\tprompt", "keystatus\tavailable", "compression\tzstd",
                "ashift\t12", "autotrim\ton", "bootfs\trpool/ROOT/horcrux"]
        miss = [w for w in want if w.replace("\t", " ") not in o.replace("\t", " ")]
        self.step("zfs-native-encryption-and-pool-props", rc == 0 and not miss, f"missing: {miss}" if miss else "aes-256-gcm/passphrase/prompt/zstd/ashift 12/autotrim")
        rc, o = self.sh("zfs list -H -o name,mountpoint,recordsize,encryptionroot -r rpool")
        names = [l.split("\t")[0] for l in o.splitlines()]
        need = ["rpool/ROOT/horcrux", "rpool/home", "rpool/var/log", "rpool/var/lib/libvirt"]
        self.step("datasets", all(n in names for n in need) and "rpool/var/lib/libvirt\t/var/lib/libvirt\t64K" in o, ", ".join(names))
        rc, dbg = self.sh("cat /etc/fstab; echo ---; findmnt -o TARGET,SOURCE,FSTYPE | head -30; echo ---; rc-status -a 2>&1 | head -40; dmesg | grep -iE 'fstab|zfs|mount' | tail -10")
        open(f"{a.out}/debug-boot1.txt", "w").write(dbg)
        rc, o = self.sh("findmnt -no SOURCE /; findmnt -no FSTYPE /boot; findmnt -no FSTYPE /boot/efi; cat /proc/cmdline")
        self.step("root-on-zfs-boot-environment", "rpool/ROOT/horcrux" in o and "ext4" in o and "vfat" in o, o.replace("\n", " | ")[:200])
        rc, o = self.sh("swapon --noheadings --show=NAME,TYPE; echo ---; zfs list -H -t volume; true")
        self.step("no-swap-on-zfs", "zd" not in o.split("---")[0] and "/dev/zvol" not in o, f"swap: {o.split('---')[0].strip() or 'none (zram-init not in image)'}")
        rc, o = self.sh("ls /lib/firmware/brcm 2>/dev/null | grep -ci apple; true")
        self.step("no-apple-firmware-on-the-public-image", o.strip().splitlines()[-1] == "0", f"apple files under /lib/firmware/brcm: {o.strip()}")
        qt.no_autologin_config_gate(self.sh, self.step)             # OWNER RULE: no display-manager autologin in the installed configs (GDM/LightDM/SDDM/agetty)
        qt.shadow_reuse_gate(self.sh, self.step, PASSPHRASE)       # SECURITY: no account (user, root, ...) may have the disk passphrase as its password
        rc, o = self.sh(f"grep -rIlF -- '{PASSPHRASE}' /etc /boot /var/log /root /var/lib/horcrux /run 2>/dev/null | head; echo done")
        self.step("passphrase-not-stored-anywhere", o.strip() == "done", f"hits: {o.strip()}")
        rc, o = self.sh("zfs get -H -o value keylocation rpool; test -e /run/horcrux-zfs.key && echo KEYFILE || echo nokeyfile")
        self.step("key-file-removed", "nokeyfile" in o and "prompt" in o, o.strip().replace("\n", " "))
        rc, o = self.sh("ssh_ok=1; horcrux-be list")
        self.step("horcrux-be-list", rc == 0 and "horcrux" in o, o.strip().replace("\n", " | "))
        rc, o = self.sh("HOME=/root; ( . /etc/portage/bashrc 2>/dev/null; EBUILD_PHASE=setup; horcrux-be auto-snapshot ); zfs list -H -t snapshot -o name | grep pre-upgrade")
        self.step("pre-upgrade-snapshot-hook", "ROOT/horcrux@pre-upgrade-" in o, o.strip().replace("\n", " "))
        rc, o = self.sh("/etc/kernel/postinst.d/90-horcrux-be && ls -l /boot/be/horcrux/ && grep -c 'be-horcrux' /boot/grub/grub.cfg")
        self.step("kernel-update-hook-refreshes-boot-environment", rc == 0 and "vmlinuz" in o and "initramfs.img" in o, o.strip().replace("\n", " | ")[:200])
        # owner rule (hogshead): everything on the installed system belongs to a Portage package (horcrux-orphans = qfile -o made
        # merged-/usr safe). Allowed: files generated by postinst/eselect/installkernel and the installer's data dir.
        rc, o = self.sh("horcrux-orphans > /tmp/orph.txt; wc -l < /tmp/orph.txt; while read f; do [ -L \"$f\" ] && echo \"LINK $f\" || echo \"FILE $f\"; done < /tmp/orph.txt", timeout=900)
        allow = re.compile(r"^(/usr/share/mime/|/usr/share/info/dir$|/usr/share/(binutils|gcc)-data/.*/info/dir$|/usr/share/applications/mimeinfo\.cache$|"
                           r"/usr/share/icons/[^/]+/icon-theme\.cache$|/usr/lib/locale/locale-archive$|/usr/lib/cracklib_dict\.|/usr/lib64/lib(freebl3|softokn3|nssdbm3)\.chk$|"
                           r"/usr/lib64/gconv/gconv-modules\.cache$|/usr/lib64/vlc/plugins/plugins\.dat$|/boot/(vmlinuz|System\.map|config|initramfs)-|/usr/libexec/rc/console/keymap$|/boot/(be|grub|efi)/|/usr/lib(64)?/(gio/modules|gdk-pixbuf-[^/]*|gtk-[34]\.0)/.*cache)")
        lines = [l.split(" ", 1) for l in o.splitlines() if l.startswith(("FILE ", "LINK "))]
        bad = [p for k, p in lines if k == "FILE" and not allow.match(p)]     # symlinks are eselect/gcc-config/perl-cleaner products: reported, allowed
        open(f"{a.out}/orphans.txt", "w").write(o)
        self.step("portage-owns-every-file", rc == 0 and not bad, f"{len(lines)} generated/eselect paths allowed, {len(bad)} unowned: {bad[:25]}")
        if a.server:
            self.copy_local_overlay()
        rc, o = self.sh("emerge -puDN @world 2>&1 | tail -25; echo RC=${PIPESTATUS[0]}", timeout=600)
        if "profile is invalid" in o:
            # a first-boot service (gringotts-firstboot) is still fetching the Portage trees: wait for it, then resolve again
            end = time.time() + qt.T(1200)
            while time.time() < end:
                rc, t = self.sh("test -e /var/db/repos/gentoo/profiles/repo_name && ! pgrep -f 'emaint|emerge-webrsync|git clone' >/dev/null && echo TREE-READY", timeout=60)
                if "TREE-READY" in t:
                    break
                time.sleep(20)
            rc, o = self.sh("emerge -puDN @world 2>&1 | tail -25; echo RC=${PIPESTATUS[0]}", timeout=600)
        if "profile is invalid" in o:
            # the public images ship NO Portage tree (setup-rootfs.sh empties /var/db/repos/gentoo): a user syncs first, so the gate does the same
            rc, t = self.sh("emerge-webrsync -q 2>&1 | tail -5; echo RC=${PIPESTATUS[0]}", timeout=1500)
            self.step("portage-tree-syncs-on-the-installed-system", "RC=0" in t, t.strip().replace("\n", " | ")[-300:])
            rc, o = self.sh("emerge -puDN @world 2>&1 | tail -25; echo RC=${PIPESTATUS[0]}", timeout=600)
        open(f"{a.out}/world-resolve.txt", "w").write(o)
        self.step("emerge-world-resolves-no-blockers", "RC=0" in o and "blocked" not in o.lower(), o.strip().replace("\n", " | ")[-300:])
        rc, o = self.sh("mkdir -p /mnt/under; mount --bind / /mnt/under; ls -A /mnt/under/home | wc -l; umount /mnt/under; zfs get -H -o value mounted rpool/home")
        self.step("nothing-hidden-under-the-home-dataset", o.strip().splitlines()[-2:] == ["0", "yes"], "files in the root BE's /home beneath the mounted home dataset: " + o.strip().replace("\n", " "))
        if a.desktop:
            self.desktop_checks()
        elif a.server:
            self.server_checks(first_boot=True)

    def change_and_rollback(self):
        if self.z.server:
            # headless server: the rolled-back BE must keep its serial console (the reboot is driven over the serial line); the desktop run proves a BE switch with
            # a changed kernel command line (console=ttyS0 dropped) instead
            rc, o = self.sh("horcrux-be snapshot before-change && echo CHANGE > /root/marker-after-snapshot && "
                            "horcrux-be rollback horcrux@before-change --name rolledback && horcrux-be list && zpool get -H -o value bootfs rpool && "
                            "grep -c 'be-rolledback' /boot/grub/grub.cfg && grep -q ttyS0 /boot/grub/grub.cfg")
            self.step("rollback-boot-environment-created", rc == 0 and "rpool/ROOT/rolledback" in o, o.strip().replace("\n", " | ")[-300:])
            self.sh("cat /boot/grub/grub.cfg > /root/grub.cfg.copy")
            return
        rc, o = self.sh("horcrux-be snapshot before-change && echo CHANGE > /root/marker-after-snapshot && "
                        "sed -i 's/console=ttyS0,115200n8//' /etc/horcrux/be.conf && grep ^CMDLINE /etc/horcrux/be.conf && "
                        "horcrux-be rollback horcrux@before-change --name rolledback && horcrux-be list && zpool get -H -o value bootfs rpool && "
                        "grep -c 'be-rolledback' /boot/grub/grub.cfg && ! grep -q ttyS0 /boot/grub/grub.cfg")
        self.step("rollback-boot-environment-created", rc == 0 and "rpool/ROOT/rolledback" in o, o.strip().replace("\n", " | ")[-300:])
        self.sh("cat /boot/grub/grub.cfg > /root/grub.cfg.copy")

    def wait_exit(self, vm, secs=120):
        """let the guest power off by itself (the next phase boots the same qcow2: QEMU's write lock must be gone)"""
        t = time.time()
        while vm.alive() and time.time() - t < qt.T(secs):
            time.sleep(1)
        if vm.alive():
            vm.p.kill()
        try:
            vm.p.wait(timeout=20)
        except Exception:
            pass
        time.sleep(2)

    # --- boot 2: USB keyboard, VGA only, rolled-back BE
    def boot2(self, nvars):
        a = self.z
        self.no_ps2 = True
        qt.pflash_vars(nvars)
        vm = qt.VM("3-boot2", self.qemu_args(None, True, nvars), a.out)
        kb = qt.Kbd(vm, True)
        try:
            ok = self.vga_stable(vm, "passphrase-prompt-vga")
            self.step("boot2-vga-prompt-screen", ok, "VGA console settled at the passphrase prompt (see 3-boot2-passphrase-prompt-vga.png); no serial console, PS/2 off")
            up = self.unlock_usb(vm, kb, "boot2")
            self.step("boot2-ssh-after-usb-keyboard-passphrase", up, "passphrase typed on the USB keyboard")
            rc, o = self.sh("zpool get -H -o value bootfs rpool; findmnt -no SOURCE /; test -e /root/marker-after-snapshot && echo MARKER || echo NOMARKER; "
                            "horcrux-be list; dmesg | grep -ci 'i8042' ")
            self.step("rolled-back-to-previous-boot-environment", "rpool/ROOT/rolledback" in o and "NOMARKER" in o, o.strip().replace("\n", " | ")[:300])
            rc, o = self.sh("zfs get -H -o value keystatus rpool; lsblk -no FSTYPE,MOUNTPOINT | head -20")
            if a.desktop:
                self.desktop_checks(vm)
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()


    # ======================================================================== HEADLESS SERVER GATE (--server)
    # Horcrux is a server-first OS: the server image must install and run with NO display at all (serial console + ssh, answer file), and carry the same
    # regression gates as the desktop. Everything below runs over ssh/serial; nothing needs VGA, a USB keyboard or a GPU, so it works under x86_64 KVM and
    # aarch64 TCG alike.
    # display-stack packages that must not be on a server (the live-only graphical installer stack is removed by the installer; a few non-GUI LIBRARIES live in
    # x11-libs/x11-misc because Portage categorises them there: libpciaccess + pixman are qemu/libvirt dependencies, shared-mime-info is data)
    DISPLAY_RE = (r"^(gui-libs|gui-wm|x11-base|x11-libs|x11-misc|x11-apps|x11-drivers|x11-themes|x11-terms|gnome-base|gnome-extra|kde-[a-z]+|xfce-[a-z]+|dev-qt|"
                  r"media-libs/(mesa|libglvnd|libepoxy)|x11-wm|www-client|media-video/(pipewire|wireplumber)|sys-auth/(seatd|elogind)|net-misc/networkmanager)/")
    DISPLAY_ALLOW = ("x11-libs/libpciaccess", "x11-libs/pixman", "x11-misc/shared-mime-info", "x11-apps/bdftopcf")   # bdftopcf: font compiler, build dependency of media-fonts/terminus-font (the console font)
    CORE_PKGS = ("sys-apps/horcrux-core", "net-misc/horcrux-time", "net-misc/horcrux-chrony-nm", "sys-apps/horcrux-firstboot", "sys-apps/horcrux-backup", "net-misc/chrony")

    def public_image_checks(self):
        """a PUBLIC image carries nothing personal: project/public-forbidden.txt lists the owner's own picks and machine-specific apps (Steam is for one machine only)"""
        try:
            forb = [l.strip() for l in open(os.path.join(HERE, "..", "project", "public-forbidden.txt")) if l.strip() and not l.startswith("#")]
        except OSError as e:
            self.step("public-image-has-no-personal-or-machine-packages", False, f"cannot read public-forbidden.txt: {e}")
            return
        rc, o = self.sh("for p in " + " ".join(forb) + "; do qlist -I \"$p\"; done; echo CHECKED")
        hit = [l for l in o.splitlines() if "/" in l]
        self.step("public-image-has-no-personal-or-machine-packages", "CHECKED" in o and not hit, f"{len(forb)} forbidden atoms checked; installed: {hit or 'none'}")

    def headless_checks(self):
        rc, o = self.sh("qlist -I | sort")
        pk = [l.strip() for l in o.splitlines() if "/" in l]
        gui = [x for x in pk if re.match(self.DISPLAY_RE, x + "/") and x not in self.DISPLAY_ALLOW]
        open(f"{self.z.out}/installed-packages.txt", "w").write("\n".join(pk) + "\n")
        self.step("server-has-no-display-stack-installed", rc == 0 and len(pk) > 100 and not gui,
                  f"{len(pk)} packages installed; gui-libs/gui-wm/x11-*/gnome-*/mesa/qt/pipewire/NetworkManager found: {gui[:20] or 'none'} (allowed library deps: {', '.join(self.DISPLAY_ALLOW)})")
        rc, o = self.sh("(pgrep -x 'Xorg|Xwayland|gdm|lightdm|sddm|gnome-shell|cage|sway|weston' || true) | wc -l; (rc-status -a 2>&1 | grep -Eci 'gdm|lightdm|sddm|display-manager|xdm' || true)")
        self.step("server-runs-no-x-wayland-or-display-manager", o.split()[-2:] == ["0", "0"], f"display processes / display-manager services: {' '.join(o.split()[-2:])}")
        rc, o = self.sh("grep -o 'console=tty[A-Za-z0-9,]*' /proc/cmdline | tr '\\n' ' '; echo; pgrep -af 'agetty|console-menu|live-serial' | head -4")
        self.step("serial-console-is-the-system-console", "console=tty" in o, re.sub(r"\s+", " ", o).strip()[:200])

    def copy_local_overlay(self):
        """(--overlay-dir) copy the branch's core packages over the synced /var/db/repos/horcrux, once the first-boot sync is DONE (it would overwrite them);
        used when the branch is not on main yet, i.e. the published overlay (what an installed system really syncs) lacks the new packages"""
        if not getattr(self.z, "overlay_dir", "") or getattr(self, "_overlay_copied", False):
            return
        self.firstboot_check()
        self._overlay_copied = True
        if True:
            import base64, io, tarfile
            buf = io.BytesIO()
            with tarfile.open(fileobj=buf, mode="w:gz") as tf:
                for rel in ("sys-apps/horcrux-core", "sys-apps/horcrux-firstboot", "sys-apps/horcrux-backup", "sys-apps/horcrux-repo-config", "net-misc/horcrux-time",
                            "net-misc/horcrux-chrony-nm", "profiles", "metadata", "gnome-extra/horcrux-gnome-defaults"):
                    full = os.path.join(self.z.overlay_dir, rel)
                    if os.path.exists(full):
                        tf.add(full, arcname=rel)
            b64 = base64.b64encode(buf.getvalue()).decode()
            rc, t = self.sh(f"echo {b64} | base64 -d | tar xz -C /var/db/repos/horcrux && echo OVERLAY-COPIED", timeout=120)
            self.step("local-overlay-copied-for-the-world-check", "OVERLAY-COPIED" in t, f"{len(b64)} bytes of the branch's core packages over the synced overlay (branch not published yet)")

    def world_headless_check(self):
        """`emerge -pv @world` on the installed server: a world update must not pull a single display package (the Portage tree is fetched by horcrux-firstboot)"""
        end = time.time() + qt.T(1500)
        o = ""
        while time.time() < end:
            rc, o = self.sh("emerge -pvuDN @world 2>&1 | tail -80; echo RC=${PIPESTATUS[0]}", timeout=900)
            if "profile is invalid" not in o:
                break
            time.sleep(25)       # horcrux-firstboot is still fetching the tree
        open(f"{self.z.out}/server-world-resolve.txt", "w").write(o)
        merges = [l for l in o.splitlines() if l.startswith(("[ebuild", "[binary"))]
        gui = [l for l in merges if re.search(r"\] (gui-libs|gui-wm|gnome-base|gnome-extra|x11-base|xfce-[a-z]+|kde-[a-z]+|dev-qt)/|x11-libs/(gtk|cairo|pango|libX)|media-libs/mesa", l)]
        self.step("server-world-update-pulls-no-display-stack", "RC=0" in o and "blocked" not in o.lower() and not gui,
                  f"{len(merges)} packages in the update list; display packages in it: {gui[:6] or 'none'}; " + re.sub(r"\s+", " ", o)[-160:])

    def core_checks(self):
        rc, o = self.sh("for p in " + " ".join(self.CORE_PKGS) + "; do qlist -I $p || echo MISSING:$p; done; cat /usr/share/horcrux/core-release; horcrux-time-config check; "
                        "qfile -q /usr/sbin/horcrux-time-config /usr/libexec/horcrux-timesync /usr/libexec/horcrux-firstboot /etc/init.d/horcrux-timesync /etc/init.d/horcrux-firstboot "
                        "/etc/portage/package.use/horcrux-core /usr/sbin/ctg-restic && echo OWNED; "
                        "rc-update show default | grep -E 'chronyd|horcrux-timesync|horcrux-firstboot|cronie|horcrux-api|sshd' | tr -s ' ' | tr '\\n' ';'")
        self.step("core-packages-installed-package-owned-and-enabled", "MISSING" not in o and "OWNED" in o and "OK horcrux time config" in o and "horcrux-core" in o
                  and all(x in o for x in ("chronyd", "horcrux-timesync", "horcrux-firstboot", "horcrux-api")), re.sub(r"\s+", " ", o).strip()[:400])
        rc, o = self.sh("rc-status -a 2>&1 | grep -Ei 'crashed|stopped.*(chronyd|horcrux)'; echo END; rc-status -s | grep -E 'chronyd|horcrux-timesync|horcrux-api|sshd' | tr -s ' '")
        self.step("core-services-running-none-crashed", o.split("END")[0].strip() == "" and "chronyd" in o and "horcrux-timesync" in o and "horcrux-api" in o, re.sub(r"\s+", " ", o).strip()[:300])
        rc, o = self.sh("/usr/sbin/ctg-restic-cron backup; echo BACKUP-RC=$?; ctg-restic content; echo CONTENT-RC=$?; ls /etc/cron.d/horcrux-backup; pgrep -x crond >/dev/null && echo CRONDUP; "
                        "grep -c canutethegreat /etc/ctg-backup/paths /etc/ctg-backup/excludes; ls /etc/ctg-backup/required 2>&1 | head -1")
        self.step("backup-in-core-generic-not-provisioned-no-owner-paths", "BACKUP-RC=0" in o and "CONTENT-RC=0" in o and "/etc/cron.d/horcrux-backup" in o and "CRONDUP" in o
                  and "paths:0" in o and "excludes:0" in o and "No such file" in o, re.sub(r"\s+", " ", o).strip()[:300])
        rc, o = self.sh("grep -x 'sys-fs/zfs-kmod dist-kernel' /etc/portage/package.use/horcrux-core && grep -qw dist-kernel /var/db/pkg/sys-fs/zfs-kmod-*/USE && echo KMOD-DIST-KERNEL-OK; "
                        "grep -x 'sys-fs/zfs dist-kernel' /etc/portage/package.use/horcrux-core | head -1; grep -qw dist-kernel /var/db/pkg/sys-fs/zfs-[0-9]*/USE && echo ZFS-DIST-KERNEL-OK")
        self.step("zfs-dist-kernel-rule-owned-by-core-and-active", "KMOD-DIST-KERNEL-OK" in o and "ZFS-DIST-KERNEL-OK" in o, re.sub(r"\s+", " ", o).strip()[:300])

    def web_checks(self):
        sc, code, body = qt.http(qt.FWD_WEB, "/")
        rc, o = self.sh("curl -sk -o /dev/null -w '%{http_code}' https://127.0.0.1:8006/; echo; ss -ltn | grep -c ':8006 '")
        self.step("server-web-ui-reachable-from-the-network", code in (200, 301, 302, 303, 401) and re.match(r"\s*(200|30\d|401)", o) is not None, f"host->guest forward: {sc} {code}; in guest: {re.sub(chr(10), ' ', o).strip()}")
        sc, code, body = qt.http(qt.FWD_WEB, self.P["health"][0])
        self.step("server-web-api-health", code == 200, f"{self.P['health'][0]} -> {code}")

    def firstboot_check(self):
        if getattr(self, "_firstboot_checked", False):
            return
        self._firstboot_checked = True
        end = time.time() + qt.T(1500)
        o = ""
        while time.time() < end:
            rc, o = self.sh("ls /var/lib/horcrux/ 2>&1; grep -E '^== (horcrux-firstboot|waiting|network is up|hook|portage trees|done|tree sync)' /var/log/horcrux-firstboot.log 2>&1; rc-status -a 2>&1 | grep horcrux-firstboot; test -s /run/horcrux-firstboot.pid && echo PIDLEFT || echo PIDGONE", timeout=60)
            if "firstboot-done" in o:
                break
            time.sleep(20)
        open(f"{self.z.out}/firstboot.log", "w").write(o)
        self.step("firstboot-waits-for-network-syncs-trees-not-crashed", "== horcrux-firstboot" in o and "firstboot-done" in o and "== done" in o and "crashed" not in o and "PIDGONE" in o,
                  re.sub(r"\s+", " ", o).strip()[-300:])

    def server_checks(self, first_boot=True):
        """the server gate: every desktop regression gate that does not need a display, plus the headless ones"""
        a = self.z
        self.headless_checks()
        if getattr(a, "public", False):
            self.public_image_checks()
        self.core_checks()
        self.web_checks()
        if getattr(a, "rtc_base", ""):
            self.clock_checks()                 # months-off RTC on every boot: chrony steps it after the network is up (dhcpcd/static, no NetworkManager)
        self.toolchain_check()
        self.package_contents_check()
        self.kmod_build_check()
        if first_boot:
            self.firstboot_check()
            self.world_headless_check()

    def boot2_server(self, nvars):
        """REBOOT into the installed system, serial console only: passphrase on the serial line, rolled-back boot environment, then the server gates again
        (with --late-network N the NIC link is DOWN while the system boots and comes up N seconds later)"""
        a = self.z
        qt.pflash_vars(nvars)
        vm = qt.VM("3-boot2-server", self.qemu_args(None, True, nvars), a.out)
        try:
            if a.late_network:
                time.sleep(1)
                vm.qmp("human-monitor-command", **{"command-line": "set_link n0 off"})
                self.step("late-network-link-down-from-the-start", True, f"set_link n0 off right after QEMU start; up again in {a.late_network} s (wall clock)")
            m = vm.expect_raw(r"(?i)(encrypted zfs password|enter passphrase|password for rpool|disk passphrase)[^\n]*", 420)
            self.step("boot2-passphrase-prompt-on-serial", m, m.group(0).decode(errors="replace").strip() if m else "no prompt")
            time.sleep(1)
            vm.send(PASSPHRASE + "\r")
            if a.late_network:
                time.sleep(a.late_network)
                vm.qmp("human-monitor-command", **{"command-line": "set_link n0 on"})
            up = self.wait_ssh(timeout=600 if a.late_network else 420)
            self.step("boot2-ssh-after-serial-passphrase-reboot", up, "installed system rebooted and reachable")
            if not up:
                return
            rc, o = self.sh("zpool get -H -o value bootfs rpool; findmnt -no SOURCE /; test -e /root/marker-after-snapshot && echo MARKER || echo NOMARKER")
            self.step("rolled-back-to-previous-boot-environment", "rpool/ROOT/rolledback" in o and "NOMARKER" in o, o.strip().replace("\n", " | ")[:300])
            self.server_checks(first_boot=False)
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()

    # --- boot 3 (--hibernate): ONE passphrase, hibernate, power off, boot, ONE passphrase, resume
    def unlock_usb(self, vm, kb, label):
        """type the passphrase on the USB keyboard; on a loaded host the screen can look settled while GRUB/the kernel is still
        loading and the keys are lost: type again (a wrong/missing passphrase just re-prompts) before giving up"""
        for attempt in (1, 2, 3):
            kb.text(PASSPHRASE, pause=0.05)
            kb.key("ENTER")
            if self.wait_ssh(timeout=150 if attempt < 3 else 420):
                if attempt > 1:
                    print(f"  ({label}: passphrase had to be typed {attempt} times: keys sent before the prompt exists are lost)", flush=True)
                return True
            vm.shot(f"{label}-retype-{attempt}")
        return False

    def type_passphrase_usb(self, vm, label):
        ok = self.vga_stable(vm, label)
        kb = qt.Kbd(vm, True)
        up = self.unlock_usb(vm, kb, label)
        return ok, up

    def hibernate_phase(self, nvars):
        a = self.z
        self.no_ps2 = True
        qt.pflash_vars(nvars)
        vm = qt.VM("4-boot3-hibernate", self.qemu_args(None, True, nvars), a.out)
        try:
            _ok, up = self.type_passphrase_usb(vm, "passphrase-prompt-before-hibernate")
            self.step("boot3-single-passphrase-ssh", up, "ONE passphrase typed (LUKS swap + ZFS): a second prompt would have blocked boot")
            rc, o = self.sh("swapon --noheadings --bytes --show=NAME,SIZE,PRIO; echo ---; cryptsetup status hxswap | head -8; echo ---; "
                            "cat /sys/power/resume; echo ---; test -e /run/hx/key && echo KEYLEFT || echo nokey; echo ---; "
                            "grep -E 'resume=|rd.hx.swap=' /proc/cmdline | tr ' ' '\\n' | grep -E 'resume|hx'; echo ---; "
                            "zfs get -H -o value keystatus,keylocation rpool | tr '\\n' ' '; echo ---; grep -c hxswap /etc/fstab; cat /etc/fstab | grep swap")
            self.res["hibernate_swap"] = o
            sw = o.split("---")[0]
            m = re.search(r"/dev/(?:mapper/hxswap|dm-\d+)\s+(\d+)\s+(-?\d+)", sw)
            ram_kb = int(re.search(r"MemTotal:\s+(\d+)", self.sh("cat /proc/meminfo")[1]).group(1))
            self.step("hibernate-swap-is-luks2-dm-device-at-least-ram", bool(m) and int(m.group(1)) >= ram_kb * 1024 and "LUKS2" in o,
                      o.replace("\n", " | ")[:400])
            self.step("resume-device-set-and-no-key-left-in-run", re.search(r"---\s*\d+:\d+", o) is not None and "nokey" in o and "KEYLEFT" not in o,
                      "/sys/power/resume set by hx-resume; /run/hx/key shredded")
            self.step("kernel-args-resume-and-unlock", "resume=UUID=" in o and "rd.hx.swap=" in o, "")
            self.step("zram-higher-priority-than-disk-swap", "zram" not in sw or (bool(re.search(r"zram\d+\s+\d+\s+16383", sw))), sw.strip().replace("\n", " | "))
            # initramfs: the one-passphrase module and the hook order, on the REAL installed image
            rc, o = self.sh("for k in /boot/initramfs-*.img /boot/be/*/initramfs.img; do echo \"== $k\"; lsinitrd $k | grep -E 'hx-unlock|hx-zfskey|resume|cryptsetup|zfs.ko|dm-crypt|mount-zfs|horcrux-unlock'; done")
            open(f"{a.out}/initramfs-listing.txt", "w").write(o)
            from importlib import util as _u
            sp = _u.spec_from_file_location("zfsswap", os.path.join(HERE, "overlay/usr/lib/live-installer/liveinst/zfsswap.py"))
            # (zfsswap imports its package siblings lazily only inside the installer stages: the pure checks load standalone)
            zs = _u.module_from_spec(sp)
            sp.loader.exec_module(zs)
            parts = re.split(r"^== (\S+)$", o, flags=re.M)
            bad = []
            for img, txt in zip(parts[1::2], parts[2::2]):
                pr = zs.initramfs_problems(txt)
                ho = zs.hook_order_ok(txt + "\nusr/lib/dracut/hooks/pre-mount/50-hx-zfskey.sh\n")
                if pr or not ho:
                    bad.append(f"{img}: missing {pr} hook-order-ok={ho}")
            self.step("initramfs-has-unlock-resume-zfs-and-hook-order", bool(parts[1::2]) and not bad, "; ".join(bad) or f"{len(parts[1::2])} images ok")
            # marker in RAM + a running process
            rc, o = self.sh("echo $(cat /proc/sys/kernel/random/boot_id) > /dev/shm/boot_id_before; head -c 24 /dev/urandom | base64 > /dev/shm/marker; "
                            "cp /dev/shm/marker /run/marker-run; "
                            "nohup sh -c 'i=0; while :; do i=$((i+1)); echo $i > /dev/shm/counter; sleep 1; done' >/dev/null 2>&1 & echo $! > /dev/shm/counter.pid; "
                            "sleep 4; cat /dev/shm/counter /dev/shm/counter.pid /dev/shm/marker /dev/shm/boot_id_before; zpool status -x")
            self.res["marker_before"] = o
            lines = o.strip().splitlines()
            before = {"counter": int(lines[0]), "pid": lines[1].strip(), "marker": lines[2].strip(), "boot_id": lines[3].strip()}
            self.step("marker-and-process-written", before["counter"] >= 3, f"counter={before['counter']} pid={before['pid']}")
            rc, o = self.sh("loginctl hibernate; echo rc=$?", timeout=40)
            # elogind starts the hibernation and the guest dies under the ssh session: "closed by remote host" / no output = it worked;
            # an explicit non-zero rc means loginctl refused (then the kernel interface is tried so the rest still runs)
            refused = "rc=" in o and "rc=0" not in o
            if refused:
                self.step("loginctl-hibernate", False, o.strip().replace("\n", " | ")[:300])
                self.sh("echo disk > /sys/power/state", timeout=30)
            t = time.time()
            while vm.alive() and time.time() - t < qt.T(180):
                time.sleep(2)
            gone = not vm.alive()
            if not refused:
                self.step("loginctl-hibernate", gone, "`loginctl hibernate` (elogind) wrote the image and powered the guest off: " + o.strip().replace("\n", " | ")[:100])
            self.step("hibernate-powered-off", gone, f"VM powered off after writing the image ({int(time.time() - t)}s)")
            if not gone:
                vm.p.kill()
        finally:
            if vm.alive():
                vm.p.kill()
        if not gone:
            return
        time.sleep(3)
        # boot again: ONE passphrase; the kernel resumes the saved memory BEFORE the pool is imported
        qt.pflash_vars(nvars)
        vm = qt.VM("5-boot4-resume", self.qemu_args(None, True, nvars), a.out)
        try:
            _ok, ok = self.type_passphrase_usb(vm, "passphrase-prompt-resume-boot")
            self.step("resume-boot-single-passphrase-ssh", ok, "back on the network after ONE passphrase")
            rc, o = self.sh("echo BOOT_NOW=$(cat /proc/sys/kernel/random/boot_id); echo BOOT_BEFORE=$(cat /dev/shm/boot_id_before 2>/dev/null); "
                            "echo MARKER=$(cat /dev/shm/marker 2>/dev/null); echo C1=$(cat /dev/shm/counter 2>/dev/null); "
                            "p=$(cat /dev/shm/counter.pid 2>/dev/null); kill -0 $p 2>/dev/null && echo PROC=ALIVE || echo PROC=DEAD; sleep 3; "
                            "echo C2=$(cat /dev/shm/counter 2>/dev/null); dmesg | grep -iE 'hibernat|image restored|PM: Image' | tail -6")
            kv = dict(l.split("=", 1) for l in o.splitlines() if re.match(r"^[A-Z0-9_]+=", l))
            resumed = bool(kv.get("BOOT_NOW")) and kv.get("BOOT_NOW") == kv.get("BOOT_BEFORE") == before["boot_id"] and kv.get("MARKER") == before["marker"]
            self.step("resumed-same-boot-id-and-marker-survived", resumed, f"boot_id before/now: {before['boot_id'][:8]}/{(kv.get('BOOT_NOW') or '')[:8]}; marker equal={kv.get('MARKER') == before['marker']}")
            c1 = int(kv["C1"]) if (kv.get("C1") or "").isdigit() else -1
            c2 = int(kv["C2"]) if (kv.get("C2") or "").isdigit() else -1
            self.step("process-survived-and-still-running", kv.get("PROC") == "ALIVE" and c1 >= before["counter"] and c2 > c1,
                      f"counter before hibernate {before['counter']}, after resume {c1} -> {c2}")
            ls = [l for l in o.splitlines() if not re.match(r"^[A-Z0-9_]+=", l)]
            self.res["resume_dmesg"] = "\n".join(ls)
            rc, o = self.sh("zpool status -x; zpool status rpool | grep -E 'state|errors'; zfs get -H -o value keystatus rpool; test -e /run/hx/key && echo KEYLEFT || echo nokey")
            self.step("pool-healthy-after-resume", "all pools are healthy" in o and "available" in o and "KEYLEFT" not in o, o.strip().replace("\n", " | ")[:300])
            rc, o = self.sh("zpool scrub -w rpool; zpool status rpool | grep -E 'scan:|errors:|state:'", timeout=int(qt.T(600)))
            self.step("scrub-clean-after-resume", rc == 0 and "with 0 errors" in o and "No known data errors" in o, o.strip().replace("\n", " | ")[:300])
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()

    def session_audio_autostart_pre(self, user):
        """before the first GNOME login: an XDG autostart entry for this user and a system-wide one, each leaving a marker when gnome-session-openrc runs it"""
        import base64
        ent = lambda n: base64.b64encode(f"[Desktop Entry]\nType=Application\nName=hx {n}\nExec=/bin/sh -c 'date > /tmp/hx-autostart-{n}'\n".encode()).decode()
        self.sh(f"rm -f /tmp/hx-autostart-*; mkdir -p /home/{user}/.config/autostart; chown -R {user}:{user} /home/{user}/.config; echo {ent('user')} | base64 -d > /home/{user}/.config/autostart/hx-autostart-user.desktop; "
                f"chown {user}:{user} /home/{user}/.config/autostart/hx-autostart-user.desktop; echo {ent('xdg')} | base64 -d > /etc/xdg/autostart/hx-autostart-xdg.desktop")

    def session_audio_autostart_checks(self, user, vm):
        """after the GNOME login: XDG autostart ran, PipeWire is up in the user session by itself (no per-user runlevel), the gsd audio daemons started after it,
        and a volume key (QMP volumeup -> USB keyboard -> mutter -> gsd-media-keys) really changes the sink volume"""
        uid = self.sh(f"id -u {user}")[1].strip().splitlines()[-1]
        env = f"XDG_RUNTIME_DIR=/run/user/{uid} DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"
        u = lambda c, t=60: self.sh(f"su - {user} -c \"export {env}; {c}\" 2>&1", timeout=t)
        # the first login of a fresh account is slow (cold caches): XDG autostart runs some seconds after gnome-shell appears, so poll instead of sampling once
        end = time.time() + qt.T(120)
        while True:
            rc, o = self.sh("ls /tmp/hx-autostart-* 2>&1; cat /etc/user/init.d/gsd | grep -c 'want pipewire-pulse wireplumber'")
            if ("hx-autostart-user" in o and "hx-autostart-xdg" in o) or time.time() > end:
                break
            time.sleep(5)
        self.step("xdg-autostart-runs-under-gnome-session-openrc", "hx-autostart-user" in o and "hx-autostart-xdg" in o, o.strip().replace("\n", " | ")[:200])
        self.sh("rm -f /etc/xdg/autostart/hx-autostart-xdg.desktop /home/%s/.config/autostart/hx-autostart-user.desktop /tmp/hx-autostart-*" % user)
        time.sleep(8)
        rc, o = u("wpctl status | sed -n 1,30p")
        self.step("pipewire-wireplumber-pulse-running-after-login-and-sink-present", "WirePlumber" in o and re.search(r"\d+\.\s+\S", o.partition("Sinks:")[2].partition("Sources:")[0]) is not None and "Could not connect" not in o,
                  re.sub(r"\s+", " ", o)[:400])
        rc, o = self.sh(f"for p in $(pgrep -u {user} 'pipewire|wireplumber|gsd-media-keys|gsd-sound'); do echo $(awk '{{print $22}}' /proc/$p/stat) $(cat /proc/$p/comm) $(tr '\\0' ' ' < /proc/$p/cmdline | cut -c1-60); done | sort -n")
        t = {}
        for l in o.strip().splitlines():
            f = l.split()
            if len(f) >= 2 and f[0].isdigit():
                t.setdefault("pulse" if "pipewire-pulse" in l or "pipewire.*pulse" in l or "pulse.conf" in l else f[1], int(f[0]))
        okord = all(k in t for k in ("wireplumber", "gsd-media-keys", "gsd-sound", "pipewire")) and max(t["wireplumber"], t["pipewire"], t.get("pulse", 0)) <= min(t["gsd-media-keys"], t["gsd-sound"])
        self.step("gsd-media-keys-and-gsd-sound-started-after-pipewire-and-wireplumber", okord, o.strip().replace("\n", " | ")[:300])
        rc, o = u("wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.40; wpctl get-volume @DEFAULT_AUDIO_SINK@")
        before = re.findall(r"Volume: ([0-9.]+)", o)
        after = []
        for _ in range(3):
            vm.qmp("send-key", keys=[{"type": "qcode", "data": "volumeup"}])
            time.sleep(1.5)
            rc, o2 = u("wpctl get-volume @DEFAULT_AUDIO_SINK@")
            after = re.findall(r"Volume: ([0-9.]+)", o2)
            if after and before and float(after[-1]) > float(before[-1]):
                break
        self.step("volume-key-changes-the-sink-volume", bool(before and after and float(after[-1]) > float(before[-1])), f"before {before} after {after}")

    def tray_extension_checks(self, user):
        """GNOME has no tray without the AppIndicator extension: it must be enabled by the system dconf default and a StatusNotifierItem must register"""
        uid = self.sh(f"id -u {user}")[1].strip().splitlines()[-1]
        env = f"XDG_RUNTIME_DIR=/run/user/{uid} DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"
        rc, o = self.sh(f"su - {user} -c \"export {env}; gnome-extensions list --enabled; echo ---; gsettings get org.gnome.shell enabled-extensions; echo ---; "
                        f"gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner org.kde.StatusNotifierWatcher\" 2>&1; "
                        f"ls /etc/dconf/db/local /etc/dconf/profile/user; qfile /etc/dconf/db/local /etc/dconf/profile/user")
        self.step("gnome-extensions-enabled-includes-appindicator-from-system-dconf", "appindicatorsupport@rgcjonas.gmail.com" in o.partition("---")[0] and "(true,)" in o, re.sub(r"\s+", " ", o)[:400])
        rc, o = self.sh(f"echo {SNI_TEST} | base64 -d > /tmp/hx-sni.py; su - {user} -c \"export {env}; python3 /tmp/hx-sni.py\" 2>&1; rm -f /tmp/hx-sni.py", timeout=90)
        self.step("status-notifier-item-registers-on-the-watcher-and-a-host-exists", "StatusNotifierItem-4242-1" in o.partition("SNI-RESULT")[2] and "'IsStatusNotifierHostRegistered': True" in o, re.sub(r"\s+", " ", o)[:400])

    def _user_bus_env(self, user):
        uid = self.sh(f"id -u {user}")[1].strip().splitlines()[-1]
        return uid, f"XDG_RUNTIME_DIR=/run/user/{uid} DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"

    def session_bus_checks(self, user):
        """gnome-shell must live on the OpenRC user bus (/run/user/UID/bus), not on gdm's private /tmp/dbus-* bus, so that org.gnome.Shell and
        org.gnome.SessionManager are on the SAME bus (otherwise Log Out / Restart / Power Off in the shell menu do nothing, and gsd-media-keys cannot grab keys)"""
        uid, env = self._user_bus_env(user)
        rc, o = self.sh(f"P=$(pgrep -u {user} -x gnome-shell | head -1); echo PID=$P; tr '\\0' '\\n' < /proc/$P/environ | grep -E '^(DBUS_SESSION_BUS_ADDRESS|XDG_RUNTIME_DIR)='")
        self.step("gnome-shell-runs-on-the-user-bus-not-gdms-private-bus", f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus" in o and "/tmp/dbus-" not in o, re.sub(r"\s+", " ", o)[:300])
        names = "; ".join(f"echo {n}: \\$(gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner {n})" for n in ("org.gnome.SessionManager", "org.gnome.Shell"))
        rc, o = self.sh(f"B=$(tr '\\0' '\\n' < /proc/$(pgrep -u {user} -x gnome-shell | head -1)/environ | grep '^DBUS_SESSION_BUS_ADDRESS=' | cut -d= -f2-); "
                        f"su - {user} -c \"export XDG_RUNTIME_DIR=/run/user/{uid} DBUS_SESSION_BUS_ADDRESS=$B; echo BUS=$B; {names}\" 2>&1")
        self.step("session-manager-and-shell-names-on-the-shells-own-bus", o.count("(true,)") == 2, re.sub(r"\s+", " ", o)[:300])

    def session_logout(self, user):
        """Log Out through the shell's own bus (org.gnome.SessionManager.Logout 1); wait until the user's shell is gone and GDM shows a greeter session again"""
        uid, env = self._user_bus_env(user)
        self.sh(f"B=$(tr '\\0' '\\n' < /proc/$(pgrep -u {user} -x gnome-shell | head -1)/environ | grep '^DBUS_SESSION_BUS_ADDRESS=' | cut -d= -f2-); "
                f"su - {user} -c \"export XDG_RUNTIME_DIR=/run/user/{uid} DBUS_SESSION_BUS_ADDRESS=$B; gdbus call --session --dest org.gnome.SessionManager --object-path /org/gnome/SessionManager --method org.gnome.SessionManager.Logout 1\" 2>&1")
        end, gone, greeter = time.time() + qt.T(150), False, False
        o = ""
        while time.time() < end:
            rc, o = self.sh(f"pgrep -u {user} -x gnome-shell >/dev/null && echo USERSHELL; loginctl list-sessions --no-legend | awk '{{print $3}}' | tr '\\n' ' '; pgrep -u gdm-greeter -x gnome-shell >/dev/null && echo GREETERSHELL")
            gone = "USERSHELL" not in o
            greeter = "gdm-greeter" in o and "GREETERSHELL" in o
            if gone and greeter:
                break
            time.sleep(5)
        return gone and greeter, re.sub(r"\s+", " ", o)[:300]

    def session_logout_check(self, user):
        ok, o = self.session_logout(user)
        self.step("shell-log-out-via-session-manager-returns-to-gdm", ok, o)

    def install_rtc_check(self, serial_log):
        """the installer must have written the RTC from a synced clock (liveinst/rtc.py logs one `rtc:` line)"""
        try:
            txt = open(serial_log, errors="replace").read()
        except OSError:
            txt = ""
        m = re.findall(r"rtc: [^\r\n]*", txt)
        self.step("installer-writes-rtc-from-a-synced-clock", any("hardware clock written (UTC) from a synced clock" in x for x in m), " | ".join(m)[:300] or "no `rtc:` line in the install log")

    def clock_checks(self):
        """regression gate for gringotts 2026-11-03 (RTC months off, chrony without sources, no clock hook): the dispatcher hook is package-owned,
        chronyd steps (makestep) and is enabled, and a VM booted with a months-off RTC ends up with the right time after the network is up"""
        rc, o = self.sh("f=/etc/NetworkManager/dispatcher.d/20-horcrux-chrony; test -x $f && echo HOOK; qfile -q $f && echo OWNED; qlist -I net-misc/horcrux-chrony-nm; "
                        "grep -h '^makestep' /etc/chrony/chrony.conf /etc/chrony.conf 2>/dev/null | head -2; grep -hE '^rtcsync' /etc/chrony/chrony.conf 2>/dev/null; "
                        "rc-update show default | grep -c chronyd; grep -h '^clock=' /etc/conf.d/hwclock; test -f /usr/lib/live-installer/liveinst/rtc.py && echo RTCPY")
        self.step("chrony-nm-dispatcher-hook-installed-and-package-owned", "HOOK" in o and "OWNED" in o and "horcrux-chrony-nm" in o, re.sub(r"\s+", " ", o).strip()[:300])
        self.step("chronyd-enabled-makestep-rtcsync-clock-utc", "makestep" in o and "rtcsync" in o and re.search(r"(?m)^1$", o) is not None and 'clock="UTC"' in o and "RTCPY" in o, re.sub(r"\s+", " ", o).strip()[:300])
        rc, o = self.sh("date -u +%s")
        try:
            first = int(o.strip().splitlines()[-1])
        except (ValueError, IndexError):
            first = 0
        t0, ok, delta = time.time(), False, None
        end = t0 + qt.T(480)
        while time.time() < end:
            rc, o = self.sh("date -u +%s", timeout=30)
            try:
                delta = int(o.strip().splitlines()[-1]) - int(time.time())
            except (ValueError, IndexError):
                time.sleep(5)
                continue
            if abs(delta) < 120:
                ok = True
                break
            time.sleep(8)
        rc, ch = self.sh("chronyc -n sources 2>&1 | head -6; chronyc -n tracking 2>&1 | head -3")
        self.step("months-off-rtc-clock-stepped-by-chrony-after-network", ok,
                  f"booted {int(time.time()) - first} s from the host clock ({(int(time.time()) - first) // 86400} days off), after {int(time.time() - t0)} s the offset is {delta} s | " + re.sub(r"\s+", " ", ch)[:200])

    def toolchain_check(self):
        """the image strip once deleted /usr/include and every static lib: `C compiler cannot create executables`, no zfs-kmod rebuild on a kernel update"""
        rc, o = self.sh("printf '#include <stdio.h>\\n#include <linux/types.h>\\nint main(void){puts(\"hx-ok\");return 0;}\\n' > /tmp/t.c && gcc /tmp/t.c -o /tmp/t && /tmp/t; rm -f /tmp/t /tmp/t.c", timeout=90)
        self.step("toolchain-compiles-and-links-a-c-program", "hx-ok" in o, o.strip()[-200:])

    # ---- the installed system must be repairable by Portage: every package file exists, and a kernel module can be built against the running kernel
    CONTENTS_CHECK = r"""
import glob, os, collections, re
# the ONLY files that may be missing: runtime state dirs Portage records with .keep markers. Intentional strips are removed from the CONTENTS records at build
# time by common/vdb-prune.py (reviewed list); anything else missing here is a bug in the image (tar --exclude, a strip eating a toolchain file, ...).
ALLOW_PREFIX = ("/var/log/", "/var/spool/", "/var/cache/", "/var/tmp/", "/run/", "/tmp/")
miss = collections.OrderedDict(); n = 0; pk = 0
for cf in sorted(glob.glob("/var/db/pkg/*/*/CONTENTS")):
    pkg = "/".join(cf.split("/")[-3:-1]); pk += 1
    for line in open(cf, errors="replace"):
        if line.startswith("obj "):
            path = " ".join(line.rstrip("\n").split(" ")[1:-2])
        elif line.startswith("sym "):
            path = line[4:line.index(" -> ")]
        else:
            continue
        if os.path.lexists(path) or path.startswith(ALLOW_PREFIX):
            continue
        # dist-kernel (gentoo-kernel-bin) records the initramfs it generates beside the kernel image and deletes it after copying it to /boot
        # ... and the unified kernel image (arch/*/boot/uki.efi) it assembles for the same purpose (USE=generic-uki)
        if re.match(r"^/usr/src/linux-[^/]+/arch/[^/]+/boot/(initrd|uki\.efi)", path):
            continue
        miss.setdefault(pkg, []).append(path); n += 1
print("CONTENTS-PACKAGES", pk)
print("CONTENTS-MISSING", n, "files in", len(miss), "packages")
for p, m in sorted(miss.items(), key=lambda x: -len(x[1]))[:8]:
    print("  ", len(m), p, " ".join(m[:3]))
"""

    KMOD_CHECK = r"""
set -e; kv=$(uname -r); d=$(mktemp -d); cd $d
test -e /lib/modules/$kv/build/Makefile || { echo "NO-KERNEL-BUILD-TREE for $kv ($(readlink /lib/modules/$kv/build))"; exit 1; }
printf '#include <linux/module.h>\n#include <linux/init.h>\nstatic int __init hx_init(void){return 0;}\nstatic void __exit hx_exit(void){}\nmodule_init(hx_init);\nmodule_exit(hx_exit);\nMODULE_LICENSE("GPL");\n' > hx.c
printf 'obj-m := hx.o\n' > Kbuild
make -C /lib/modules/$kv/build M=$d modules > build.log 2>&1 || { tail -15 build.log; echo KMOD-BUILD-FAILED; exit 1; }
modinfo -F vermagic $d/hx.ko | grep -q "^$kv " && echo KMOD-OK $kv
cd /; rm -rf $d
"""

    def _b64sh(self, interp, text):
        import base64
        return f"echo {base64.b64encode(text.encode()).decode()} | base64 -d | {interp}"

    def package_contents_check(self):
        """gringotts 2026-11-03: the image strip deleted 444 packages' dev files and the unpack dropped every dir named `dev`: a package database that lies, so no
        file-level repair and no kernel-module rebuild. FAIL if any installed package has a missing CONTENTS file (beyond the runtime-state allowlist above)."""
        rc, o = self.sh(self._b64sh("python3", self.CONTENTS_CHECK), timeout=300)
        m = re.search(r"CONTENTS-MISSING (\d+) files", o)
        self.step("package-contents-complete-no-missing-files", bool(m) and m.group(1) == "0" and "CONTENTS-PACKAGES" in o, re.sub(r"\s+", " ", o).strip()[:400])

    def kmod_build_check(self):
        """a kernel module builds against the RUNNING kernel's build tree (what zfs-kmod needs after every kernel update: kernel tree + compiler + libelf/openssl)"""
        rc, o = self.sh(self._b64sh("sh", self.KMOD_CHECK), timeout=600)
        self.step("kernel-module-builds-against-running-kernel", "KMOD-OK" in o, re.sub(r"\s+", " ", o).strip()[-300:])
        # ... and a kernel UPDATE must rebuild the ZFS module in the same transaction (USE=dist-kernel on zfs-kmod), else the new initramfs has no zfs.ko
        rc, o = self.sh("if qlist -I sys-kernel/gentoo-kernel-bin sys-kernel/gentoo-kernel | grep -q .; then "
                        "grep -qw dist-kernel /var/db/pkg/sys-fs/zfs-kmod-*/USE && echo DISTKERNEL-OK || echo DISTKERNEL-MISSING; else echo NOT-DIST-KERNEL; fi; "
                        "grep -h dist-kernel /etc/portage/package.use/* 2>/dev/null | head -4; qfile -q /etc/portage/package.use/horcrux-zfs")
        self.step("zfs-kmod-rebuilds-with-every-dist-kernel-update", "DISTKERNEL-OK" in o or "NOT-DIST-KERNEL" in o, re.sub(r"\s+", " ", o).strip()[:300])

    def desktop_exec_check(self):
        rc, o = self.sh(DESKTOP_EXEC_CHECK, timeout=120)
        self.step("desktop-files-exec-binaries-exist", "DESKTOP-FILES-CHECKED" in o and "DESKTOP-FILES-MISSING none" in o, re.sub(r"\s+", " ", o).strip()[:400])

    def keyring_pam_checks(self):
        """owner order 2026-11-03 (never sign in again after a reboot/restore): a GDM password login must start exactly ONE pam_gnome_keyring set, with
        the session line AFTER pam_elogind. pambase[gnome-keyring] + gdm-password gave two daemons, the first before /run/user/UID existed: no persistent
        login keyring (gringotts: empty ~/.local/share/keyrings, every sign-in lost at logout). Reproduced and fixed in a VM with real GDM keyboard logins
        (ctg-infrastructure hosts/gringotts/keyring-qa); this static gate keeps the stack from regressing (stale base, pambase update)."""
        rc, o = self.sh("/usr/libexec/horcrux-keyring-pam check; echo rc=$?; qfile /usr/libexec/horcrux-keyring-pam /usr/bin/horcrux-keyring-status; "
                        "echo SLL=$(grep -c '^[^#]*pam_gnome_keyring' /etc/pam.d/system-local-login); echo GDM=$(grep -c '^[^#]*pam_gnome_keyring' /etc/pam.d/gdm-password); "
                        "echo PASSWD=$(grep -c '^[^#]*pam_gnome_keyring' /etc/pam.d/passwd)")
        self.step("gdm-login-starts-one-keyring-daemon-after-elogind", "OK keyring PAM" in o and "rc=0" in o and "SLL=0" in o and "GDM=3" in o and "PASSWD=1" in o
                  and "gnome-extra/horcrux-gnome-defaults" in o, re.sub(r"\s+", " ", o)[:300])

    def desktop_checks(self, vm=None):
        end = time.time() + qt.T(240)
        o = ""
        while time.time() < end:
            rc, o = self.sh("pgrep -x gdm >/dev/null && echo GDM; pgrep -f gnome-shell >/dev/null && echo SHELL; pgrep -x NetworkManager >/dev/null && echo NM; "
                            "rc-status -s | grep -E 'gdm|xdm|elogind|NetworkManager|dbus' | tr -s ' '; id owner")
            if "GDM" in o and "SHELL" in o:
                break
            time.sleep(6)
        self.step("desktop-gdm-and-gnome-shell-running", "GDM" in o and "SHELL" in o, o.strip().replace("\n", " | ")[:300])
        qt.greeter_no_session_gate(self.sh, self.step, "owner", "gdm", wait=60)         # OWNER RULE: after the disk unlock the greeter shows, no user session exists before a greeter login
        self.desktop_exec_check()
        if getattr(self.z, "public", False):
            self.public_image_checks()
        self.keyring_pam_checks()
        self.clock_checks()
        self.toolchain_check()
        self.package_contents_check()
        self.kmod_build_check()
        # hogshead hand-off: an account with an EMPTY, expired (day 0) password must force the owner to choose a password at the first
        # login (PAM nullok + new_authtok_reqd), with no temporary password stored anywhere. Exercised through login(1) on a pty: no password prompt (nullok), forced new-password prompt, passwdqc.
        rc, o = self.sh("userdel -rf hxtmp >/dev/null 2>&1; useradd -m hxtmp && passwd -d hxtmp >/dev/null && chage -d 0 hxtmp && grep '^hxtmp:' /etc/shadow | cut -d: -f2,3; "
                        "(sleep 2; printf 'Granite-Meadow-Lantern-47!\\n'; sleep 1; printf 'Granite-Meadow-Lantern-47!\\n'; sleep 4; printf 'id\\nexit\\n') | "
                        "script -qec 'login hxtmp' /dev/null | tr -d '\\r' | tail -6; "
                        "echo AFTER; grep '^hxtmp:' /etc/shadow | cut -d: -f2,3 | cut -c1-3; sleep 3; userdel -rf hxtmp >/dev/null 2>&1; true", timeout=90)
        before, _, after = o.partition("AFTER")
        self.step("empty-expired-password-forces-change-at-first-login", before.strip().startswith(":0") and after.strip()[:1] == "$" and "uid=" in before,
                  o.strip().replace("\n", " | ")[:300])
        if vm:
            # the greeter is graphical: wait until the VGA console is no longer the black text console (many distinct colours)
            end, colours = time.time() + qt.T(150), 0
            while time.time() < end:
                ppm = vm.grab()
                m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", ppm or b"")
                if m:
                    px = ppm[m.end():]
                    colours = len({px[i:i + 3] for i in range(0, min(len(px), 3 * 1280 * 800), 3 * 53)})
                    if colours > 40:
                        break
                time.sleep(4)
            time.sleep(3)
            vm.shot("gdm-login-screen")
            self.step("gdm-greeter-is-graphical-on-vga", colours > 40, f"{colours} distinct colours sampled on the VGA console (text console has < 10)")


def main():
    qt.arm_deadline()
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--desktop", action="store_true")
    ap.add_argument("--server", action="store_true", help="HEADLESS SERVER gate (server-first): serial console + ssh only, no VGA/USB keyboard path; the same checks as the desktop run "
                    "(months-off RTC, late network, package-contents-complete, kernel-module build, zfs-kmod dist-kernel rule, shadow reuse, no autologin, reboot into the installed system) "
                    "plus: no display stack installed, web UI answers, core packages present, nothing personal. Runs on x86_64 (KVM) and aarch64 (TCG).")
    ap.add_argument("--public", action="store_true", help="a PUBLIC image: nothing personal / machine-specific may be installed (project/public-forbidden.txt)")
    ap.add_argument("--overlay-dir", default="", help="(--server) the repo's gentoo/ overlay: its horcrux-core packages are copied over the synced /var/db/repos/horcrux before `emerge -pvuDN @world`, "
                    "for a run on a branch whose overlay is not published yet (on main the published overlay is what the installed system really syncs)")
    ap.add_argument("--late-network", type=int, default=0, metavar="SECS", help="(--server) boot #2 with the NIC link DOWN for SECS seconds (QMP set_link), then up: chrony must still step the clock and the web UI must come up")
    ap.add_argument("--hibernate", action="store_true", help="portable profile: LUKS2 hibernation swap, ONE passphrase, hibernate -> power off -> boot -> resume (marker + process survive, pool clean, scrub clean)")
    ap.add_argument("--fake-t2", action="store_true", help="force the T2 path with answer-file t2=yes (this QEMU cannot present PCI id 106b:1801; detection itself is unit-tested with a fake sysfs)")
    ap.add_argument("--expect-t2-refusal", action="store_true", help="(with --fake-t2) the image has no T2 kernel: assert the installer refuses")
    ap.add_argument("--t2", default="no", help="answer file t2 = no|auto|yes (default no: the normal tests ignore a T2 device)")
    ap.add_argument("--rtc-base", default="2026-07-01T12:00:00", help="desktop runs: start EVERY boot with this (months-off) hardware clock; '' disables the clock gates")
    ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--cpu", default=os.environ.get("QEMU_TEST_CPU", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16"))
    ap.add_argument("--disk-gb", type=int, default=24)
    ap.add_argument("--keep-disks", action="store_true")
    a = ap.parse_args()
    qt.ARCH = qt.HOST_ARCH
    kvm = os.path.exists("/dev/kvm")
    qt.SLOW = float(os.environ.get("QEMU_SLOW", 0) or 0) or (1.0 if kvm else 3.0)
    if a.desktop:
        a.mem = max(a.mem, 6144)
    try:
        ZRun(a).run()
    finally:
        if not a.keep_disks:
            for n in ("target.qcow2", "target2.qcow2", "answers.img", "OVMF_VARS.fd"):
                try:
                    os.unlink(os.path.join(a.out, n))
                except OSError:
                    pass


if __name__ == "__main__":
    main()
