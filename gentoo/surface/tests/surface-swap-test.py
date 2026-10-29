#!/usr/bin/env python3
"""QEMU test for the PORTABLE SWAP (hibernate) of the Horcrux Surface ISO, and for the swap-free SERVER install.

  surface-swap-test.py --iso horcrux-surface-*.iso --out DIR [--which hibernate|server|none|all]

hibernate : Surface-faked DMI, -cpu IvyBridge -m 4096, UEFI. Unattended install of xfs-luks with `swap` NOT set in the answer file
            (auto -> the Surface hardware profile / DMI says portable -> LUKS2 -> LVM: root + 4.5 GiB swap LV). Boot (USB keyboard
            only), ONE passphrase. Then: a tmpfs marker file + a long-running process holding a random 1 MiB token in memory, record
            boot_id; `loginctl hibernate`; QEMU powers off; boot again, type the passphrase ONCE; the system RESUMES: same boot_id,
            same pid/start time, the counter kept counting, the token still hashes to the recorded value, the tmpfs file is intact.
server    : generic server DMI (no battery, chassis 'other'), same answer file: `swap: none (server...)`, no LVM, no swap partition,
            no swap in fstab/proc, no resume= on the command line.
none      : Surface DMI but answer file `swap = "none"` (override): no LVM, zram only.
Evidence: result.json + screens + serial logs in --out. Reuses surface-qemu-test.py (T class) and common/qemu-test.py.
"""
import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("sq", os.path.join(HERE, "surface-qemu-test.py"))
SQ = importlib.util.module_from_spec(spec)
spec.loader.exec_module(SQ)
Q = SQ.Q
LUKS_PW = SQ.LUKS_PW
SERVER_DMI = "type=1,manufacturer=Example Servers Inc,product=RackServer 1000,version=1"

MARKER_PY = r'''
import os, time, hashlib
tok = os.urandom(1 << 20)                       # lives only in this process' memory
open("/dev/shm/marker.sha", "w").write(hashlib.sha256(tok).hexdigest())
n = 0
while True:
    n += 1
    open("/dev/shm/counter", "w").write(str(n))
    open("/dev/shm/selfcheck", "w").write(hashlib.sha256(tok).hexdigest() + " " + str(n))
    time.sleep(1)
'''


