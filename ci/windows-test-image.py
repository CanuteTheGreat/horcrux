#!/usr/bin/env python3
"""Windows test image for Horcrux CI (Horcrux Console C2b).

    windows-test-image.py key   WIN_ISO VIRTIO_ISO        -> prints the cache key
    windows-test-image.py build WIN_ISO VIRTIO_ISO OUT.qcow2

`build` installs Windows unattended (ci/windows-test/autounattend.xml) under
KVM with the configuration Horcrux gives a Windows 11 VM (q35 + SMM, Secure
Boot OVMF with the Microsoft keys, swtpm TPM 2.0, virtio-blk), installs the
virtio-win drivers and the QEMU guest agent, waits until the first-logon
settings ran, then powers the guest off through ACPI and leaves OUT.qcow2.
The Horcrux guest tools are deliberately NOT installed: the e2e proves that
the node installs them.

The key covers both ISOs and every file that shapes the image, so a runner
volume can keep one installed image per key and CI does not reinstall on
every run (an install takes 40-60 min). ISO hashes are cached next to the
ISO as `<iso>.sha256` (keyed by size and mtime).

Env: HXW_MEM_MB (default 4096), HXW_CPUS (4), HXW_TIMEOUT_MIN (120),
HXW_WORK (work dir, default next to OUT), HXW_VNC (e.g. 127.0.0.1:47).
"""
import hashlib
import json
import os
import shutil
import socket
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
FILES = [
    os.path.join(HERE, "windows-test", "autounattend.xml"),
    os.path.join(HERE, "windows-test", "hxci-firstlogon.cmd"),
]
# Bump when the install procedure below changes in a way that changes the
# image (the key covers the ISOs and FILES; edits that don't change the
# image keep the cached one).
IMAGE_VERSION = "1"
OVMF_CODE = ["/usr/share/OVMF/OVMF_CODE_4M.secboot.fd", "/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd"]
OVMF_VARS = ["/usr/share/OVMF/OVMF_VARS_4M.ms.fd", "/usr/share/edk2/ovmf/OVMF_VARS.secboot.fd"]


def sha256_file(path, cache=True):
    st = os.stat(path)
    tag = f"{st.st_size}:{int(st.st_mtime)}"
    side = path + ".sha256"
    if cache and os.path.exists(side):
        try:
            t, h = open(side).read().split()
            if t == tag:
                return h
        except (OSError, ValueError):
            pass
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 22), b""):
            h.update(b)
    d = h.hexdigest()
    if cache:
        try:
            with open(side, "w") as f:
                f.write(f"{tag} {d}\n")
        except OSError:
            pass  # read-only ISO location: hash again next time
    return d


def key(win_iso, virtio_iso):
    h = hashlib.sha256(IMAGE_VERSION.encode())
    h.update(sha256_file(win_iso).encode())
    h.update(sha256_file(virtio_iso).encode())
    for f in FILES:
        h.update(hashlib.sha256(open(f, "rb").read()).hexdigest().encode())
    return h.hexdigest()[:16]


def first(paths):
    for p in paths:
        if os.path.exists(p):
            return p
    raise SystemExit(f"FAIL none of {paths} exists (install ovmf)")


class Qmp:
    def __init__(self, path):
        self.s = socket.socket(socket.AF_UNIX)
        self.s.settimeout(15)
        self.s.connect(path)
        self.f = self.s.makefile("rw")
        self.f.readline()
        self.cmd("qmp_capabilities")

    def cmd(self, name, **args):
        self.f.write(json.dumps({"execute": name, "arguments": args}) + "\n")
        self.f.flush()
        while True:
            r = json.loads(self.f.readline())
            if "return" in r or "error" in r:
                return r


def qga(path, cmd, args=None, timeout=10):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    s.connect(path)
    f = s.makefile("rw")
    sid = int(time.time() * 1000) % 1000000
    f.write(json.dumps({"execute": "guest-sync", "arguments": {"id": sid}}) + "\n")
    f.flush()
    while True:
        r = json.loads(f.readline())
        if r.get("return") == sid:
            break
    f.write(json.dumps({"execute": cmd, "arguments": args or {}}) + "\n")
    f.flush()
    r = json.loads(f.readline())
    s.close()
    return r


