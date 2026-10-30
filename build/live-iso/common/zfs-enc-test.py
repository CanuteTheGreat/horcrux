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
        rc, o = self.sh("emerge -puDN @world 2>&1 | tail -25; echo RC=${PIPESTATUS[0]}", timeout=600)
        open(f"{a.out}/world-resolve.txt", "w").write(o)
        self.step("emerge-world-resolves-no-blockers", "RC=0" in o and "blocked" not in o.lower(), o.strip().replace("\n", " | ")[-300:])
        rc, o = self.sh("mkdir -p /mnt/under; mount --bind / /mnt/under; ls -A /mnt/under/home | wc -l; umount /mnt/under; zfs get -H -o value mounted rpool/home")
        self.step("nothing-hidden-under-the-home-dataset", o.strip().splitlines()[-2:] == ["0", "yes"], "files in the root BE's /home beneath the mounted home dataset: " + o.strip().replace("\n", " "))
        if a.desktop:
            self.desktop_checks()

    def change_and_rollback(self):
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
            rc, o = self.sh("k=$(ls /boot/initramfs-* | tail -1); lsinitrd $k | grep -E 'hx-unlock|hx-zfskey|resume|cryptsetup|zfs.ko|dm-crypt|mount-zfs'")
            from importlib import util as _u
            sp = _u.spec_from_file_location("zfsswap", os.path.join(HERE, "overlay/usr/lib/live-installer/liveinst/zfsswap.py"))
            # (zfsswap imports its package siblings lazily only inside the installer stages: the pure checks load standalone)
            zs = _u.module_from_spec(sp)
            sp.loader.exec_module(zs)
            self.step("initramfs-has-unlock-resume-zfs-and-hook-order", not zs.initramfs_problems(o) and zs.hook_order_ok(o + "\nusr/lib/dracut/hooks/pre-mount/50-hx-zfskey.sh\n") , o.strip().replace("\n", " | ")[:300])
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
        rc, o = self.sh("ls /tmp/hx-autostart-* 2>&1; cat /etc/user/init.d/gsd | grep -c 'want pipewire-pulse wireplumber'")
        self.step("xdg-autostart-runs-under-gnome-session-openrc", "hx-autostart-user" in o and "hx-autostart-xdg" in o, o.strip().replace("\n", " | ")[:200])
        self.sh("rm -f /etc/xdg/autostart/hx-autostart-xdg.desktop /home/%s/.config/autostart/hx-autostart-user.desktop /tmp/hx-autostart-*" % user)
        time.sleep(8)
        rc, o = u("wpctl status | sed -n 1,30p")
        self.step("pipewire-wireplumber-pulse-running-after-login-and-sink-present", "WirePlumber" in o and re.search(r"\d+\.\s+\S", o.partition("Sinks:")[2].partition("Sources:")[0]) is not None and "Could not connect" not in o,
                  re.sub(r"\s+", " ", o)[:400])
        rc, o = self.sh(f"ps -u {user} -o lstart=,comm= | grep -E 'pipewire$|wireplumber|pipewire-pulse|gsd-media-keys|gsd-sound' | while read a b c d e comm; do echo $(date -d \"$a $b $c $d $e\" +%s) $comm; done | sort -n")
        names = [l.split()[-1] for l in o.strip().splitlines()]
        pos = {n: i for i, n in enumerate(names)}
        okord = all(n in pos for n in ("wireplumber", "gsd-media-keys", "gsd-sound")) and max(i for n, i in pos.items() if n in ("wireplumber", "pipewire")) <= min(pos["gsd-media-keys"], pos["gsd-sound"])
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

    def desktop_exec_check(self):
        rc, o = self.sh(DESKTOP_EXEC_CHECK, timeout=120)
        self.step("desktop-files-exec-binaries-exist", "DESKTOP-FILES-CHECKED" in o and "DESKTOP-FILES-MISSING none" in o, re.sub(r"\s+", " ", o).strip()[:400])

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
        self.desktop_exec_check()
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
    ap.add_argument("--hibernate", action="store_true", help="portable profile: LUKS2 hibernation swap, ONE passphrase, hibernate -> power off -> boot -> resume (marker + process survive, pool clean, scrub clean)")
    ap.add_argument("--fake-t2", action="store_true", help="force the T2 path with answer-file t2=yes (this QEMU cannot present PCI id 106b:1801; detection itself is unit-tested with a fake sysfs)")
    ap.add_argument("--expect-t2-refusal", action="store_true", help="(with --fake-t2) the image has no T2 kernel: assert the installer refuses")
    ap.add_argument("--t2", default="no", help="answer file t2 = no|auto|yes (default no: the normal tests ignore a T2 device)")
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