class ST(SQ.T):
    def __init__(self, a, which):
        super().__init__(a)
        self.which = which
        self.res["which"] = which
        self.dir = a.out

    # --------------------------------------------------------------------------------------------- answer files
    def answer(self, pwhash, pub, swap=None, desktop=True):
        lines = ["[global]", 'keyboard = "us"', 'timezone = "Europe/Berlin"', f'hostname = "swap-{self.which}.test.lan"',
                 f'root_password_hash = "{pwhash}"', f'ssh_keys = ["{pub}"]', "ssh_password_auth = false", "",
                 "[disk]", 'filesystem = "xfs-luks"', f'luks_passphrase = "{LUKS_PW}"', 'disks = ["/dev/vda"]']
        if swap:
            lines.append(f'swap = "{swap}"')
        if desktop:
            lines += ["", "[desktop]", "enable = true", f'user = "{SQ.USER}"', f'password = "{SQ.USER_PW}"', "autologin = true"]
        lines += ["", "[network.management]", 'mode = "dhcp"', ""]
        return "\n".join(lines)

    # --------------------------------------------------------------------------------------------- phases
    def setup(self):
        a = self.a
        self.disk, self.answers, self.nvars = f"{a.out}/target.qcow2", f"{a.out}/answers.img", f"{a.out}/OVMF_VARS.fd"
        subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", self.disk, f"{a.disk_gb}G"])
        Q.ARCH = "x86_64"
        Q.pflash_vars(self.nvars)
        self.key_path = f"{a.out}/test_key"
        for f in (self.key_path, self.key_path + ".pub"):
            if os.path.exists(f):
                os.unlink(f)
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "qemu-test", "-f", self.key_path])
        self.pub = open(self.key_path + ".pub").read().strip()
        self.pwhash = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemutest1", Q.PW]).decode().strip()

    def install(self, text, expect_plan):
        a, t0 = self.a, time.time()
        Q.make_answer_image(self.answers, "horcrux", text)
        vm = Q.VM("1-install", self.base(False, self.nvars), a.out)
        try:
            ok = vm.expect_raw(r"(Serial console \(ttyS0|safe graphics)", 150)
            self.step("grub-menu-reached", ok, f"{int(time.time() - t0)}s")
            time.sleep(2)
            if ok and b"Serial console" in ok.group(0):
                vm.send("s")
                time.sleep(1.5)
                vm.send("4")
            else:
                for _ in range(2):
                    vm.send("\x1b[B")
                    time.sleep(0.6)
                vm.send("\r")
            self.step("live-userland-up", vm.expect_raw(r"AUTOINSTALL-START", 480), "answer file found")
            plan = vm.expect_raw(r"AUTOINSTALL-PLAN[^\n]*", 60, consume=False)
            self.step("plan-is-xfs-luks", bool(plan) and b"fs=xfs-luks" in plan.group(0), plan.group(0).decode() if plan else "no plan line")
            ok = vm.expect_raw(r"AUTOINSTALL-(OK|FAILED[^\n]*)", 1700, consume=False)
            log = open(f"{a.out}/1-install-serial.log", "rb").read().decode("latin1")
            open(f"{a.out}/install-log.txt", "w").write(log)
            m = re.search(r"swap: [^\r\n]*", log)
            self.step("installer-logged-the-swap-decision", bool(m) and expect_plan(m.group(0)), m.group(0) if m else "no 'swap:' line in the installer log")
            self.install_log = log
            self.step("installer-finished", ok and b"AUTOINSTALL-OK" in ok.group(0), ok.group(0).decode() if ok else "timeout")
            end = time.time() + 240
            while vm.alive() and time.time() < end:
                time.sleep(1)
            self.step("vm-stopped-after-install", not vm.alive(), f"{int(time.time() - t0)}s total")
        finally:
            if vm.alive():
                vm.p.kill()

    def unlock(self, vm, name, serial=False):
        """wait for the passphrase prompt, type the passphrase ONCE, wait for ssh. Surface profile: VGA console + USB keyboard only.
        A plain server install keeps console=ttyS0 (last console= wins): its prompt is on the serial line, typed there."""
        t0 = time.time()
        if serial:
            ok = vm.expect_raw(r"Enter passphrase for", 420)
            self.step(f"{name}-prompt-on-serial", bool(ok), f"{int(time.time() - t0)}s: server install keeps console=ttyS0")
            time.sleep(1)
            vm.send(LUKS_PW + "\n")
        else:
            time.sleep(40)          # GRUB + "Loading initial ramdisk" also look idle on a loaded host: wait past them before judging the screen
            ppm = self.settle(vm, name, timeout=420, quiet=4)
            self.step(f"{name}-prompt-on-vga", ppm is not None, f"{int(time.time() - t0)}s: passphrase prompt idle on tty0")
            vm.shot(f"{name}-luks-prompt")
            self.key(vm, LUKS_PW + "\n")
        t1 = time.time()
        while time.time() - t1 < 600 and vm.alive():
            if self.ssh("true", timeout=15)[0] == 0:
                return time.time() - t0
            time.sleep(5)
        return None

    # --------------------------------------------------------------------------------------------- hibernate
    def run_hibernate(self):
        a = self.a
        SQ.DMI = "type=1,manufacturer=Microsoft Corporation,product=Surface with Windows 8 Pro,version=1,sku=Surface_Pro"
        self.setup()
        Q.make_answer_image(self.answers, "horcrux", self.answer(self.pwhash, self.pub))   # `swap` is NOT in the answer file
        self.step("answer-file-has-no-swap-key", "swap" not in self.answer(self.pwhash, self.pub).split("[desktop]")[0].replace("swap-hibernate", ""), "auto must decide from the Surface profile")
        self.install(self.answer(self.pwhash, self.pub), lambda s: "hibernate swap" in s and "4.5 GiB" in s and "hardware profile surface" in s)
        log = self.install_log
        self.step("installer-built-lvm-in-luks", "LVM inside LUKS: VG hxvg, swap LV 4608 MiB" in log, "vgcreate hxvg + swap LV 4608 MiB (4.5 GiB = RAM 4 GiB x 1.125)")
        self.step("initramfs-check-incl-lvm-and-resume", re.search(r"initramfs check \([^)]*lvm, resume\): all reachable", log) is not None, "installer's lsinitrd check")
        Q.pflash_vars(self.nvars)
        vm = Q.VM("2-boot1", self.base(True, self.nvars), a.out)
        marker = {}
        try:
            secs = self.unlock(vm, "boot1")
            self.step("boot1-one-passphrase-then-up", secs is not None, f"{int(secs) if secs else 0}s; LVM root + swap came up behind ONE passphrase")
            self.soft = True
            s = self.step
            rc, out = self.ssh("cat /proc/swaps; echo @@@; swapon --show --noheadings; echo @@@; lvs --noheadings -o lv_name,vg_name,lv_size,lv_attr 2>&1; echo @@@; "
                               "findmnt -no SOURCE,FSTYPE /; echo @@@; cat /proc/cmdline; echo @@@; grep swap /etc/fstab; echo @@@; "
                               "cat /sys/power/resume /sys/power/disk /sys/power/state; echo @@@; lsblk -no NAME,TYPE,FSTYPE | tr -s ' '")
            open(f"{a.out}/screens/swap-state-boot1.txt", "w").write(out)
            p = out.split("@@@") + [""] * 6
            procswaps = p[0]
            s("two-swaps-zram-first", re.search(r"/dev/zram0\s+partition\s+\d+\s+\d+\s+100", procswaps) is not None
              and re.search(r"/dev/dm-\d+\s+partition\s+4718588\s+\d+\s+10", procswaps) is not None, re.sub(r"\s+", " ", procswaps)[:200])
            s("swap-lv-is-4.5GiB", "swap" in p[2] and re.search(r"4\.50g", p[2]) is not None, re.sub(r"\s+", " ", p[2]).strip())
            s("root-is-xfs-on-lvm-on-luks", "/dev/mapper/hxvg-root xfs" in p[3], p[3].strip())
            cmd = p[4]
            s("cmdline-has-resume-and-lvm", re.search(r"resume=UUID=[0-9a-f-]{36}", cmd) is not None and "rd.lvm.vg=hxvg" in cmd and "ttyS0" not in cmd, cmd.strip()[:300])
            s("fstab-swap-pri10", re.search(r"/dev/mapper/hxvg-swap\s+none\s+swap\s+sw,pri=10", p[5]) is not None, p[5].strip())
            s("power-sysfs-resume-device-set", re.search(r"^\d+:\d+$", p[6].strip().splitlines()[0]) is not None and "shutdown" in p[6], re.sub(r"\s+", " ", p[6]))
            rc, out = self.ssh("cat /etc/elogind/logind.conf.d/*.conf /etc/elogind/sleep.conf.d/*.conf; echo @@@; grep -E '^(CriticalPowerAction|UsePercentageForPolicy|PercentageAction)' /etc/UPower/UPower.conf; echo @@@; "
                               "for p in HandlePowerKey HandleLidSwitch HandleLidSwitchDocked; do echo \"$p=$(dbus-send --system --print-reply --dest=org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.DBus.Properties.Get string:org.freedesktop.login1.Manager string:$p 2>&1 | grep -o 'string \".*\"')\"; done; "
                               "dbus-send --system --print-reply --dest=org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager.CanHibernate 2>&1 | tail -1; echo @@@; "
                               "lsinitrd /boot/initramfs-*.img > /tmp/lsinitrd.txt 2>&1; grep -E 'resume|lvm' /tmp/lsinitrd.txt | head -8; echo @@@; "
                               "for h in cmdline pre-udev pre-trigger initqueue/settled pre-mount mount; do echo \"hook $h:\"; grep -oE \"hooks/$h/[^ ]*\" /tmp/lsinitrd.txt | tr '\\n' ' '; echo; done; echo @@@; "
                               "f=$(awk '/10-parse-resume.sh/{print $NF}' /tmp/lsinitrd.txt | head -1); echo \"# $f\"; lsinitrd /boot/initramfs-*.img \"$f\" 2>&1 | head -90", timeout=180)
            open(f"{a.out}/screens/policy-and-initramfs.txt", "w").write(out)
            q = out.split("@@@") + [""] * 6
            s("elogind-policy-lid-suspend-hibernate-mode", "HandleLidSwitch=suspend" in q[0] and "HibernateMode=shutdown" in q[0] and "HandlePowerKey=suspend" in q[0], "lid close = suspend, HibernateMode=shutdown")
            s("upower-critical-battery-hibernate", "CriticalPowerAction=Hibernate" in q[1] and "UsePercentageForPolicy=true" in q[1], re.sub(r"\s+", " ", q[1]).strip())
            s("elogind-effective-policy-via-dbus", 'HandlePowerKey=string "suspend"' in q[2] and 'HandleLidSwitch=string "suspend"' in q[2] and 'HandleLidSwitchDocked=string "ignore"' in q[2] and 'string "yes"' in q[2],
              "running elogind (drop-ins read): " + re.sub(r"\s+", " ", q[2]).strip())
            s("initramfs-has-resume-and-lvm-hooks", re.search(r"resume", q[3]) is not None and re.search(r"lvm", q[3]) is not None, re.sub(r"\s+", " ", q[3]).strip()[:300])
            order = q[4]
            open(f"{a.out}/screens/initramfs-hook-order.txt", "w").write(order)
            # dracut order: cmdline hooks -> initqueue (settled hooks run here, as soon as the udev queue is quiet) -> pre-mount -> mount (99-mount-root).
            # parse-resume.sh queues the resume as an initqueue/settled job, so it fires before pre-mount and the root mount.
            pr = q[5]
            open(f"{a.out}/screens/parse-resume.sh.txt", "w").write(pr)
            s("resume-queued-before-root-mount", "hooks/cmdline/10-parse-resume.sh" in order and "hooks/mount/99-mount-root.sh" in order and "wait_for_dev /dev/resume" in pr and "/sys/power/resume" in pr,
              "cmdline/10-parse-resume.sh waits for /dev/resume and queues the initqueue/settled job that writes /sys/power/resume; the root is mounted later by mount/99-mount-root.sh")
            # --- the marker
            rc, out = self.ssh("cat > /root/marker.py <<'EOF'" + MARKER_PY + "EOF\nnohup python3 /root/marker.py >/dev/null 2>&1 < /dev/null & echo $! > /root/marker.pid; sleep 4; "
                               "head -c 65536 /dev/urandom | base64 > /dev/shm/marker.txt; sha256sum /dev/shm/marker.txt | cut -d' ' -f1; "
                               "pid=$(cat /root/marker.pid); echo $pid; awk '{print $22}' /proc/$pid/stat; cat /proc/sys/kernel/random/boot_id; "
                               "cat /dev/shm/counter; echo; cat /dev/shm/marker.sha; echo; awk '{print $1}' /proc/uptime; df -h /dev/shm | tail -1")
            L = out.split()
            marker = {"txt_sha": L[0], "pid": L[1], "start": L[2], "boot_id": L[3], "counter": int(L[4]), "tok_sha": L[5], "uptime": float(L[6])}
            self.res["marker"] = marker
            s("marker-in-ram-recorded", len(marker["tok_sha"]) == 64 and marker["pid"].isdigit(), json.dumps(marker))
            rc, out = self.ssh("free -m | sed -n 2p; cat /proc/swaps | tail -n +2")
            time.sleep(2)
            self.soft = False
            # --- hibernate
            rc, out = self.ssh("sync; (sleep 2; loginctl hibernate) >/root/hibernate.out 2>&1 &", timeout=20)
            t0 = time.time()
            end = t0 + 420
            while vm.alive() and time.time() < end:
                time.sleep(2)
            self.step("hibernate-powers-the-vm-off", not vm.alive(), f"{int(time.time() - t0)}s from `loginctl hibernate` to QEMU exit")
        finally:
            if vm.alive():
                vm.p.kill()
        # --- resume
        Q.pflash_vars(self.nvars) if False else None          # keep the NVRAM of boot 1 (the boot entry stays)
        vm = Q.VM("3-resume", self.base(True, self.nvars), a.out)
        try:
            time.sleep(40)
            ppm = self.settle(vm, "resume", timeout=420, quiet=4)
            self.step("resume-passphrase-prompt-on-vga", ppm is not None, "prompt before the resume (LUKS must be open to read the image)")
            vm.shot("resume-luks-prompt")
            t0 = time.time()
            self.key(vm, LUKS_PW + "\n")
            up = None
            while time.time() - t0 < 600 and vm.alive():
                if self.ssh("true", timeout=15)[0] == 0:
                    up = time.time() - t0
                    break
                time.sleep(5)
            self.step("resumed-after-ONE-passphrase", up is not None, f"ssh answers {int(up) if up else 0}s after the single passphrase")
            time.sleep(8)
            vm.shot("resumed")
            self.soft = True
            s = self.step
            rc, out = self.ssh("cat /proc/sys/kernel/random/boot_id; pid=%s; awk '{print $22}' /proc/$pid/stat; cat /proc/$pid/cmdline | tr '\\0' ' '; echo; cat /dev/shm/counter; echo; "
                               "cat /dev/shm/selfcheck; echo; cat /dev/shm/marker.sha; echo; sha256sum /dev/shm/marker.txt | cut -d' ' -f1; awk '{print $1}' /proc/uptime; "
                               "dmesg | grep -iE 'PM: (hibernation|Image)|Image restored|hibernat' | tail -12" % marker["pid"])
            open(f"{a.out}/screens/resume-state.txt", "w").write(out)
            ln = out.splitlines()
            s("same-boot-id-as-before-hibernate", ln[0].strip() == marker["boot_id"], f"{ln[0].strip()} (a cold boot would have a new one)")
            s("same-process-same-start-time", ln[1].strip() == marker["start"] and "marker.py" in ln[2], f"pid {marker['pid']} starttime {ln[1].strip()} {ln[2].strip()}")
            try:
                ctr = int(ln[3].strip())
            except ValueError:
                ctr = -1
            s("process-kept-running", ctr > marker["counter"], f"counter {marker['counter']} -> {ctr}")
            chk = ln[4].split()
            s("in-memory-token-intact", chk[0] == marker["tok_sha"] == ln[5].strip(), f"sha256 of the 1 MiB token held by the process: {chk[0][:16]}.. == recorded {marker['tok_sha'][:16]}..")
            s("tmpfs-file-intact", ln[6].strip() == marker["txt_sha"], f"/dev/shm/marker.txt sha256 {ln[6].strip()[:16]}..")
            s("uptime-continues-across-hibernate", float(ln[7]) > marker["uptime"], f"{marker['uptime']:.0f}s -> {float(ln[7]):.0f}s")
            s("dmesg-shows-image-restored", "hibernation exit" in "\n".join(ln[8:]), " | ".join(ln[8:])[:300])
            rc, out = self.ssh("cat /proc/swaps; lvs --noheadings 2>&1; cat /root/hibernate.out; grep -iE 'hibernat|resume' /var/log/messages 2>/dev/null | tail -6")
            open(f"{a.out}/screens/after-resume-swap.txt", "w").write(out)
            s("swap-still-active-after-resume", "/dev/zram0" in out and re.search(r"/dev/dm-\d+", out) is not None, re.sub(r"\s+", " ", out)[:240])
            rc, out = self.ssh("dmesg | grep -ciE 'xfs.*(corrupt|error|metadata I/O)|I/O error|EXT4-fs error'; touch /root/after-resume && sync && echo writes-ok")
            s("no-fs-errors-after-resume-and-root-writable", out.split()[0] == "0" and "writes-ok" in out, re.sub(r"\s+", " ", out))
            self.soft = False
            self.ssh("sync; (sleep 2; poweroff) >/dev/null 2>&1 &", timeout=20)
            t0 = time.time()
            while vm.alive() and time.time() < t0 + 180:
                time.sleep(2)
            self.step("clean-poweroff-after-resume", not vm.alive(), f"{int(time.time() - t0)}s")
        finally:
            if vm.alive():
                vm.p.kill()
        # --- a normal (cold) boot afterwards: the image must have been invalidated (no resume loop), one passphrase again
        vm = Q.VM("4-cold", self.base(True, self.nvars), a.out)
        try:
            secs = self.unlock(vm, "cold")
            self.step("cold-boot-after-resume-one-passphrase", secs is not None, f"{int(secs) if secs else 0}s")
            self.soft = True
            rc, out = self.ssh("cat /proc/sys/kernel/random/boot_id; ls /dev/shm; cat /proc/swaps | tail -n +2; ls /root/after-resume; dmesg | grep -ciE 'xfs.*(corrupt|error)|I/O error'")
            L = out.split("\n")
            self.step("cold-boot-is-a-fresh-boot", L[0].strip() != marker["boot_id"] and "counter" not in out and "/dev/zram0" in out, re.sub(r"\s+", " ", out)[:240])
            self.step("cold-boot-fs-clean", out.strip().splitlines()[-1].strip() == "0" and "/root/after-resume" in out, "the write made after the resume survived, no fs errors")
        finally:
            if vm.alive():
                self.ssh("poweroff", timeout=10)
                time.sleep(8)
            if vm.alive():
                vm.p.kill()

    # --------------------------------------------------------------------------------------------- server / none
    def run_plain(self, dmi, swap, desktop, expect_plan, label):
        a = self.a
        SQ.DMI = dmi
        self.setup()
        text = self.answer(self.pwhash, self.pub, swap=swap, desktop=desktop)
        self.install(text, expect_plan)
        Q.pflash_vars(self.nvars)
        vm = Q.VM("2-boot1", self.base(True, self.nvars), a.out)
        try:
            secs = self.unlock(vm, "boot1", serial=(label == "server"))
            self.step("boot-one-passphrase-then-up", secs is not None, f"{int(secs) if secs else 0}s")
            self.soft = True
            s = self.step
            rc, out = self.ssh("cat /proc/swaps; echo @@@; swapon --show --noheadings; echo @@@; lvs --noheadings 2>&1; vgs --noheadings 2>&1; echo @@@; findmnt -no SOURCE,FSTYPE /; echo @@@; "
                               "cat /proc/cmdline; echo @@@; cat /etc/fstab; echo @@@; lsblk -no NAME,TYPE,FSTYPE /dev/vda | tr -s ' '; echo @@@; "
                               "ls /etc/elogind/logind.conf.d /etc/dracut.conf.d 2>&1; echo @@@; free -m | sed -n 3p; zramctl --noheadings; echo @@@; blkid | grep -ci swap")
            open(f"{a.out}/screens/swap-state-{label}.txt", "w").write(out)
            p = out.split("@@@") + [""] * 12
            devs = [l.split()[0] for l in p[0].strip().splitlines()[1:] if l.strip()]
            if label == "server":
                s("server-proc-swaps-empty", devs == [] and p[1].strip() == "", re.sub(r"\s+", " ", p[0]).strip() or "(empty)")
            else:
                s("none-only-zram-no-disk-swap", devs == ["/dev/zram0"], re.sub(r"\s+", " ", p[0]).strip())
            s(f"{label}-no-lvm", p[2].strip() == "" or "No volume groups" in p[2] or "not found" in p[2], re.sub(r"\s+", " ", p[2]).strip() or "no lvs/vgs")
            s(f"{label}-root-is-plain-luks-xfs", "/dev/mapper/hxroot xfs" in p[3], p[3].strip())
            s(f"{label}-no-resume-no-lvm-on-cmdline", "resume=" not in p[4] and "rd.lvm" not in p[4], p[4].strip()[:240])
            s(f"{label}-no-swap-in-fstab", "swap" not in p[5], "fstab has no swap line")
            s(f"{label}-no-swap-partition", "swap" not in p[6] and p[6].count("part") == 3, re.sub(r"\s+", " ", p[6]))
            s(f"{label}-no-portable-policy-files", "10-horcrux-portable" not in p[7] and "95-portable-swap" not in p[7], re.sub(r"\s+", " ", p[7]).strip()[:160])
            s(f"{label}-swap-total-zero", re.search(r"Swap:\s+0\s+0\s+0", p[8]) is not None and p[9].strip().endswith("0") if label == "server" else True, re.sub(r"\s+", " ", p[8]).strip())
        finally:
            if vm.alive():
                self.ssh("poweroff", timeout=10)
                time.sleep(8)
            if vm.alive():
                vm.p.kill()

    def run(self):
        w = self.which
        if w == "hibernate":
            self.run_hibernate()
        elif w == "server":
            self.run_plain(SERVER_DMI, None, False, lambda s: "swap: none (server:" in s, "server")
        elif w == "none":
            self.run_plain("type=1,manufacturer=Microsoft Corporation,product=Surface with Windows 8 Pro,version=1,sku=Surface_Pro", "none", True,
                           lambda s: "swap: none (swap = none requested)" in s, "none")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--which", default="all", choices=["hibernate", "server", "none", "all"])
    ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--cpu", default="IvyBridge")
    ap.add_argument("--disk-gb", type=int, default=24)
    ap.add_argument("--keep-disks", action="store_true")
    a = ap.parse_args()
    runs = ["hibernate", "server", "none"] if a.which == "all" else [a.which]
    bad = []
    base_out = a.out
    for w in runs:
        a.out = os.path.join(base_out, w)
        os.makedirs(a.out, exist_ok=True)
        t = ST(a, w)
        try:
            t.run()
        except SystemExit:
            pass
        finally:
            if not a.keep_disks:
                for n in ("target.qcow2", "answers.img", "OVMF_VARS.fd"):
                    try:
                        os.unlink(os.path.join(a.out, n))
                    except OSError:
                        pass
        t.res["ok"] = not t.failed
        json.dump(t.res, open(f"{a.out}/result.json", "w"), indent=1)
        print(f"== {w}: " + ("ALL PASS" if not t.failed else "FAILED " + ", ".join(t.failed)), flush=True)
        bad += t.failed
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
