#!/usr/bin/env python3
"""secureboot-test.py --iso X.iso --out DIR [--arch x86_64|aarch64] [--scenarios nokey,enroll,off]

UEFI Secure Boot gate for a signed live ISO (boot/secureboot-sign.sh), unattended, on the Secure Boot builds of the
firmware (OVMF/AAVMF ".ms": Microsoft keys in KEK/db, Secure Boot enabled). Everything goes over the serial console.

  nokey   Secure Boot ON, nothing enrolled: shim must refuse our GRUB ("Security Violation"), open MokManager, and GRUB
          must NOT run. (A firmware that boots an unknown signature would make the whole chain meaningless.)
  enroll  Secure Boot ON, nothing enrolled, then the user path exactly as documented: MokManager -> Enroll key from disk ->
          the medium -> ENROLL_THIS_KEY_IN_MOKMANAGER.cer -> Continue -> Yes -> Reboot. After the reboot GRUB starts, the
          kernel reports "Secure boot enabled" and the installer comes up. The enrolled key stays in the firmware's NVRAM.
  off     Secure Boot OFF (plain firmware build): the same shim chain still boots (no regression for SB-off machines).

The installed system under Secure Boot is covered by qemu-test.py --secure-boot mok (install + boot of the disk).
Exit 0 = every step passed; evidence (serial logs, result.json) in --out.
"""
import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time

FW = {
    "x86_64": dict(qemu="qemu-system-x86_64", sb=[("/usr/share/OVMF/OVMF_CODE_4M.secboot.fd", "/usr/share/OVMF/OVMF_VARS_4M.ms.fd"),
                                                  ("/usr/share/OVMF/OVMF_CODE_4M.ms.fd", "/usr/share/OVMF/OVMF_VARS_4M.ms.fd")],
                   plain=[("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd")], serial="ttyS0"),
    "aarch64": dict(qemu="qemu-system-aarch64", sb=[("/usr/share/AAVMF/AAVMF_CODE.ms.fd", "/usr/share/AAVMF/AAVMF_VARS.ms.fd")],
                    plain=[("/usr/share/AAVMF/AAVMF_CODE.no-secboot.fd", "/usr/share/AAVMF/AAVMF_VARS.fd"),
                           ("/usr/share/AAVMF/AAVMF_CODE.fd", "/usr/share/AAVMF/AAVMF_VARS.fd")], serial="ttyAMA0"),
}
KEYS = {"enter": b"\r", "down": b"\x1b[B", "up": b"\x1b[A", "space": b" "}
ANSI = re.compile(rb"\x1b\[[0-9;?]*[a-zA-Z]|\x1b[()][0-9A-Za-z]")
RESULTS = []


def step(name, ok, info=""):
    RESULTS.append({"name": name, "ok": bool(ok), "info": info})
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" ({info})" if info else ""), flush=True)
    return ok


def firmware(arch, sb):
    for code, vars_ in FW[arch]["sb" if sb else "plain"]:
        if os.path.exists(code) and os.path.exists(vars_):
            return code, vars_
    raise SystemExit(f"no {'Secure Boot' if sb else 'plain'} UEFI firmware for {arch} (apt: ovmf / qemu-efi-aarch64)")