def guest_ps(path, script, wait=120):
    r = qga(path, "guest-exec", {"path": "powershell.exe",
                                 "arg": ["-NoProfile", "-Command", script], "capture-output": True}, 30)
    pid = r["return"]["pid"]
    import base64
    for _ in range(wait):
        st = qga(path, "guest-exec-status", {"pid": pid}, 30)["return"]
        if st.get("exited"):
            return base64.b64decode(st.get("out-data", "")).decode(errors="replace").strip()
        time.sleep(1)
    return "TIMEOUT"


def build(win_iso, virtio_iso, out):
    t0 = time.time()
    work = os.environ.get("HXW_WORK") or (out + ".work")
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(os.path.join(work, "unattend"))
    os.makedirs(os.path.join(work, "tpm"))
    for f in FILES:
        shutil.copy(f, os.path.join(work, "unattend"))
    subprocess.run(["xorriso", "-as", "mkisofs", "-quiet", "-V", "UNATTEND", "-J", "-r",
                    "-o", os.path.join(work, "unattend.iso"), os.path.join(work, "unattend")], check=True)
    disk = out + ".part"
    subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", disk, "64G"], check=True)
    vars_fd = os.path.join(work, "vars.fd")
    shutil.copy(first(OVMF_VARS), vars_fd)
    tpm_sock = os.path.join(work, "tpm.sock")
    subprocess.run(["swtpm", "socket", "--tpm2", "--tpmstate", "dir=" + os.path.join(work, "tpm"),
                    "--ctrl", "type=unixio,path=" + tpm_sock, "--daemon"], check=True)
    qmp_sock, qga_sock = os.path.join(work, "qmp.sock"), os.path.join(work, "qga.sock")
    mem, cpus = os.environ.get("HXW_MEM_MB", "4096"), os.environ.get("HXW_CPUS", "4")
    kvm = os.access("/dev/kvm", os.W_OK)
    argv = [
        "qemu-system-x86_64", "-name", "hx-win-test-image",
        "-machine", "q35,smm=on" + (",accel=kvm" if kvm else ""),
        "-global", "driver=cfi.pflash01,property=secure,value=on",
        "-cpu", "host,hv-relaxed,hv-vapic,hv-spinlocks=0x1fff,hv-time" if kvm else "max",
        "-m", mem, "-smp", cpus,
        "-drive", "if=pflash,unit=0,format=raw,readonly=on,file=" + first(OVMF_CODE),
        "-drive", "if=pflash,unit=1,format=raw,file=" + vars_fd,
        "-chardev", "socket,id=chrtpm,path=" + tpm_sock,
        "-tpmdev", "emulator,id=tpm0,chardev=chrtpm", "-device", "tpm-crb,tpmdev=tpm0",
        "-drive", f"file={disk},if=none,id=d0,format=qcow2,cache=unsafe,discard=unmap",
        "-device", "virtio-blk-pci,drive=d0,bootindex=2",
        "-device", "ahci,id=ahci",
        "-drive", f"file={win_iso},if=none,id=cd0,media=cdrom,readonly=on",
        "-device", "ide-cd,drive=cd0,bus=ahci.0,bootindex=1",
        "-drive", f"file={virtio_iso},if=none,id=cd1,media=cdrom,readonly=on",
        "-device", "ide-cd,drive=cd1,bus=ahci.1",
        "-drive", f"file={os.path.join(work, 'unattend.iso')},if=none,id=cd2,media=cdrom,readonly=on",
        "-device", "ide-cd,drive=cd2,bus=ahci.2",
        # user-mode NAT like the proven spike: 24H2's OOBE is happiest with a NIC
        "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0",
        "-chardev", f"socket,id=qga0,path={qga_sock},server=on,wait=off",
        "-device", "virtio-serial-pci",
        "-device", "virtserialport,chardev=qga0,name=org.qemu.guest_agent.0",
        "-device", "qemu-xhci", "-device", "usb-tablet",
        "-vga", "std", "-display", "none",
        "-qmp", f"unix:{qmp_sock},server=on,wait=off",
    ]
    if os.environ.get("HXW_VNC"):
        argv += ["-vnc", os.environ["HXW_VNC"]]
    log = open(os.path.join(work, "qemu.log"), "w")
    q = subprocess.Popen(argv, stdout=log, stderr=subprocess.STDOUT)
    print(f"ok - QEMU started (pid {q.pid}, kvm={kvm})", flush=True)
    # "Press any key to boot from CD or DVD"
    for _ in range(30):
        try:
            Qmp(qmp_sock).cmd("send-key", keys=[{"type": "qcode", "data": "spc"}])
        except OSError:
            pass
        time.sleep(1)
    deadline = t0 + int(os.environ.get("HXW_TIMEOUT_MIN", "120")) * 60
    stage = "agent"
    last = 0
    while time.time() < deadline:
        if q.poll() is not None:
            raise SystemExit(f"FAIL QEMU exited ({q.returncode}): {open(os.path.join(work, 'qemu.log')).read()[-2000:]}")
        try:
            if stage == "agent":
                r = qga(qga_sock, "guest-get-osinfo", timeout=5)
                if "return" in r:
                    print(f"ok - guest agent answers after {(time.time() - t0) / 60:.0f} min: "
                          f"{r['return'].get('pretty-name')} {r['return'].get('kernel-release')}", flush=True)
                    stage = "firstlogon"
            if stage == "firstlogon":
                r = qga(qga_sock, "guest-file-open", {"path": r"C:\hxci-firstlogon.done", "mode": "r"}, timeout=5)
                if "return" in r:
                    qga(qga_sock, "guest-file-close", {"handle": r["return"]}, timeout=5)
                    print(f"ok - first-logon settings applied after {(time.time() - t0) / 60:.0f} min", flush=True)
                    break
        except (OSError, ValueError, KeyError):
            pass
        if time.time() - last > 600:
            last = time.time()
            print(f"... waiting for the {stage} ({(time.time() - t0) / 60:.0f} min)", flush=True)
        time.sleep(15)
    else:
        try:
            Qmp(qmp_sock).cmd("screendump", filename=os.path.join(work, "stuck.ppm"))
        except OSError:
            pass
        q.kill()
        raise SystemExit(f"FAIL install did not finish (stage {stage}); screendump {work}/stuck.ppm")
    time.sleep(30)  # let the agent's own first-boot work settle
    for k, v in {
        "os": "(Get-CimInstance Win32_OperatingSystem).Caption + ' ' + (Get-CimInstance Win32_OperatingSystem).BuildNumber",
        "secure_boot": "Confirm-SecureBootUEFI",
        "tpm": "(Get-Tpm).TpmReady",
        "virtio": "(Get-PnpDevice -PresentOnly | ? { $_.FriendlyName -match 'VirtIO' } | Select -Expand FriendlyName) -join '; '",
        "session": "(query user) -join ' | '",
    }.items():
        try:
            print(f"ok - {k}: {guest_ps(qga_sock, v)}", flush=True)
        except (OSError, KeyError) as e:
            print(f"WARN {k}: {e}", flush=True)
    Qmp(qmp_sock).cmd("system_powerdown")
    try:
        q.wait(timeout=600)
    except subprocess.TimeoutExpired:
        q.kill()
        raise SystemExit("FAIL the guest did not power off within 10 min")
    subprocess.run(["pkill", "-f", "swtpm socket --tpm2 --tpmstate dir=" + os.path.join(work, "tpm")])
    os.replace(disk, out)
    shutil.rmtree(work, ignore_errors=True)
    print(f"PASS Windows test image {out} in {(time.time() - t0) / 60:.0f} min", flush=True)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "key":
        print(key(sys.argv[2], sys.argv[3]))
    elif len(sys.argv) == 5 and sys.argv[1] == "build":
        build(*[os.path.abspath(a) for a in sys.argv[2:]])
    else:
        raise SystemExit(__doc__)
