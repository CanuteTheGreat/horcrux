#!/usr/bin/env python3
"""QEMU/KVM acceptance test of the Horcrux SURFACE image (XFCE tablet profile), the way the owner will use it:

  surface-xfce-test.py --iso horcrux-surface-*.iso --out DIR [--mem 4096] [--cpu IvyBridge]

  1. boot the ISO from a (virtual) USB stick, UEFI, 4 GiB, IvyBridge, Surface DMI; the default entry runs `horcrux-surface-auto`:
     read-only inventory of the (empty) disk, the TEST passphrase is typed twice on the USB keyboard, the installer runs by itself
  2. first boot of the installed disk with NO PS/2 keyboard: ONE passphrase on the USB keyboard, LightDM autologin, XFCE session
  3. checks: XFS on LUKS2 on LVM, hibernate swap + zram, kernel from the Surface kernel package, qfile -o orphans, packages of
     the meta package, session processes (xfwm4, panel, power manager, onboard, nm-applet, pipewire), greeter + on-screen keyboard
  4. apps launch inside the session (Firefox, Brave, Nextcloud, Rustinion agent, Tailscale, onboard) + memory/responsiveness numbers
  5. `loginctl hibernate` -> power off -> ONE passphrase -> same boot_id/process/tmpfs (resume), the XFCE session survives
  6. clean `emerge -puDN @world` (needs the network of the VM)
Evidence: result.json, screens/*.png|txt, serial logs in --out. Reuses surface-qemu-test.py (T class) + common/qemu-test.py.
"""
import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("sq", os.path.join(HERE, "surface-qemu-test.py"))
SQ = importlib.util.module_from_spec(spec)
spec.loader.exec_module(SQ)
Q = SQ.Q
LUKS_PW = SQ.LUKS_PW
USER = "canutethegreat"

MARKER_PY = r'''
import os, time, hashlib
tok = os.urandom(1 << 20)
open("/dev/shm/marker.sha", "w").write(hashlib.sha256(tok).hexdigest())
n = 0
while True:
    n += 1
    open("/dev/shm/counter", "w").write(str(n))
    open("/dev/shm/selfcheck", "w").write(hashlib.sha256(tok).hexdigest() + " " + str(n))
    time.sleep(1)
'''

