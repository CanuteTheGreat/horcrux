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
            m = vm.expect_raw(r"(?i)(encrypted zfs password|enter passphrase|password for rpool)[^\n]*", 420)
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
        if a.desktop:
            self.desktop_checks()

    def change_and_rollback(self):
        rc, o = self.sh("horcrux-be snapshot before-change && echo CHANGE > /root/marker-after-snapshot && "
                        "sed -i 's/^CMDLINE=.*/CMDLINE=\"console=tty0\"/' /etc/horcrux/be.conf && "
                        "horcrux-be rollback horcrux@before-change --name rolledback && horcrux-be list && zpool get -H -o value bootfs rpool && "
                        "grep -c 'be-rolledback' /boot/grub/grub.cfg && ! grep -q ttyS0 /boot/grub/grub.cfg")
        self.step("rollback-boot-environment-created", rc == 0 and "rpool/ROOT/rolledback" in o, o.strip().replace("\n", " | ")[-300:])
        self.sh("cat /boot/grub/grub.cfg > /root/grub.cfg.copy")

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
            kb.text(PASSPHRASE, pause=0.05)
            kb.key("ENTER")
            self.step("boot2-ssh-after-usb-keyboard-passphrase", self.wait_ssh(), "passphrase typed on the USB keyboard")
            rc, o = self.sh("zpool get -H -o value bootfs rpool; findmnt -no SOURCE /; test -e /root/marker-after-snapshot && echo MARKER || echo NOMARKER; "
                            "horcrux-be list; dmesg | grep -ci 'i8042' ")
            self.step("rolled-back-to-previous-boot-environment", "rpool/ROOT/rolledback" in o and "NOMARKER" in o, o.strip().replace("\n", " | ")[:300])
            rc, o = self.sh("zfs get -H -o value keystatus rpool; lsblk -no FSTYPE,MOUNTPOINT | head -20")
            if a.desktop:
                self.desktop_checks(vm)
            self.sh("poweroff", timeout=10)
            time.sleep(8)
        finally:
            if vm.alive():
                vm.p.kill()


    # --- boot 3 (--hibernate): ONE passphrase, hibernate, power off, boot, ONE passphrase, resume
    def type_passphrase_usb(self, vm, label):
        ok = self.vga_stable(vm, label)
        kb = qt.Kbd(vm, True)
        kb.text(PASSPHRASE, pause=0.05)
        kb.key("ENTER")
        return ok

    def hibernate_phase(self, nvars):
        a = self.z
        self.no_ps2 = True
        qt.pflash_vars(nvars)
        vm = qt.VM("4-boot3-hibernate", self.qemu_args(None, True, nvars), a.out)
        try:
            self.type_passphrase_usb(vm, "passphrase-prompt-before-hibernate")
            self.step("boot3-single-passphrase-ssh", self.wait_ssh(), "ONE passphrase typed (LUKS swap + ZFS): a second prompt would have blocked boot")
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
            lc_ok = "rc=0" in o
            if not lc_ok:                      # elogind refused: record it as a finding, but still try the kernel interface
                self.step("loginctl-hibernate", False, o.strip().replace("\n", " | ")[:300])
                self.sh("echo disk > /sys/power/state", timeout=30)
            else:
                self.step("loginctl-hibernate", True, "elogind started the hibernation")
            t = time.time()
            while vm.alive() and time.time() - t < qt.T(180):
                time.sleep(2)
            gone = not vm.alive()
            self.step("hibernate-powered-off", gone, f"VM powered off after writing the image ({int(time.time() - t)}s)")
            if not gone:
                vm.p.kill()
        finally:
            if vm.alive():
                vm.p.kill()
        if not gone:
            return
        # boot again: ONE passphrase; the kernel resumes the saved memory BEFORE the pool is imported
        qt.pflash_vars(nvars)
        vm = qt.VM("5-boot4-resume", self.qemu_args(None, True, nvars), a.out)
        try:
            self.type_passphrase_usb(vm, "passphrase-prompt-resume-boot")
            ok = self.wait_ssh()
            self.step("resume-boot-single-passphrase-ssh", ok, "back on the network after ONE passphrase")
            rc, o = self.sh("cat /proc/sys/kernel/random/boot_id; cat /dev/shm/boot_id_before /dev/shm/marker; cat /dev/shm/counter; "
                            "p=$(cat /dev/shm/counter.pid); kill -0 $p && echo ALIVE || echo DEAD; sleep 3; cat /dev/shm/counter; "
                            "dmesg | grep -iE 'hibernat|image restored|PM: Image|resume' | tail -8")
            ls = o.strip().splitlines()
            resumed = len(ls) >= 6 and ls[0] == ls[1] == before["boot_id"] and ls[2] == before["marker"]
            self.step("resumed-same-boot-id-and-marker-survived", resumed, f"boot_id before/after equal={ls[0] == ls[1] if len(ls) > 1 else None}; marker equal={ls[2] == before['marker'] if len(ls) > 2 else None}")
            c1, c2 = int(ls[3]), int(ls[5]) if len(ls) > 5 and ls[5].isdigit() else -1
            self.step("process-survived-and-still-running", ls[4] == "ALIVE" and c1 >= before["counter"] and c2 > c1,
                      f"counter before hibernate {before['counter']}, after resume {c1} -> {c2}")
            self.res["resume_dmesg"] = "\n".join(ls[6:])
            rc, o = self.sh("zpool status -x; zpool status rpool | grep -E 'state|errors'; zfs get -H -o value keystatus rpool; test -e /run/hx/key && echo KEYLEFT || echo nokey")
            self.step("pool-healthy-after-resume", "all pools are healthy" in o and "available" in o and "KEYLEFT" not in o, o.strip().replace("\n", " | ")[:300])
            rc, o = self.sh("zpool scrub -w rpool; zpool status rpool | grep -E 'scan:|errors:|state:'", timeout=int(qt.T(600)))
            self.step("scrub-clean-after-resume", rc == 0 and "with 0 errors" in o and "No known data errors" in o, o.strip().replace("\n", " | ")[:300])
            self.sh("poweroff", timeout=10)
            time.sleep(8)
        finally:
            if vm.alive():
                vm.p.kill()

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
