#!/usr/bin/env python3
"""GNOME desktop VM gate: install a REAL desktop ISO into a VM, boot the installed disk with a virtual display + QMP keyboard, and run the
attach-mode GNOME tests (tests/*-vm-test.py) against it. One install serves every test.

    gnome-vm-gate.py --iso X.iso --out DIR [--init openrc|systemd] [--tests network-notify[,session-lifecycle]] [--project horcrux]

1. qemu-test.py --flow auto --install-only --answer-file tests/data/gnome-session-answers.toml --keep-disks: unattended desktop install (ext4, no autologin, login
   user "owner") and the written-disk checks (the harness cannot log in on the console with this answer file's own root password);
2. QEMU boots the kept disk (virtio-vga + USB keyboard/tablet, QMP socket, user-mode net with an ssh forward);
3. each test in --tests runs against it (they log in through the real GDM greeter with QMP key events when nobody is logged in).

Tests: network-notify (network-notify-vm-test.py: horcrux-gnome-defaults >= 1-r14 network notifications) and session-lifecycle
(gnome-session-lifecycle-test.py --cycles 1: greeter logins/logouts, screen sharing, autostart identity). Default: network-notify.
Exit 0 = every selected test passed. Evidence (logs, screenshots, result json) goes to OUT/<test>/; the VM disk is deleted at the end.
"""
import argparse
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
TESTS = {
    "network-notify": ("tests/network-notify-vm-test.py", []),
    "session-lifecycle": ("tests/gnome-session-lifecycle-test.py", ["--cycles", "1"]),
}
BOOT_CPU = os.environ.get("QEMU_TEST_CPU", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16")


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def ssh(key, port, cmd, timeout=30):
    r = subprocess.run(["ssh", "-i", key, "-p", str(port), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR",
                        "-o", "ConnectTimeout=8", "-o", "BatchMode=yes", "root@127.0.0.1", cmd], capture_output=True, text=True, timeout=timeout)
    return r.returncode, (r.stdout + r.stderr).strip()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--project", default="horcrux")
    ap.add_argument("--init", choices=["openrc", "systemd"], default="openrc")
    ap.add_argument("--tests", default="network-notify", help="comma list of: " + ", ".join(TESTS))
    ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--smp", type=int, default=4)
    ap.add_argument("--boot-wait", type=int, default=420, help="seconds for sshd of the installed system to answer")
    ap.add_argument("--qemu", default="qemu-system-x86_64")
    ap.add_argument("--pre-script", default="", help="a shell script run as root in the booted guest before the tests (a local proof of a package that the ISO under test does not carry yet)")
    a = ap.parse_args()
    names = [t for t in a.tests.split(",") if t]
    bad = [t for t in names if t not in TESTS]
    if bad:
        sys.exit(f"unknown test(s) {bad}; known: {sorted(TESTS)}")
    out = os.path.abspath(a.out)
    inst = os.path.join(out, "install")
    os.makedirs(inst, exist_ok=True)

    # 1. unattended desktop install + the harness's own checks
    cmd = [sys.executable, "-u", os.path.join(HERE, "qemu-test.py"), "--iso", a.iso, "--project", a.project, "--init", a.init, "--mode", "bios",
           "--media", "cdrom", "--flow", "auto", "--fs", "ext4", "--answer-file", os.path.join(HERE, "tests/data/gnome-session-answers.toml"),
           "--keep-disks", "--install-only", "--mem", str(a.mem), "--out", inst]
    print("[gate] installing:", " ".join(cmd), flush=True)
    rc = subprocess.call(cmd)
    if rc != 0:
        print(f"[FAIL] gnome-vm-gate install: qemu-test.py exited {rc}", flush=True)
        return 1
    disk, key = os.path.join(inst, "target.qcow2"), os.path.join(inst, "test_key")
    if not (os.path.exists(disk) and os.path.exists(key)):
        print("[FAIL] gnome-vm-gate install: no kept disk / ssh key in " + inst, flush=True)
        return 1

    # 2. boot the installed disk. Sockets live in a short temp dir (sun_path limit 108 bytes).
    sock_dir = tempfile.mkdtemp(prefix="gvg")
    qmp = os.path.join(sock_dir, "qmp.sock")
    port = free_port()
    qargs = [a.qemu, "-machine", "pc", "-cpu", BOOT_CPU, "-m", str(a.mem), "-smp", str(a.smp), "-drive", f"file={disk},if=virtio,format=qcow2",
             "-netdev", f"user,id=n0,hostfwd=tcp:127.0.0.1:{port}-:22", "-device", "virtio-net-pci,netdev=n0", "-device", "virtio-vga,xres=1280,yres=800",
             "-device", "qemu-xhci,id=xhci", "-device", "usb-kbd,bus=xhci.0", "-device", "usb-tablet,bus=xhci.0",
             "-qmp", f"unix:{qmp},server=on,wait=off", "-display", "none", "-no-reboot"]
    if os.path.exists("/dev/kvm") and os.access("/dev/kvm", os.W_OK):
        qargs[1:1] = ["-enable-kvm"]
    else:
        print("[WARN] no /dev/kvm: the GNOME session will be very slow", flush=True)
    qlog = open(os.path.join(out, "boot-qemu.log"), "w")
    q = subprocess.Popen(qargs, stdout=qlog, stderr=subprocess.STDOUT)
    failed = []
    try:
        end = time.time() + a.boot_wait
        up = False
        while time.time() < end and q.poll() is None:
            try:
                if ssh(key, port, "true", 15)[0] == 0:
                    up = True
                    break
            except subprocess.TimeoutExpired:
                pass
            time.sleep(5)
        if not up:
            print(f"[FAIL] gnome-vm-gate boot: installed system did not answer ssh within {a.boot_wait} s (qemu exit {q.poll()})", flush=True)
            return 1
        print(f"[PASS] gnome-vm-gate boot: installed {a.init} desktop answers ssh ({int(time.time() - end + a.boot_wait)} s)", flush=True)
        # GDM must be up before a test types the password at the greeter
        for _ in range(60):
            if "gdm" in ssh(key, port, "ps -eo comm | grep -x -e gdm -e gdm-wayland-ses -e gdm-session-wor | head -1")[1]:
                break
            time.sleep(5)
        if a.pre_script:
            # idempotent script; a package merge can restart sshd/dbus under the connection (rc 255 "closed by remote host"): run it again
            for attempt in range(1, 4):
                try:
                    r = subprocess.run(["ssh", "-i", key, "-p", str(port), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
                                        "-o", "LogLevel=ERROR", "-o", "ServerAliveInterval=15", "root@127.0.0.1", "sh -s"], stdin=open(a.pre_script),
                                       capture_output=True, text=True, timeout=1500)
                except subprocess.TimeoutExpired:
                    print(f"[WARN] gnome-vm-gate pre-script attempt {attempt}: timed out", flush=True)
                    continue
                print(f"[{'PASS' if r.returncode == 0 else 'WARN'}] gnome-vm-gate pre-script attempt {attempt} rc={r.returncode}: {(r.stdout + r.stderr).strip()[-1500:]}", flush=True)
                if r.returncode == 0:
                    break
                time.sleep(10)
            else:
                print("[FAIL] gnome-vm-gate pre-script failed 3 times", flush=True)
                return 1
        # 3. the tests
        for name in names:
            script, extra = TESTS[name]
            tdir = os.path.join(out, name)
            os.makedirs(tdir, exist_ok=True)
            tcmd = [sys.executable, "-B", "-u", os.path.join(HERE, script), "--qmp", qmp, "--port", str(port), "--key", key, "--user", "owner", "--out", tdir] + extra
            print(f"[gate] {name}: {' '.join(tcmd)}", flush=True)
            with open(os.path.join(out, f"test-{name}.log"), "w") as lf:
                p = subprocess.Popen(tcmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace")
                for line in p.stdout:
                    lf.write(line)
                    if line.startswith(("[PASS]", "[FAIL]", "RESULT", "Traceback")) or "Error" in line[:60]:
                        print(f"  {name}: {line.rstrip()[:260]}", flush=True)
                p.wait()
            print(f"[{'PASS' if p.returncode == 0 else 'FAIL'}] gnome-vm-gate {name} (exit {p.returncode})", flush=True)
            if p.returncode != 0:
                failed.append(name)
    finally:
        try:
            q.send_signal(signal.SIGKILL)
            q.wait(timeout=20)
        except Exception:
            pass
        qlog.close()
        shutil.rmtree(sock_dir, ignore_errors=True)
        for f in ("target.qcow2", "answers.img", "OVMF_VARS.fd"):
            try:
                os.remove(os.path.join(inst, f))
            except OSError:
                pass
    print(f"RESULT gnome-vm-gate {'FAIL: ' + ', '.join(failed) if failed else 'PASS'} ({', '.join(names)})", flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