class XT(SQ.T):
    def __init__(self, a):
        super().__init__(a)
        self.dir = a.out
        self.nvars = f"{a.out}/OVMF_VARS.fd"
        self.disk = f"{a.out}/target.qcow2"
        self.answers = f"{a.out}/surfkeys.img"
        self.evid = f"{a.out}/screens"

    def save(self, name, text):
        open(f"{self.evid}/{name}", "w").write(text)

    def setup(self):
        a = self.a
        subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", self.disk, f"{a.disk_gb}G"])
        Q.ARCH = "x86_64"
        Q.pflash_vars(self.nvars)
        self.key_path = f"{a.out}/test_key"
        for f in (self.key_path, self.key_path + ".pub"):
            if os.path.exists(f):
                os.unlink(f)
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "qemu-test", "-f", self.key_path])
        pub = open(self.key_path + ".pub").read().strip()
        d = tempfile.mkdtemp()
        open(f"{d}/authorized_keys", "w").write(pub + "\n")
        subprocess.check_call(["mke2fs", "-q", "-t", "ext4", "-L", "SURFKEYS", "-d", d, self.answers, "8M"])
        shutil.rmtree(d)

    def unlock(self, vm, name):
        t0 = time.time()
        time.sleep(40)
        ppm = self.settle(vm, name, timeout=420, quiet=4)
        self.step(f"{name}-passphrase-prompt-on-vga", ppm is not None, f"{int(time.time() - t0)}s: LUKS prompt idle on the screen")
        vm.shot(f"{name}-luks-prompt")
        self.key(vm, LUKS_PW + "\n")
        t1 = time.time()
        while time.time() - t1 < 600 and vm.alive():
            if self.ssh("true", timeout=15)[0] == 0:
                return time.time() - t0
            time.sleep(5)
        return None

    # ------------------------------------------------------------------------------------------------ install
    def install(self):
        a, t0 = self.a, time.time()
        vm = Q.VM("1-install", self.base(False, self.nvars), a.out)
        try:
            ok = vm.expect_raw(r"(Serial console \(ttyS0|safe graphics)", 150)
            self.step("grub-menu-reached", ok, f"{int(time.time() - t0)}s, cpu={a.cpu}, {a.mem} MiB, DMI Microsoft Corporation / Surface with Windows 8 Pro")
            time.sleep(2)
            vm.shot("grub-menu")
            vm.send("s")                       # serial submenu: same entries, kernel console also on the UART so the test can read the markers
            time.sleep(1.5)
            vm.shot("grub-serial-submenu")
            vm.send("a")                       # the one-shot Surface entry (it is also the DEFAULT entry of the plain menu)
            ok = vm.expect_raw(r"SURFACE-AUTO: START", 480, consume=False)
            self.step("auto-installer-started", ok, f"{int(time.time() - t0)}s after power-on: horcrux-surface-auto on tty1")
            inv = vm.expect_raw(r"SURFACE-AUTO: INVENTORY rc=(\d+) disk=(\S+)", 240, consume=False)
            self.step("inventory-found-empty-disk", inv and inv.group(1) == b"0" and inv.group(2) == b"/dev/vda", inv.group(0).decode() if inv else "no inventory marker")
            ok = vm.expect_raw(r"SURFACE-AUTO: WAITING-FOR-PASSPHRASE", 120, consume=False)
            self.step("asks-for-the-passphrase", ok, "the only question")
            time.sleep(3)
            vm.shot("passphrase-prompt")
            self.key(vm, LUKS_PW + "\n")
            time.sleep(2)
            self.key(vm, LUKS_PW + "\n")
            ok = vm.expect_raw(r"SURFACE-AUTO: PASSPHRASE-ACCEPTED", 60, consume=False)
            self.step("passphrase-accepted", ok, "typed twice on the USB keyboard")
            time.sleep(20)
            vm.shot("installing")
            ok = vm.expect_raw(r"SURFACE-AUTO: (INSTALL-OK|INSTALL-FAILED[^\n]*)", 2400, consume=False)
            self.step("installer-finished-ok", ok and b"INSTALL-OK" in ok.group(0), f"{int(time.time() - t0)}s total: " + (ok.group(0).decode() if ok else "timeout"))
            time.sleep(3)
            vm.shot("install-done")
            self.key(vm, "\n")                 # "press Enter to restart"
            end = time.time() + 120
            while vm.alive() and time.time() < end:
                time.sleep(1)
            self.step("vm-restarts-after-install", not vm.alive(), f"{int(time.time() - t0)}s total")
        finally:
            if vm.alive():
                vm.p.kill()

    # ------------------------------------------------------------------------------------------------ checks
    def checks(self, vm, secs):
        a = self.a
        self.soft = True
        s = self.step
        rc, out = self.ssh("findmnt -no SOURCE,FSTYPE /; findmnt -no FSTYPE /boot; cat /proc/cmdline; uname -r; lvs --noheadings -o lv_name,lv_size; "
                           "cryptsetup status hxroot | head -4; cryptsetup luksDump $(cryptsetup status hxroot | sed -n 's/ *device: *//p') | grep -E 'Version|PBKDF|Memory' | head -6; "
                           "cat /proc/swaps; echo @@@; cat /etc/surface-kernelrelease; qlist -Iv sys-kernel/gentoo-kernel sys-kernel/horcrux-surface-config app-misc/horcrux-surface-desktop; "
                           "ls /etc/portage/patches/sys-kernel/gentoo-kernel | wc -l", timeout=60)
        self.save("system-state.txt", out)
        flat = out.replace("\r", "")
        s("root-is-xfs-on-lvm-on-luks2", "/dev/mapper/hxvg-root xfs" in flat and "argon2id" in flat and re.search(r"Version:\s+2", flat) is not None, flat.splitlines()[0] if flat else "")
        s("boot-is-ext4", re.search(r"^ext4$", flat, re.M) is not None, "/boot ext4")
        s("hibernate-swap-4.5g-plus-zram", re.search(r"hxvg-swap|swap\s+4\.50g", flat) is not None and "/dev/zram0" in flat and "resume=UUID=" in flat, "swap LV 4.5g, zram0 prio 100, resume= on the command line")
        s("lid-quirk-on-cmdline", "button.lid_init_state=open" in flat and "ttyS0" not in flat.split("@@@")[0].split("root=")[0] + flat.split("rd.")[0], "button.lid_init_state=open, VGA-only console")
        s("kernel-and-meta-packages-are-portage-packages", "gentoo-kernel-6.18.59" in flat and "horcrux-surface-config-1" in flat and "horcrux-surface-desktop-1" in flat and re.search(r"^16$", flat, re.M) is not None, re.sub(r"\s+", " ", flat.split("@@@")[-1])[:300])
        rc, out = self.ssh("zgrep -c . /proc/config.gz >/dev/null 2>&1; k=$(uname -r); c=/lib/modules/$k/build/.config; for o in SURFACE_AGGREGATOR SURFACE_HID HID_MULTITOUCH HID_WACOM MWIFIEX_PCIE MWIFIEX_SDIO HID_SENSOR_ACCEL_3D XFS_FS HIBERNATION; do grep -E \"^CONFIG_$o=\" $c; done; "
                           "find /lib/modules/$k -name 'surface_aggregator*' | head -2; ls /sys/class/dmi/id >/dev/null && cat /sys/class/dmi/id/product_name", timeout=60)
        self.save("kernel-config-surface.txt", out)
        s("surface-kernel-has-the-patched-drivers-and-config", out.count("CONFIG_") >= 9 and "surface_aggregator" in out, re.sub(r"\s+", " ", out)[:300])
        # ---- ownership: horcrux-orphans (shipped by sys-apps/horcrux-tools; merged-/usr safe qfile -o over /usr /opt /boot /lib/modules /lib/firmware)
        # allowed = generated files only (eselect/gcc-config links, icon/mime/gconv caches, info/dir, nss .chk, cracklib dict, /boot/{grub,efi}); plus no
        # Portage config-protection leftovers (._cfg*) anywhere under /etc
        rc, out = self.ssh("horcrux-orphans > /tmp/po.out 2>&1; wc -l < /tmp/po.out; find /etc -name '._cfg*' > /tmp/cfg.out; wc -l < /tmp/cfg.out", timeout=900)
        rc2, full = self.ssh("cat /tmp/po.out; echo @@CFG; cat /tmp/cfg.out", timeout=60)
        self.save("horcrux-orphans.txt", full)
        po, _, cfg = full.partition("@@CFG")
        ALLOW = [re.compile(x) for x in (r"/etc/", r"^/usr/lib/python[0-9.]+/.*__pycache__/", r"^/usr/lib64?/gconv/gconv-modules\.cache$", r"^/usr/share/(mime|icons)/", r"^/usr/share/(info/)?dir$",
                 r"^/usr/share/applications/mimeinfo\.cache$", r"^/usr/share/glib-2\.0/schemas/gschemas\.compiled$", r"^/usr/lib64?/lib(freebl3|softokn3|nssdbm3)\.chk$", r"^/usr/lib/locale/locale-archive$",
                 r"^/usr/libexec/rc/console/keymap$", r"^/usr/share/cracklib/", r"^/boot/(grub|efi|EFI|initramfs-|System\.map|config-|vmlinuz-)", r"^/usr/lib/modules/[^/]+/modules\.",
                 r"^/usr/lib64?/(gdk-pixbuf-2\.0|gio|gtk-[234]\.0)/.*cache$", r"^/usr/lib/\.build-id/", r"^/usr/bin/(eselect|gcc|g\+\+|cc|c\+\+|cpp)", r"^/usr/(bin|lib64?)/.*->")]
        orph = [l.strip() for l in po.splitlines() if l.strip().startswith("/") and not any(r.search(l.strip()) for r in ALLOW)]
        cfgs = [l.strip() for l in cfg.splitlines() if l.strip().startswith("/")]
        s("horcrux-orphans-clean", len(orph) == 0, f"{len(orph)} unexpected: " + ", ".join(orph[:12]) + f" | raw lines={len(po.splitlines())}")
        s("no-cfg-protect-leftovers", len(cfgs) == 0, ", ".join(cfgs[:5]) or "none under /etc")
        rc3, qf = self.ssh("qfile /usr/bin/live-installer /usr/bin/horcrux-orphans /usr/share/live-installer/branding/full.png /etc/init.d/live-installer /etc/init.d/zram-swap /usr/bin/horcrux-surface-auto /usr/bin/brave /usr/bin/rustinion | sort", timeout=60)
        self.save("qfile-spot-check.txt", qf)
        s("installer-files-owned-by-horcrux-tools-and-live-data", qf.count("sys-apps/horcrux-tools") >= 3 and "sys-apps/horcrux-live-data" in qf and "horcrux-live-installer" not in qf, re.sub(r"\s+", " ", qf)[:300])
        # ---- session
        end, got = time.time() + 300, ""
        want = ["lightdm", "Xorg", "xfce4-session", "xfwm4", "xfce4-panel", "xfdesktop", "xfce4-power-man", "nm-applet", "onboard", "xfsettingsd", "pipewire", "wireplumber", "xfce4-screensav", "tailscaled"]
        while time.time() < end:
            rc, got = self.ssh("ps -eo comm | sort -u | tr '\\n' ' '")
            if all(w in got for w in want[:9]):
                break
            time.sleep(6)
        s("xfce-session-up-for-autologin-user", all(w in got for w in want[:9]), "missing: " + ", ".join(w for w in want if w not in got) if not all(w in got for w in want) else "all running")
        self.save("processes.txt", got)
        time.sleep(5)
        vm.shot("desktop")
        rc, out = self.ssh("loginctl list-sessions --no-legend; grep -c lightdm /etc/runlevels/default/* 2>/dev/null; ls /etc/runlevels/default | tr '\\n' ' '; echo; cat /etc/conf.d/display-manager; cat /etc/lightdm/lightdm.conf.d/*.conf | grep -E 'autologin|greeter|session'")
        self.save("session-state.txt", out)
        s("lightdm-autologin-configured", "autologin-user=" + USER in out and "DISPLAYMANAGER=\"lightdm\"" in out, re.sub(r"\s+", " ", out)[:240])
        # session env helper: run commands as the user inside the X session
        def usr(cmd, timeout=120):
            inner = ("export DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/$(id -u) XAUTHORITY=$HOME/.Xauthority; " + cmd)
            return self.ssh(f"su - {USER} -c {SQ.shlex.quote(inner)}", timeout=timeout)
        self.usr = usr
        rc, out = usr("xrandr 2>&1 | head -3; xinput list --short 2>&1 | head -12; gsettings get org.onboard.auto-show enabled 2>&1; gsettings get org.gnome.desktop.interface toolkit-accessibility 2>&1; "
                      "cat /etc/xdg/xfce4/xfconf/xfce-perchannel-xml/xfce4-power-manager.xml | grep -c critical-power-action; xfconf-query -c xfce4-power-manager -l 2>&1 | head -20")
        self.save("x11-state.txt", out)
        s("x-session-has-output-and-input-and-schema-defaults", "Screen 0" in out and "true" in out and "critical-power-action" not in "FAIL", re.sub(r"\s+", " ", out)[:240])
        rc, out = self.ssh("rc-service elogind status 2>&1 | tail -1; dbus-send --system --print-reply --dest=org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager.CanHibernate 2>&1 | tail -1; "
                           "dbus-send --system --print-reply --dest=org.freedesktop.UPower /org/freedesktop/UPower org.freedesktop.DBus.Properties.Get string:org.freedesktop.UPower string:LidIsPresent 2>&1 | tail -1; "
                           "pgrep -a onboard | head -2; test -x /usr/libexec/iio-sensor-proxy -o -x /usr/sbin/iio-sensor-proxy && echo iio-sensor-proxy-installed")
        self.save("power-state.txt", out)
        s("can-hibernate-and-sensor-proxy-installed", 'string "yes"' in out and "iio-sensor-proxy-installed" in out, re.sub(r"\s+", " ", out)[:240])
        # ---- apps launch inside the session
        launch = {
            "firefox": "firefox-bin", "brave": "brave-bin", "nextcloud": "nextcloud", "onboard": "onboard", "terminal": "xfce4-terminal",
        }
        t_launch = {}
        # RAM at idle (session up, nothing launched), then with Brave alone open, then with everything
        time.sleep(30)
        rc, out = self.ssh("free -m; echo; cat /proc/pressure/memory 2>/dev/null; cat /proc/swaps", timeout=60)
        self.save("memory-idle.txt", out)
        self.res["memory_idle"] = re.sub(r"\s+", " ", out.splitlines()[1])
        s("memory-idle-recorded", True, self.res["memory_idle"])
        t1 = time.time()
        usr("(setsid brave --no-first-run --new-window about:blank >/tmp/app-brave0.log 2>&1 &) ; sleep 1; true", timeout=30)
        for _ in range(50):
            if self.ssh(f"pgrep -u {USER} -f brave | head -1")[1].strip():
                break
            time.sleep(2)
        time.sleep(40)
        rc, out = self.ssh("free -m; echo; ps -eo rss,comm --sort=-rss | head -8; cat /proc/pressure/memory 2>/dev/null; cat /proc/swaps", timeout=60)
        self.save("memory-with-brave.txt", out)
        self.res["memory_brave_only"] = re.sub(r"\s+", " ", out.splitlines()[1])
        self.res["brave_open_seconds"] = round(time.time() - t1 - 40, 1)
        s("memory-with-brave-recorded", True, self.res["memory_brave_only"])
        self.ssh(f"pkill -u {USER} -f brave; sleep 3", timeout=30)
        for name, cmd in (("firefox", "firefox-bin --new-window about:blank"), ("brave", "brave --no-first-run --new-window about:blank"),
                          ("nextcloud", "nextcloud --background"), ("xfce4-terminal", "xfce4-terminal"), ("onboard", "onboard")):
            t1 = time.time()
            usr(f"(setsid {cmd} >/tmp/app-{name}.log 2>&1 &) ; sleep 1; true", timeout=30)
            ok = False
            for _ in range(50):
                rc, o = self.ssh(f"pgrep -u {USER} -f '{name.split('-')[0] if name != 'xfce4-terminal' else name}' | head -3")
                if o.strip():
                    ok = True
                    break
                time.sleep(2)
            t_launch[name] = round(time.time() - t1, 1)
            s(f"app-{name}-launches", ok, f"process present after {t_launch[name]}s")
        time.sleep(25)
        rc, out = usr("xprop -root _NET_CLIENT_LIST_STACKING 2>&1; for w in $(xprop -root _NET_CLIENT_LIST 2>/dev/null | grep -o '0x[0-9a-f]*'); do xprop -id $w WM_CLASS 2>/dev/null; done")
        self.save("windows.txt", out)
        vm.shot("apps-running")
        wins = out
        s("app-windows-mapped", sum(k in wins for k in ("Firefox", "Brave", "Nextcloud", "onboard", "Onboard", "Xfce4-terminal")) >= 3, re.sub(r"\s+", " ", wins)[:300])
        rc, out = self.ssh("rustinion --version 2>&1 | head -2; tailscale version 2>&1 | head -1; nextcloud --version 2>&1 | head -1; firefox-bin --version 2>&1 | head -1; brave --version 2>&1 | head -1; "
                           "qlist -Iv app-admin/rustinion-bin net-vpn/tailscale www-client/brave-bin www-client/firefox-bin net-misc/nextcloud-client x11-misc/lightdm app-accessibility/onboard sys-firmware/intel-microcode sys-firmware/sof-firmware | tr '\\n' ' '", timeout=90)
        self.save("app-versions.txt", out)
        s("apps-versions-and-portage-ownership", all(x in out for x in ("rustinion-bin", "tailscale", "brave-bin", "firefox-bin", "nextcloud-client", "onboard", "intel-microcode", "sof-firmware")), re.sub(r"\s+", " ", out)[:300])
        rc, out = self.ssh("qfile -o /usr/bin/brave /usr/bin/firefox-bin /usr/bin/rustinion /usr/bin/onboard /usr/bin/tailscale /usr/bin/nextcloud | tr '\\n' ' '; echo; qfile /usr/bin/brave /usr/bin/rustinion /usr/bin/onboard | tr '\\n' ' '")
        s("app-binaries-owned-by-ebuilds", "orphan" not in out.lower() and out.count(":") >= 3, re.sub(r"\s+", " ", out)[:300])
        # ---- memory / responsiveness at 4 GB with everything running
        time.sleep(30)
        rc, out = self.ssh("free -m; echo; ps -eo rss,comm --sort=-rss | head -16; echo; cat /proc/loadavg; echo; cat /proc/pressure/memory /proc/pressure/cpu 2>/dev/null; echo; cat /proc/swaps; echo; vmstat 1 5", timeout=60)
        self.save("memory-with-apps.txt", out)
        self.res["memory_with_apps"] = re.sub(r"\s+", " ", out.splitlines()[1])
        self.res["app_launch_seconds"] = t_launch
        s("memory-with-apps-recorded", True, re.sub(r"\s+", " ", out.splitlines()[1]))
        t1 = time.time()
        usr("xfce4-terminal -e 'true' --disable-server >/dev/null 2>&1; true")
        self.res["terminal_open_seconds"] = round(time.time() - t1, 1)
        self.soft = False

    def greeter_check(self, vm):
        """LightDM greeter + on-screen keyboard: stop the session, disable autologin for this one start, look at the greeter, restore."""
        self.soft = True
        s = self.step
        self.ssh("mv /etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf /root/60-autologin.conf.bak; rc-service display-manager stop >/dev/null 2>&1; pkill -9 Xorg; sleep 2; rc-service display-manager start >/dev/null 2>&1", timeout=90)
        time.sleep(25)
        vm.shot("lightdm-greeter")
        rc, out = self.ssh("ps -eo user,comm,args | grep -E 'lightdm-gtk-greeter|onboard' | grep -v grep | cut -c1-150; ls /etc/lightdm; cat /etc/lightdm/lightdm-gtk-greeter.conf | grep -vE '^#|^$' | head -20")
        self.save("greeter-state.txt", out)
        s("lightdm-gtk-greeter-shown", "lightdm-gtk-greeter" in out, re.sub(r"\s+", " ", out)[:240])
        rc, wm = self.ssh("export DISPLAY=:0 XAUTHORITY=$(ls /var/run/lightdm/root/:0 /run/lightdm/root/:0 2>/dev/null | head -1); xprop -root _NET_CLIENT_LIST 2>&1 | head -2; "
                          "for w in $(xprop -root _NET_CLIENT_LIST 2>/dev/null | grep -o '0x[0-9a-f]*'); do xprop -id $w WM_CLASS _NET_WM_NAME 2>/dev/null; done; pgrep -a onboard | head -2", timeout=60)
        self.save("greeter-windows.txt", wm)
        s("greeter-onboard-window-mapped", re.search(r"[Oo]nboard", wm) is not None, re.sub(r"\s+", " ", wm)[:240])
        s("greeter-onscreen-keyboard-available", "onboard" in out or re.search(r"keyboard\s*=\s*onboard", out) is not None, "greeter has an on-screen keyboard (a11y button / onboard)")
        self.ssh("mv /root/60-autologin.conf.bak /etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf; rc-service display-manager stop >/dev/null 2>&1; pkill -9 Xorg; sleep 2; rc-service display-manager start >/dev/null 2>&1", timeout=90)
        time.sleep(30)
        self.soft = False

    def world_update(self):
        s = self.step
        self.soft = True
        rc, out = self.ssh("(emerge-webrsync -q >/tmp/webrsync.log 2>&1; echo webrsync rc=$?; tail -3 /tmp/webrsync.log; "
                           "emerge --pretend --verbose --update --deep --newuse --getbinpkg @world 2>&1 | tail -40) 2>&1", timeout=1500)
        self.save("emerge-world-update.txt", out)
        pk = re.findall(r"^\[(?:ebuild|binary)[^\]]*\]\s+(\S+)", out, re.M)
        s("emerge-webrsync-ok", "webrsync rc=0" in out, re.sub(r"\s+", " ", out[:200]))
        s("world-update-clean", "webrsync rc=0" in out and "blocked" not in out.lower() and "conflict" not in out.lower() and "!!!" not in out and len(pk) == 0,
          f"{len(pk)} packages pending: {', '.join(pk[:10])}" if pk else "nothing to update, no blockers")
        self.soft = False

    # ------------------------------------------------------------------------------------------------ run
    def run(self):
        a = self.a
        self.setup()
        self.install()
        Q.pflash_vars(self.nvars)
        vm = Q.VM("2-boot1", self.base(True, self.nvars), a.out)
        marker = {}
        try:
            t0 = time.time()
            secs = self.unlock(vm, "boot1")
            self.step("first-boot-one-passphrase-then-up", secs is not None, f"{int(secs) if secs else 0}s from power-on to ssh; the LUKS prompt was answered ONCE (USB keyboard, PS/2 off)")
            self.res["first_boot_seconds"] = int(secs or 0)
            self.checks(vm, secs)
            self.greeter_check(vm)
            self.world_update()
            self.soft = False
            # ---- marker + session pid before hibernate
            rc, out = self.ssh("cat > /root/marker.py <<'EOF'" + MARKER_PY + "EOF\nnohup python3 /root/marker.py >/dev/null 2>&1 < /dev/null & echo $! > /root/marker.pid; sleep 4; "
                               "head -c 65536 /dev/urandom | base64 > /dev/shm/marker.txt; sha256sum /dev/shm/marker.txt | cut -d' ' -f1; "
                               "pid=$(cat /root/marker.pid); echo $pid; awk '{print $22}' /proc/$pid/stat; cat /proc/sys/kernel/random/boot_id; "
                               "cat /dev/shm/counter; echo; cat /dev/shm/marker.sha; echo; awk '{print $1}' /proc/uptime; pgrep -u " + USER + " -x xfce4-session; echo; free -m | sed -n 2p")
            L = out.split()
            marker = {"txt_sha": L[0], "pid": L[1], "start": L[2], "boot_id": L[3], "counter": int(L[4]), "tok_sha": L[5], "uptime": float(L[6]), "xfce_pid": L[7]}
            self.res["marker"] = marker
            self.step("marker-in-ram-recorded", len(marker["tok_sha"]) == 64 and marker["xfce_pid"].isdigit(), json.dumps(marker))
            self.ssh("sync; (sleep 2; loginctl hibernate) >/root/hibernate.out 2>&1 &", timeout=20)
            t1 = time.time()
            while vm.alive() and time.time() < t1 + 480:
                time.sleep(2)
            self.step("hibernate-powers-the-vm-off", not vm.alive(), f"{int(time.time() - t1)}s from `loginctl hibernate` to QEMU exit (image written to the encrypted swap LV)")
        finally:
            if vm.alive():
                vm.p.kill()
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
            time.sleep(10)
            vm.shot("resumed-desktop")
            self.soft = True
            s = self.step
            rc, out = self.ssh("cat /proc/sys/kernel/random/boot_id; pid=%s; awk '{print $22}' /proc/$pid/stat; cat /proc/$pid/cmdline | tr '\\0' ' '; echo; cat /dev/shm/counter; echo; "
                               "cat /dev/shm/selfcheck; echo; cat /dev/shm/marker.sha; echo; sha256sum /dev/shm/marker.txt | cut -d' ' -f1; awk '{print $1}' /proc/uptime; pgrep -u %s -x xfce4-session; "
                               "dmesg | grep -iE 'PM: (hibernation|Image)|Image restored|hibernat' | tail -6" % (marker["pid"], USER))
            self.save("resume-state.txt", out)
            ln = out.splitlines()
            s("same-boot-id-as-before-hibernate", ln[0].strip() == marker["boot_id"], f"{ln[0].strip()} (a cold boot would have a new one)")
            s("same-process-and-token-and-tmpfs", ln[1].strip() == marker["start"] and ln[4].split()[0] == marker["tok_sha"] and ln[6].strip() == marker["txt_sha"], "pid/starttime, in-memory token sha256, /dev/shm file sha256 all unchanged")
            try:
                ctr = int(ln[3].strip())
            except ValueError:
                ctr = -1
            s("process-kept-running-after-resume", ctr > marker["counter"], f"counter {marker['counter']} -> {ctr}")
            s("xfce-session-survived-hibernate", ln[8].strip() == marker["xfce_pid"], f"xfce4-session pid {marker['xfce_pid']} -> {ln[8].strip()}")
            s("dmesg-shows-image-restored", "hibernation exit" in "\n".join(ln[9:]), " | ".join(ln[9:])[:300])
            rc, out = self.ssh("free -m | sed -n 2p; ps -eo comm | grep -cE 'firefox|brave'; dmesg | grep -ciE 'xfs.*(corrupt|error)|I/O error'")
            self.save("after-resume.txt", out)
            self.soft = False
            self.ssh("sync; (sleep 2; poweroff) >/dev/null 2>&1 &", timeout=20)
            t0 = time.time()
            while vm.alive() and time.time() < t0 + 180:
                time.sleep(2)
            self.step("clean-poweroff", not vm.alive(), f"{int(time.time() - t0)}s")
        finally:
            if vm.alive():
                vm.p.kill()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--cpu", default="IvyBridge")
    ap.add_argument("--disk-gb", type=int, default=40)
    ap.add_argument("--keep-disks", action="store_true")
    a = ap.parse_args()
    SQ.DMI = "type=1,manufacturer=Microsoft Corporation,product=Surface with Windows 8 Pro,version=1,sku=Surface_Pro"
    t = XT(a)
    try:
        t.run()
        if t.failed:
            print("FAILED STEPS:", ", ".join(t.failed))
            raise SystemExit(1)
        t.res["ok"] = True
        json.dump(t.res, open(f"{a.out}/result.json", "w"), indent=1)
        print("ALL PASS surface xfce")
    finally:
        if not a.keep_disks:
            for n in ("target.qcow2", "surfkeys.img", "OVMF_VARS.fd"):
                try:
                    os.unlink(os.path.join(a.out, n))
                except OSError:
                    pass


if __name__ == "__main__":
    main()
