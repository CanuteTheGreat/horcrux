#!/usr/bin/env python3
"""Debian test image for Horcrux Console CI (C2b, Linux guest path).

    linux-test-image.py OUT.qcow2

Downloads Debian's official `nocloud` image (root logs in on the serial
console without a password; checksum from Debian's SHA512SUMS), boots it
once under QEMU with user-mode NAT, installs FFmpeg (the guest tools'
capture/encode backend) and the QEMU guest agent from Debian's mirror and
powers it off. The Horcrux guest tools are deliberately NOT installed: the
e2e proves that the node installs their .deb through the guest agent.

Env: HXL_RELEASE (default trixie), HXL_DEBIAN_NUM (13), HXL_MIRROR
(https://cloud.debian.org/images/cloud), HXL_CACHE (download cache dir,
default next to OUT).
"""
import hashlib
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.request

REL = os.environ.get("HXL_RELEASE", "trixie")
NUM = os.environ.get("HXL_DEBIAN_NUM", "13")
MIRROR = os.environ.get("HXL_MIRROR", "https://cloud.debian.org/images/cloud")
NAME = f"debian-{NUM}-nocloud-amd64.qcow2"
# throwaway test VM without a network in the e2e; its console logs root in
PASSWORD = "hxci"


def fetch(url, timeout=600):
    req = urllib.request.Request(url, headers={"User-Agent": "horcrux-ci/1.0"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read()


def main(out):
    t0 = time.time()
    cache = os.environ.get("HXL_CACHE") or os.path.dirname(os.path.abspath(out))
    os.makedirs(cache, exist_ok=True)
    base = f"{MIRROR}/{REL}/latest"
    sums = fetch(f"{base}/SHA512SUMS").decode()
    want = next((ln.split()[0] for ln in sums.splitlines() if ln.strip().endswith(NAME)), None)
    if not want:
        raise SystemExit(f"FAIL {NAME} not in {base}/SHA512SUMS")
    src = os.path.join(cache, NAME)
    have = hashlib.sha512(open(src, "rb").read()).hexdigest() if os.path.exists(src) else ""
    if have != want:
        data = fetch(f"{base}/{NAME}", 1200)
        if hashlib.sha512(data).hexdigest() != want:
            raise SystemExit("FAIL checksum of the downloaded Debian image")
        open(src + ".part", "wb").write(data)
        os.replace(src + ".part", src)
    print(f"ok - {NAME} ({want[:16]}...)", flush=True)
    work = out + ".part"
    subprocess.run(["qemu-img", "convert", "-O", "qcow2", src, work], check=True)
    subprocess.run(["qemu-img", "resize", "-q", work, "6G"], check=True)
    ser = out + ".serial.sock"
    log = out + ".serial.log"
    for p in (ser, log):
        if os.path.exists(p):
            os.unlink(p)
    kvm = os.access("/dev/kvm", os.W_OK)
    q = subprocess.Popen([
        "qemu-system-x86_64", "-machine", "q35" + (",accel=kvm" if kvm else ""),
        "-cpu", "host" if kvm else "max", "-m", "1536", "-smp", "2",
        "-drive", f"file={work},if=virtio,format=qcow2,cache=unsafe",
        "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0",
        "-chardev", f"socket,id=s0,path={ser},server=on,wait=off,logfile={log}", "-serial", "chardev:s0",
        "-display", "none", "-vga", "std",
        # systemd credentials over SMBIOS: systemd-firstboot (trixie's
        # nocloud image runs it on the first boot) takes its answers from
        # here instead of prompting on the console
        "-smbios", "type=11,value=io.systemd.credential:firstboot.locale=C.UTF-8",
        "-smbios", "type=11,value=io.systemd.credential:firstboot.keymap=us",
        "-smbios", "type=11,value=io.systemd.credential:firstboot.timezone=UTC",
        "-smbios", f"type=11,value=io.systemd.credential:passwd.plaintext-password.root={PASSWORD}",
        "-smbios", "type=11,value=io.systemd.credential:system.hostname=hxg-deb",
    ], stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    for _ in range(50):
        if os.path.exists(ser):
            break
        time.sleep(0.2)
    s = socket.socket(socket.AF_UNIX)
    s.connect(ser)

    def drain():
        while True:
            try:
                if not s.recv(4096):
                    return
            except OSError:
                return
    threading.Thread(target=drain, daemon=True).start()

    def text():
        return open(log, errors="replace").read()

    def wait(marker, secs):
        end = time.time() + secs
        while time.time() < end:
            if marker in text():
                return True
            if q.poll() is not None:
                return False
            time.sleep(1)
        return False

    def send(line):
        for ch in line:
            s.send(ch.encode())
            time.sleep(0.01)

    if not wait("login:", 300):
        q.kill()
        raise SystemExit("FAIL no login prompt on the serial console: " + text()[-1500:])
    send("root\n")
    if wait("Password:", 15):
        send(PASSWORD + "\n")
    time.sleep(3)
    # one marker per step; the arithmetic keeps the echo of the command
    # itself from matching
    send("export DEBIAN_FRONTEND=noninteractive; apt-get update -q >/dev/null 2>&1; "
         "apt-get install -y -q --no-install-recommends ffmpeg qemu-guest-agent >/tmp/apt.log 2>&1; "
         "echo HXL_$((40+2))_APT $?\n")
    if not wait("HXL_42_APT 0", 900):
        q.kill()
        raise SystemExit("FAIL apt-get install ffmpeg qemu-guest-agent: " + text()[-1500:])
    print("ok - ffmpeg and qemu-guest-agent installed", flush=True)
    # root shell on the graphical console (tty1) so the e2e can type into it
    send("mkdir -p /etc/systemd/system/getty@tty1.service.d; printf '[Service]\\nExecStart=\\n"
         "ExecStart=-/sbin/agetty --autologin root --noclear %%I $TERM\\n' "
         ">/etc/systemd/system/getty@tty1.service.d/autologin.conf; echo HXL_$((40+4))_TTY $?\n")
    wait("HXL_44_TTY 0", 30)
    send("apt-get clean; ffmpeg -hide_banner -version | head -1; "
         "systemctl is-enabled qemu-guest-agent; echo HXL_$((40+3))_DONE\n")
    wait("HXL_43_DONE", 60)
    send("systemctl poweroff\n")
    try:
        q.wait(timeout=180)
    except subprocess.TimeoutExpired:
        q.kill()
        raise SystemExit("FAIL the guest did not power off")
    os.replace(work, out)
    for p in (ser, log):
        if os.path.exists(p):
            os.unlink(p)
    print(f"PASS Debian test image {out} in {time.time() - t0:.0f} s", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    main(os.path.abspath(sys.argv[1]))