class VM:
    def __init__(self, a, name, sb, vars_path):
        self.name, self.out = name, a.out
        code, _ = firmware(a.arch, sb)
        self.sock = os.path.join(a.out, f"{name}.ser")
        if os.path.exists(self.sock):
            os.unlink(self.sock)
        kvm = os.path.exists("/dev/kvm") and os.uname().machine == ("x86_64" if a.arch == "x86_64" else "aarch64")
        if a.arch == "x86_64":
            mach = ["-machine", "q35,smm=on" + (",accel=kvm" if kvm else ""), "-cpu", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16",
                    "-global", "driver=cfi.pflash01,property=secure,value=on"]
            cd = ["-drive", f"if=none,id=cd,file={a.iso},format=raw,media=cdrom,readonly=on", "-device", "ide-cd,bus=ide.0,drive=cd,bootindex=0"]
        else:
            mach = ["-machine", "virt" + (",accel=kvm" if kvm else ""), "-cpu", "host" if kvm else "cortex-a72"]
            cd = ["-device", "virtio-scsi-pci,id=scsi0", "-drive", f"if=none,id=cd,file={a.iso},format=raw,media=cdrom,readonly=on",
                  "-device", "scsi-cd,bus=scsi0.0,drive=cd,bootindex=0"]
        self.log = open(os.path.join(a.out, f"{name}-serial.log"), "wb")
        self.p = subprocess.Popen([FW[a.arch]["qemu"], *mach, "-m", "3072", "-smp", "2",
                                   "-drive", f"if=pflash,format=raw,unit=0,readonly=on,file={code}",
                                   "-drive", f"if=pflash,format=raw,unit=1,file={vars_path}", *cd,
                                   "-display", "none", "-monitor", "none", "-device", "virtio-rng-pci",
                                   "-chardev", f"socket,id=s0,path={self.sock},server=on,wait=off", "-serial", "chardev:s0"],
                                  stderr=subprocess.PIPE)
        for _ in range(60):
            if os.path.exists(self.sock):
                break
            time.sleep(0.5)
        self.s = socket.socket(socket.AF_UNIX)
        self.s.connect(self.sock)
        self.s.settimeout(0.3)
        self.buf, self.mark = b"", 0

    def pump(self, t):
        end = time.time() + t
        while time.time() < end:
            try:
                d = self.s.recv(65536)
                if not d:
                    time.sleep(0.3)
                    continue
                self.buf += d
                self.log.write(d)
                self.log.flush()
            except socket.timeout:
                pass

    def text(self, frm=None):
        return ANSI.sub(b" ", self.buf[self.mark if frm is None else frm:]).decode("latin1")

    def expect(self, rx, timeout):
        end = time.time() + timeout
        while time.time() < end:
            m = re.search(rx, self.text())
            if m:
                self.mark = len(self.buf)
                return m
            self.pump(0.5)
        return None

    def key(self, k, settle=1.5):
        self.s.send(KEYS.get(k, k.encode()))
        self.pump(settle)

    def stop(self):
        if self.p.poll() is None:
            self.p.terminate()
            try:
                self.p.wait(20)
            except subprocess.TimeoutExpired:
                self.p.kill()
        self.log.close()


def fresh_vars(a, sb, name):
    _, tmpl = firmware(a.arch, sb)
    dst = os.path.join(a.out, f"{name}-VARS.fd")
    shutil.copy(tmpl, dst)
    code, _ = firmware(a.arch, sb)
    if a.arch != "x86_64" and os.path.getsize(dst) < os.path.getsize(code):
        with open(dst, "ab") as f:
            f.truncate(os.path.getsize(code))
    return dst


def menu_to_kernel(vm, a, prefix):
    """GRUB menu reached -> serial-console submenu (s) -> text install (2) -> the kernel starts (the menu entries boot `quiet`,
    so the kernel's own Secure Boot line is not on the console; Secure Boot state is asserted where it is observable: the
    refusal without a key here, and inside the installed system by qemu-test.py --secure-boot mok)"""
    ok = vm.expect(r"Serial console \(", 120)
    step(f"{prefix}-grub-menu-reached", ok, "shim verified our GRUB")
    if not ok:
        return False
    time.sleep(1)
    vm.key("s", 2)
    vm.key("2", 2)
    ok = vm.expect(r"Linux version|OpenRC|systemd\[1\]|Welcome to|dracut|Choose the install type", 180)
    step(f"{prefix}-kernel-started", ok, "GRUB loaded the kernel (shim verified its signature)")
    bad = re.search(r"bad shim signature|you need to load the kernel first", vm.text(0))
    step(f"{prefix}-kernel-signature-accepted", not bad, bad.group(0) if bad else "no 'bad shim signature'")
    return bool(ok) and not bad


def scen_nokey(a):
    vm = VM(a, "nokey", True, fresh_vars(a, True, "nokey"))
    try:
        ok = vm.expect(r"Security Violation", 90)
        step("nokey-shim-refuses-unknown-signature", ok, "Verification failed: (0x1A) Security Violation")
        vm.key("enter")
        ok = vm.expect(r"Press any key to perform MOK management", 60)
        step("nokey-mokmanager-opens", ok)
        vm.pump(25)                           # let MokManager's countdown run out: GRUB must still never start
        step("nokey-grub-never-runs", "GNU GRUB" not in vm.text(0) and "Serial console (" not in vm.text(0), "no GRUB output without the key")
    finally:
        vm.stop()


def mok_walk(vm, prefix):
    """the documented enrollment path, keystroke by keystroke"""
    seq = [(r"Security Violation", "enter", "shim refuses the unknown signature"),
           (r"Press any key to perform MOK management", "space", "MokManager countdown"),
           (r"Enroll hash from disk", ["down", "enter"], "menu: Enroll key from disk"),
           (r"EFIBOOT|\[PciRoot|[A-Z_]{4,}", "enter", "volume list: the boot medium"),
           (r"ENROLL_THIS_KEY_IN_MOKMANAGER\.cer", ["down", "enter"], "file list: the certificate"),
           (r"Continue", ["down", "enter"], "[Enroll MOK]: View key / Continue"),
           (r"Yes", ["down", "enter"], "Enroll the key(s)? Yes"),
           (r"Reboot", "enter", "Reboot")]
    for rx, keys, what in seq:
        ok = vm.expect(rx, 90)
        if not step(f"{prefix}-mokmanager: {what}", ok):
            return False
        time.sleep(1)
        for k in ([keys] if isinstance(keys, str) else keys):
            vm.key(k)
    return True


def scen_enroll(a):
    vars_path = fresh_vars(a, True, "enroll")
    vm = VM(a, "enroll", True, vars_path)
    try:
        if not mok_walk(vm, "enroll"):
            return
        menu_to_kernel(vm, a, "enroll-after-reboot")
        ok = vm.expect(r"Choose the install type|install type|Enter an option", 300)
        step("enroll-installer-up-under-secure-boot", ok, "the live installer runs (signed kernel, signed modules)")
        bad = re.search(r"Loading of unsigned module is rejected|bad shim signature", vm.text(0))
        step("enroll-no-signature-rejections", not bad, bad.group(0) if bad else "no unsigned module rejected, no bad kernel signature")
    finally:
        vm.stop()
    # the enrolled key survives a power cycle: boot again with the same NVRAM, no MokManager this time
    vm = VM(a, "enroll-2nd-boot", True, vars_path)
    try:
        ok = vm.expect(r"Serial console \(|Security Violation", 120)
        step("enroll-key-persists-next-boot", ok and "Serial" in ok.group(0), ok.group(0) if ok else "timeout")
    finally:
        vm.stop()


def scen_off(a):
    vm = VM(a, "sb-off", False, fresh_vars(a, False, "sb-off"))
    try:
        menu_to_kernel(vm, a, "off")
        ok = vm.expect(r"Choose the install type|install type|Enter an option", 300)
        step("off-installer-up", ok)
    finally:
        vm.stop()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--arch", default="x86_64", choices=list(FW))
    ap.add_argument("--scenarios", default="nokey,enroll,off")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    r = subprocess.run(["xorriso", "-osirrox", "on", "-indev", a.iso, "-ls", "/"], capture_output=True, text=True)
    if "ENROLL_THIS_KEY_IN_MOKMANAGER.cer" not in r.stdout:
        step("iso-is-signed", False, "no ENROLL_THIS_KEY_IN_MOKMANAGER.cer: the ISO was built without a Secure Boot key")
    else:
        step("iso-is-signed", True, "certificate on the medium")
        for sc in a.scenarios.split(","):
            {"nokey": scen_nokey, "enroll": scen_enroll, "off": scen_off}[sc.strip()](a)
    for f in os.listdir(a.out):
        if f.endswith("-VARS.fd") or f.endswith(".ser"):
            os.unlink(os.path.join(a.out, f))
    with open(os.path.join(a.out, "result.json"), "w") as f:
        json.dump(RESULTS, f, indent=1)
    bad = [x for x in RESULTS if not x["ok"]]
    print(f"secureboot-test: {len(RESULTS) - len(bad)} passed, {len(bad)} failed")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
