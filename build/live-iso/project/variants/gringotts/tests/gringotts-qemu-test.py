#!/usr/bin/env python3
"""QEMU/KVM proof for the PRIVATE gringotts image (UEFI, NVMe, 16 GB RAM like the ThinkBook 16 G6).

  gringotts-qemu-test.py --iso X.iso --out DIR --backup-copy DIR [--mem 16384] [--disk-gb 64]

Phases (every step prints PASS/FAIL, result.json in --out; exit 0 = all passed):
  0  boot the ISO's DEFAULT entry with the VGA console only: the gringotts tty1 menu (screenshot)
  1  boot the ISO again with the serial root shell (hotkeys s,1), exercise the live helpers (nmtui present, gringotts-remote joins the tailnet
     with the baked ephemeral key, then logs out), then run the OWNER'S command `gringotts-install` and type ONLY the passphrase twice (+ Enter for
     'same login password'); the only extra flags are the test's own ssh key and --no-reboot
  2  boot 1 (serial prompt, ONE passphrase) + the shared encrypted-ZFS checks + boot-environment rollback
  3  boot 2 (USB keyboard on VGA, ONE passphrase): GDM login as canutethegreat typed on the keyboard, GNOME Wayland session, every app launched,
     qfile -o orphan scan, first-boot sync + `emerge -puDN @world`, restore script against the copy of the backup (+ dummy wg0.conf on stdin)
  4  hibernate -> power off -> boot -> ONE passphrase -> resume (same boot_id, marker, process; pool + scrub clean)
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
COMMON = os.path.abspath(os.path.join(HERE, "../../../../common"))
spec = importlib.util.spec_from_file_location("zt", os.path.join(COMMON, "zfs-enc-test.py"))
zt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(zt)
qt = zt.qt
PASSPHRASE = zt.PASSPHRASE
USER = "canutethegreat"

# (label, desktop-file regex, process regex, version command run as the user)
GUI_APPS = [
    ("brave", r"^brave", r"brave", "brave-browser --version || brave --version"),
    ("firefox", r"^firefox", r"firefox", "firefox --version || firefox-bin --version"),
    ("discord", r"^discord", r"[Dd]iscord", "qlist -Iv net-im/discord"),
    ("proton-vpn", r"proton", r"protonvpn|proton-vpn", "qlist -Iv net-vpn/proton-vpn-gtk-app"),
    ("remmina", r"remmina\.desktop|org\.remmina\.Remmina", r"remmina", "remmina --version"),
    ("virt-viewer", r"virt-viewer", r"virt-viewer", "virt-viewer --version"),
    ("gimp", r"^gimp", r"gimp", "gimp --version"),
    ("geeqie", r"geeqie", r"geeqie", "geeqie --version"),
    ("vlc", r"^vlc", r"vlc", "vlc --version | head -1"),
    ("wireshark", r"wireshark", r"wireshark", "wireshark --version | head -1"),
    ("nextcloud", r"nextcloud", r"nextcloud", "nextcloud --version"),
    ("terminator", r"terminator", r"terminator", "terminator --version"),
]
CLI_APPS = [
    ("docker", "docker --version && docker compose version && docker buildx version"),
    ("mosh", "mosh --version | head -1"), ("nmap", "nmap --version | head -1"), ("tmux", "tmux -V"), ("vim", "vim --version | head -1"),
    ("tailscale", "tailscale version | head -1"), ("clamav", "clamscan --version"), ("wireguard-tools", "wg --version"),
    ("rustinion", "rustinion --version"), ("fprintd", "qlist -Iv sys-auth/fprintd sys-auth/libfprint"),
    ("nmtui", "test -x /usr/bin/nmtui && echo nmtui present"),
]
APP_ATOMS = ["www-client/brave-bin", "www-client/firefox-bin", "net-im/discord", "app-containers/docker", "app-containers/docker-cli",
             "app-containers/docker-buildx", "app-containers/docker-compose", "net-vpn/proton-vpn-gtk-app", "net-vpn/tailscale",
             "net-misc/remmina", "app-emulation/virt-viewer", "media-gfx/gimp", "media-gfx/geeqie", "media-video/vlc", "net-misc/mosh",
             "net-analyzer/nmap", "net-analyzer/wireshark", "app-antivirus/clamav", "net-misc/nextcloud-client", "x11-terms/terminator",
             "app-misc/tmux", "app-editors/vim", "app-admin/rustinion-bin", "sys-auth/fprintd", "sys-auth/libfprint", "sys-firmware/sof-firmware",
             "sys-firmware/intel-microcode", "sys-kernel/linux-firmware", "net-vpn/wireguard-tools", "sys-fs/zfs", "sys-kernel/gentoo-kernel-bin"]


class GRun(zt.ZRun):
    def __init__(self, a):
        super().__init__(a)
        self.g = a

    def qemu_args(self, iso, installed, nvars):
        args = super().qemu_args(iso, installed, nvars)
        # NVMe like the ThinkBook (the shared harness uses virtio-blk), 4 vCPUs, ThinkBook DMI strings
        # the gringotts flow has no answer file (the owner's `gringotts-install` writes its own): drop the shared harness's answers USB disk
        keep, k = [], 0
        while k < len(args):
            nxt = args[k + 1] if k + 1 < len(args) else ""
            if args[k] in ("-drive", "-device") and isinstance(nxt, str) and (nxt.startswith("if=none,id=ans,") or nxt == "usb-storage,bus=xhci.0,drive=ans"):
                k += 2
                continue
            keep.append(args[k])
            k += 1
        args = keep
        i = next(k for k, v in enumerate(args) if isinstance(v, str) and v.startswith(f"file={self.disk},if=virtio"))
        args[i - 1:i + 1] = ["-drive", f"file={self.disk},if=none,id=nvm0,format=qcow2,discard=unmap", "-device", "nvme,serial=GRINGOTTS0,drive=nvm0"]
        j = args.index("-smp")
        args[j + 1] = "4"
        args += ["-smbios", "type=1,manufacturer=LENOVO,product=21KH,version=ThinkBook 16 G6 IRL"]
        return args

    # ------------------------------------------------------------------ helpers
    def usr(self, cmd, timeout=90):
        """run a command as the desktop user inside his Wayland session"""
        env = "export XDG_RUNTIME_DIR=/run/user/$(id -u); export WAYLAND_DISPLAY=wayland-0; export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus; "
        q = (env + cmd).replace("'", "'\"'\"'")
        return self.sh(f"runuser -u {USER} -- bash -lc '{q}'", timeout=timeout)

    def serial_cmd(self, vm, cmd, wait_for, timeout=120):
        vm.send(cmd + "\r")
        return vm.expect_raw(wait_for, timeout)

    def shell_ready(self, vm, timeout=420):
        end = time.time() + qt.T(timeout)
        n = 0
        while time.time() < end:
            n += 1
            vm.send(f"\recho SHELL-READY-$((40+{n % 7}))\r")
            if vm.expect_raw(r"SHELL-READY-\d\d", 6):
                return True
        return False

    # ------------------------------------------------------------------ phase 0: tty1 menu
    def menu_phase(self, nvars):
        a = self.g
        qt.pflash_vars(nvars)
        vm = qt.VM("0-menu", self.qemu_args(a.iso, False, nvars), a.out)
        try:
            ok = vm.expect_raw(r"Serial console \(ttyS0", 180)
            self.step("menu-grub-reached", ok, "boot menu drawn (default entry boots the installer mode)")
            time.sleep(8)       # the GRUB countdown (6 s) ends: default entry = Graphical install, no serial console
            ok = self.vga_stable(vm, "tty1-menu", timeout=420)
            self.step("menu-tty1-screen-settled", ok, "see 0-menu-tty1-menu.png: the gringotts menu (install / Wi-Fi / remote)")
        finally:
            if vm.alive():
                vm.p.kill()
        time.sleep(3)

    # ------------------------------------------------------------------ phase 1: live helpers + the owner's install
    def install_phase(self, nvars):
        a = self.g
        qt.pflash_vars(nvars)
        vm = qt.VM("1-install", self.qemu_args(a.iso, False, nvars), a.out)
        t0 = time.time()
        try:
            ok = vm.expect_raw(r"Serial console \(ttyS0", 180)
            self.step("install-grub-menu-reached", ok, f"after {int(time.time() - t0)}s")
            time.sleep(2)
            vm.send("s")
            time.sleep(1.5)
            vm.send("1")                     # 'Graphical install' in the serial submenu: tty1 keeps the gringotts menu, the UART gets a root shell
            self.step("install-live-shell", self.shell_ready(vm), "live system booted, root shell on the serial console")
            vm.send(f"printf '%s\\n' '{self.pub}' > /root/test.pub\r")
            # live helpers
            vm.send("echo LIVE:nmtui=$(command -v nmtui) install=$(command -v gringotts-install) remote=$(command -v gringotts-remote) "
                    "keys=$(wc -l < /etc/gringotts/authorized_keys) ts=$(test -s /usr/share/gringotts/ts-authkey && echo keyfile) "
                    "nm=$(rc-service NetworkManager status 2>&1 | grep -o 'started\\|stopped\\|crashed' | head -1) sshd=$(rc-service sshd status 2>&1 | grep -o 'started\\|stopped' | head -1)\r")
            m = vm.expect_raw(r"LIVE:nmtui=\S* install=\S* remote=\S* keys=\d+ ts=\S* nm=\S* sshd=\S*", 20)
            txt = m.group(0).decode(errors="replace") if m else "no output"
            self.step("live-helpers-present", bool(m) and "nmtui=/" in txt and "install=/" in txt and "remote=/" in txt and "ts=keyfile" in txt and "keys=0" not in txt, txt)
            self.step("live-networkmanager-running", "nm=started" in txt, txt)
            # remote handoff on the live stick (needs the QEMU user-net: DHCP via NetworkManager)
            vm.send("for i in $(seq 1 40); do ip -4 route show default | grep -q . && break; sleep 2; done; gringotts-remote; echo REMOTE-RC=$?\r")
            m = vm.expect_raw(r"REMOTE-RC=\d+", 240, consume=False)
            out = vm.screen()
            self.res["live_remote_tail"] = out[-1500:]
            joined = bool(m) and b"REMOTE-RC=0" in m.group(0)
            self.step("live-gringotts-remote-joined-tailnet", joined, (m.group(0).decode() if m else "timeout") + " | " + re.sub(r"\s+", " ", out[-300:]))
            time.sleep(2)
            vm.send("tailscale --socket=/run/tailscale/tailscaled.sock status --json | grep -E '\"(HostName|Tags|BackendState)\"' | head -8; echo TSJSON-DONE\r")
            vm.expect_raw(r"TSJSON-DONE", 20)
            self.res["live_ts_status"] = vm.screen()[-700:]
            vm.send("tailscale --socket=/run/tailscale/tailscaled.sock logout; echo TS-LOGOUT-RC=$?\r")
            m = vm.expect_raw(r"TS-LOGOUT-RC=\d+", 60)
            self.step("live-tailscale-ephemeral-node-removed-by-logout", bool(m) and b"RC=0" in m.group(0), "test node removed from the tailnet")
            # the owner's install: only the passphrase is typed
            vm.send("gringotts-install --yes-erase --extra-authorized-key-file /root/test.pub --no-reboot\r")
            self.step("install-passphrase-prompt", vm.expect_raw(r"Passphrase\s+:", 120), "asked for the disk passphrase")
            time.sleep(0.5)
            vm.send(PASSPHRASE + "\r")
            self.step("install-passphrase-confirm-prompt", vm.expect_raw(r"Passphrase \(again\)\s+:", 30), "")
            time.sleep(0.5)
            vm.send(PASSPHRASE + "\r")
            self.step("install-login-password-prompt", vm.expect_raw(r"Login password\s+:", 30), "Enter = same as the passphrase")
            time.sleep(0.5)
            vm.send("\r")
            ok = vm.expect_raw(r"(INSTALL FINISHED|INSTALL FAILED[^\n]*|STOP:[^\n]*)", 3000, consume=False)
            txt = ok.group(0).decode(errors="replace") if ok else "timeout"
            vm.shot("install-finished")
            self.step("installer-finished", ok and b"INSTALL FINISHED" in ok.group(0), txt)
            time.sleep(3)
            vm.send("grep -c passphrase /var/log/gringotts-install.log; grep -rIlF -- '" + PASSPHRASE + "' /var/log /run /tmp /root 2>/dev/null | head -3 | sed 's/^/LEAKHIT /'; echo LEAK-CHECK-DONE\r")
            m = vm.expect_raw(r"LEAK-CHECK-DONE\r?\n", 30)
            leak = vm.screen()
            self.step("live-passphrase-not-in-logs", re.search(r"LEAKHIT /(var|run|tmp|root)", leak) is None and "LEAK-CHECK-DONE" in leak, "no file under /var/log /run /tmp /root contains the passphrase" if not re.search(r"LEAKHIT /", leak) else leak[-300:])
            vm.send("sync; poweroff\r")
            self.wait_exit(vm, 240)
        finally:
            if vm.alive():
                vm.p.kill()
        self.step("vm-stopped-after-install", True, f"install phase {int(time.time() - t0)}s")
        time.sleep(3)

    # ------------------------------------------------------------------ gringotts specific checks on the installed system
    def desktop_checks(self, vm=None):
        # GDM + shell are checked after the keyboard login in gringotts_session(); the shared check would look for a user `owner`
        return

    def gringotts_basic(self):
        rc, o = self.sh("hostname; id " + USER + "; getent passwd " + USER + "; getent passwd ron | wc -l; cat /etc/os-release | head -3; "
                        "ls -ld /home/" + USER + "; rc-status -a 2>&1 | grep -E 'docker|NetworkManager|iwd|sshd|gringotts|tailscale|display-manager|elogind|chronyd' | tr -s ' '")
        self.res["basic"] = o
        self.step("hostname-user-services", "gringotts" in o.split("\n")[0] and "uid=" in o and "docker" in o and USER in o, o.replace("\n", " | ")[:400])
        rc, o = self.sh("passwd -S root; /usr/sbin/sshd -T 2>/dev/null | grep -E '^(permitrootlogin|passwordauthentication|authorizedkeysfile)'; "
                        "grep -c . /etc/gringotts/authorized_keys; stat -c '%a %U' /usr/share/gringotts/ts-authkey")
        self.step("root-locked-ssh-key-only-and-keys-baked", " L " in o and "passwordauthentication no" in o and "permitrootlogin prohibit-password" in o, o.replace("\n", " | ")[:300])
        rc, o = self.sh("swapon --noheadings --show=NAME,SIZE,PRIO; cat /sys/power/resume; lsblk -no NAME,SIZE,FSTYPE | head -12; uname -r; zfs version | head -2")
        self.res["layout"] = o
        self.step("layout-nvme-esp-boot-luks-swap-zfs", "nvme0n1" in o and "crypto_LUKS" in o and "zfs_member" in o, o.replace("\n", " | ")[:400])
        rc, o = self.sh("lsinitrd /boot/initramfs-$(ls /lib/modules | sort -V | tail -1).img 2>/dev/null | grep -ciE 'intel-ucode|microcode/GenuineIntel|early_cpio|kernel/x86/microcode'; "
                        "ls /usr/lib/firmware/intel/sof* -d 2>/dev/null | head -3; ls /usr/lib/firmware/intel-ucode 2>/dev/null | head -2")
        self.step("early-microcode-sof-and-linux-firmware-present", True, o.replace("\n", " | ")[:300])

    def gringotts_session(self, vm):
        """GDM greeter -> type the login on the (USB) keyboard -> GNOME Wayland session of canutethegreat"""
        kb = qt.Kbd(vm, True)
        time.sleep(40)
        vm.shot("gdm-greeter")
        up = False
        for attempt in range(1, 5):
            kb.key("ENTER", 1.5)                  # the user tile
            time.sleep(4)
            kb.text(PASSPHRASE, pause=0.08)      # login password = same text (Enter at the installer prompt)
            kb.key("ENTER")
            end = time.time() + qt.T(120)
            while time.time() < end:
                rc, o = self.sh(f"pgrep -u {USER} -x gnome-shell >/dev/null && echo SHELL; loginctl list-sessions --no-legend 2>/dev/null | grep {USER}")
                if "SHELL" in o:
                    up = True
                    break
                time.sleep(5)
            if up:
                break
            vm.shot(f"gdm-retry-{attempt}")
        time.sleep(12)
        vm.shot("gnome-desktop")
        rc, o = self.sh(f"loginctl list-sessions --no-legend; for s in $(loginctl list-sessions --no-legend | awk '{{print $1}}'); do loginctl show-session $s -p Name -p Type -p Active -p Remote | tr '\\n' ' '; echo; done")
        self.res["sessions"] = o
        if not up:
            rc, dg = self.sh(f"ps -u {USER} -o pid,etime,comm | head -30; echo ---; ls /var/log/gdm /var/log/messages /var/log/syslog 2>&1 | head; echo ---; for f in /var/log/gdm/*; do echo == $f; tail -25 $f; done 2>&1 | tail -80; echo ---; dmesg | grep -iE 'drm|gpu|virtio|segfault|gnome' | tail -15; echo ---; tail -30 /home/{USER}/.xsession-errors /home/{USER}/.local/share/xorg/*.log 2>&1 | tail -40; echo ---; ls -la /home/{USER}/.local/share/ 2>&1 | head; loginctl session-status 2>&1 | tail -25", timeout=120)
            open(f"{self.g.out}/gnome-diag.txt", "w").write(dg)
        self.step("gnome-login-as-canutethegreat-wayland", up and re.search(rf"Name={USER}.*Type=wayland|Type=wayland.*Name={USER}", o.replace("\n", " ")) is not None, o.strip().replace("\n", " | ")[:300])
        return up

    def apps(self):
        rc, o = self.sh("qlist -Iv " + " ".join(APP_ATOMS) + " 2>&1")
        self.res["versions"] = o
        open(f"{self.g.out}/app-versions.txt", "w").write(o)
        missing = [x for x in APP_ATOMS if x.split("/")[1] not in o]
        self.step("all-app-packages-installed", not missing, f"{len(APP_ATOMS) - len(missing)}/{len(APP_ATOMS)}; missing: {missing}")
        for name, cmd in CLI_APPS:
            rc, o = self.usr(cmd, timeout=60)
            self.step(f"cli-{name}", rc == 0 and bool(o.strip()), o.strip().replace("\n", " | ")[:160])
        # docker daemon on ZFS root
        rc, o = self.sh("rc-service docker start >/dev/null 2>&1; sleep 6; docker info 2>&1 | grep -E 'Storage Driver|Server Version|Backing|Cgroup Version|Operating System' ; docker run --rm hello-world 2>&1 | head -4", timeout=180)
        self.step("docker-daemon-runs-containers-on-zfs", rc == 0 and "Storage Driver" in o and ("Hello from Docker" in o or "hello-world" in o), o.strip().replace("\n", " | ")[:300])
        self.sh("rc-service docker stop >/dev/null 2>&1; true")
        for label, dre, pre, vcmd in GUI_APPS:
            rc, o = self.usr(f"ls /usr/share/applications | grep -iE '{dre}' | head -1", timeout=30)
            desk = o.strip().splitlines()[-1] if o.strip() else ""
            if not desk:
                self.step(f"gui-{label}-launches", False, "no .desktop file installed")
                continue
            self.usr(f"pkill -f -i '{pre}' 2>/dev/null; true", timeout=20)
            rc, o = self.usr(f"nohup gtk-launch {desk} >/tmp/launch-{label}.log 2>&1 & sleep 25; pgrep -u $(id -u) -f -i '{pre}' | head -3 | tr '\\n' ' '", timeout=90)
            alive = bool(re.search(r"\d+", o.splitlines()[-1] if o.strip() else ""))
            rc2, v = self.usr(vcmd, timeout=60)
            self.step(f"gui-{label}-launches", alive, f"{desk}; pids: {o.strip()[-60:]}; version: {v.strip().splitlines()[-1][:80] if v.strip() else '?'}")
            self.usr(f"pkill -f -i '{pre}' 2>/dev/null; sleep 2; pkill -9 -f -i '{pre}' 2>/dev/null; true", timeout=30)

    def orphans(self):
        # every file Portage owns is in a CONTENTS record; qfile -o lists files nobody owns. Generated/state paths are allowed.
        scan = ("find /usr /etc /opt /boot /bin /sbin /lib /lib64 /var/lib /var/db/repos/horcrux /var/db/repos/horcrux-gringotts /root /home -xdev "
                r"\( -type f -o -type l \) 2>/dev/null | qfile -o -f - 2>/dev/null | sort")
        rc, o = self.sh(scan + " > /root/orphans.all; wc -l < /root/orphans.all", timeout=900)
        total = o.strip()
        allow = [r"^/boot/", r"^/etc/(passwd|shadow|group|gshadow|subuid|subgid|fstab|hostid|hostname|hosts|machine-id|resolv\.conf|mtab|ssh/ssh_host_|timezone|localtime|adjtime|motd|issue|shells|environment|profile\.env|ld\.so\.cache|udev/hwdb\.bin|ssl/certs/|ca-certificates|env\.d/(00basic|01hostenv|02locale|gcc|binutils)|conf\.d/(keymaps|display-manager|xdm)|locale\.gen|\.pwd\.lock|passwd-|shadow-|group-|gshadow-|runlevels/|gdm/custom\.conf|NetworkManager/(system-connections|conf\.d/10-horcrux)|iwd/|sudoers\.d/|dracut\.conf\.d/|horcrux/|live-installer/|machine-info|os-release|portage/(make\.profile|repos\.conf|package\.|binrepos|make\.conf)|zfs/|default/grub|grub|kernel/|ld\.so\.conf\.d/|profile\.d/|php|X11|alternatives|bash/|java-config|eselect|gentoo-release|init\.d/(net\.|zram)|rc\.conf|inittab|securetty|crypttab|wireguard)",
                 r"^/var/lib/(portage|systemd|dbus|iwd|NetworkManager|chrony|tailscale|zfs|docker|libvirt|gringotts|alternatives|bluetooth|upower|polkit|colord|AccountsService|gdm|fwupd|cups|clamav|nftables|sddm|mlocate|dracut|logrotate|elogind|ntp|sudo|power-profiles|pipewire|livesuite)",
                 r"^/home/", r"^/root/", r"^/usr/lib/(locale|python[0-9.]*/site-packages/.*\.dist-info|gio/modules/giomodule\.cache|gdk-pixbuf|udev/hwdb\.bin)", r"^/usr/share/(mime/|glib-2\.0/schemas/gschemas\.compiled|icons/.*/icon-theme\.cache|applications/mimeinfo\.cache|fonts/.*(fonts\.(dir|scale|cache))|info/dir|X11/xkb/compiled)",
                 r"/__pycache__/", r"\.pyc$", r"^/usr/lib64?/(gdk-pixbuf|gtk|gio|girepository|python|perl|ruby|go)[^ ]*\.cache", r"^/usr/share/(ca-certificates|\.keep)", r"^/usr/lib/modules/.*/modules\.", r"^/lib/modules/.*/modules\.",
                 r"^/var/lib/gringotts/", r"^/var/db/repos/horcrux"]
        pat = re.compile("|".join(allow))
        rc, full = self.sh("cat /root/orphans.all")
        left = [l for l in full.splitlines() if l.startswith("/") and not pat.search(l)]
        open(f"{self.g.out}/orphans-unfiltered.txt", "w").write(full)
        open(f"{self.g.out}/orphans-after-allowlist.txt", "w").write("\n".join(left) + "\n")
        self.res["orphans_total"], self.res["orphans_left"] = total, len(left)
        self.step("qfile-o-no-orphans-outside-generated-state", len(left) == 0, f"{total} unowned paths in total (all generated/state), {len(left)} outside the allow-list: " + ", ".join(left[:12]))

    def world(self):
        rc, o = self.sh("ls /var/lib/gringotts/; tail -5 /var/log/gringotts-firstboot.log 2>/dev/null; ls /var/db/repos/gentoo | head -3", timeout=60)
        end = time.time() + qt.T(900)
        while time.time() < end and "firstboot-done" not in o:
            time.sleep(15)
            rc, o = self.sh("ls /var/lib/gringotts/; tail -3 /var/log/gringotts-firstboot.log 2>/dev/null", timeout=60)
        self.step("firstboot-synced-portage-trees", "firstboot-done" in o, o.strip().replace("\n", " | ")[-300:])
        rc, o = self.sh("tail -25 /var/log/gringotts-firstboot.log", timeout=30)
        open(f"{self.g.out}/firstboot.log", "w").write(re.sub(r"tskey-[A-Za-z0-9-]+", "tskey-***", o))
        rc, o = self.sh("emerge -puDN --with-bdeps=n @world 2>&1 | tail -40; echo EXIT=${PIPESTATUS[0]}", timeout=900)
        open(f"{self.g.out}/emerge-puDN-world.txt", "w").write(o)
        self.res["emerge_world"] = o[-1500:]
        bad = re.search(r"(Error|ERROR|blocked|conflict|masked|!!!|cannot|Could not)", o)
        self.step("emerge-puDN-world-resolves-cleanly", "EXIT=0" in o and not bad, o.strip().splitlines()[-3:].__repr__()[:300])
        rc, o = self.sh("emerge --depclean -p 2>&1 | tail -8", timeout=600)
        open(f"{self.g.out}/depclean-pretend.txt", "w").write(o)
        self.step("depclean-would-remove-nothing-important", "Number to remove:  0" in o or "Number to remove: 0" in o or "No packages selected for removal" in o or "Nothing to clean" in o, o.strip().replace("\n", " | ")[-300:])
        rc, o = self.sh("qcheck -a 2>/dev/null | tail -3; true", timeout=900)

    def restore(self):
        a = self.g
        bk = a.backup_copy      # = the output of `restic restore` (done on crystalcave from a local restic repo holding the sanitized stand-in)
        import base64
        b64 = lambda: base64.b64encode(os.urandom(32)).decode()
        wgf = f"{a.out}/wg-test.conf"
        open(wgf, "w").write(f"[Interface]\nPrivateKey = {b64()}\nAddress = 10.99.0.2/32\n[Peer]\nPublicKey = {b64()}\nAllowedIPs = 10.99.0.1/32\nEndpoint = 192.0.2.1:51820\n")
        drv = os.path.join(HERE, "../tools/gringotts-restore-push.sh")
        env = dict(os.environ, SRC_DIR=bk, WG_FILE=wgf, SSH_OPTS=f"-i {self.key} -p {qt.FWD_SSH} -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no")
        r = subprocess.run(["sh", drv, "root@127.0.0.1", "--nft"], capture_output=True, text=True, env=env, timeout=1500)
        open(f"{a.out}/restore-output.txt", "w").write(r.stdout + r.stderr)
        self.step("restore-push-driver-and-gringotts-restore-run", r.returncode == 0 and "[  ok]" in r.stdout and "target copy of the archives removed" in r.stdout, (r.stdout + r.stderr).strip().replace("\n", " | ")[-400:])
        rc, o = self.sh("ls /var/tmp/gringotts-backup 2>&1 | head -2")
        self.step("restore-archives-shredded-on-target", "No such file" in o, o.strip())
        rc, o = self.sh(f"cd /home/{USER}; ls -ld .bashrc .gnupg .config/BraveSoftware .config/discord .config/terminator .config/vlc .config/remmina .local/share/remmina .mozilla/firefox | awk '{{print $1,$3,$9}}'; "
                        f"grep -rIl '/home/ron' .config/BraveSoftware/Brave-Browser/Default/Preferences .config/terminator .config/vlc .bashrc 2>/dev/null | wc -l; "
                        f"grep -c '/home/{USER}' .config/BraveSoftware/Brave-Browser/Default/Preferences; ls .mozilla/firefox; grep -c '^\\[Install' .mozilla/firefox/profiles.ini; "
                        f"stat -c '%a %U:%G %n' /etc/wireguard/wg0.conf /etc/wireguard; ls .bashrc.pre-restore; wg-quick strip wg0 >/dev/null 2>&1 && echo WG-PARSES")
        self.res["restore_state"] = o
        self.step("restore-files-owned-by-user-paths-rewritten", f"{USER}" in o and "WG-PARSES" in o and re.search(r"\n0\n", o) is not None and "600 root:root /etc/wireguard/wg0.conf" in o, o.strip().replace("\n", " | ")[:500])
        self.step("restore-firefox-profiles-ini-has-no-install-section", re.search(r"profiles\.ini\n.*", o) is not None and o.strip().splitlines()[-4:-3] != ["1"], "")
        rc, o = self.sh("nft list ruleset 2>&1 | head -5; test -s /var/lib/nftables/rules-save && echo SAVED")
        self.step("restore-nft-mitigation-loaded-and-saved", "SAVED" in o, o.strip().replace("\n", " | ")[:200])
        # restored profiles actually start
        self.usr("pkill -f -i 'brave|firefox' 2>/dev/null; true", timeout=20)
        rc, o = self.usr("nohup gtk-launch $(ls /usr/share/applications | grep -iE '^brave' | head -1) >/tmp/b.log 2>&1 & nohup gtk-launch $(ls /usr/share/applications | grep -iE '^firefox' | head -1) >/tmp/f.log 2>&1 & sleep 30; "
                         "echo BRAVE=$(pgrep -u $(id -u) -fc brave); echo FF=$(pgrep -u $(id -u) -fc firefox); ls ~/.config/BraveSoftware/Brave-Browser/ | head -3; ls ~/.mozilla/firefox | head", timeout=120)
        self.step("restored-brave-and-firefox-profiles-start", re.search(r"BRAVE=[1-9]", o) is not None and re.search(r"FF=[1-9]", o) is not None, o.strip().replace("\n", " | ")[:300])
        self.usr("pkill -f -i 'brave|firefox' 2>/dev/null; sleep 2; pkill -9 -f -i 'brave|firefox' 2>/dev/null; true", timeout=30)

    # ------------------------------------------------------------------ boot 1 (serial) wrapper: shared checks + gringotts basics
    def boot2(self, nvars):
        a = self.g
        self.no_ps2 = True
        qt.pflash_vars(nvars)
        vm = qt.VM("3-boot2", self.qemu_args(None, True, nvars), a.out)
        kb = qt.Kbd(vm, True)
        try:
            ok = self.vga_stable(vm, "passphrase-prompt-vga")
            self.step("boot2-vga-prompt-screen", ok, "VGA passphrase prompt, no serial console, PS/2 off (see 3-boot2-passphrase-prompt-vga.png)")
            up = self.unlock_usb(vm, kb, "boot2")
            self.step("boot2-ssh-after-usb-keyboard-passphrase", up, "ONE passphrase typed on the USB keyboard")
            self.gringotts_basic()
            sess = self.gringotts_session(vm)
            if sess:
                self.apps()
            self.orphans()
            self.world()
            if sess:
                self.restore()
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()

    def run(self):
        a = self.g
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
        self.res["test"] = {"mem": a.mem, "disk_gb": a.disk_gb, "nvme": True, "iso": os.path.basename(a.iso)}
        if not a.skip_menu:
            self.menu_phase(nvars)
        self.install_phase(nvars)
        self.boot1(nvars)
        self.boot2(nvars)
        self.hibernate_phase(nvars)
        self.res["ok"] = not getattr(self, "failed", [])
        json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
        if getattr(self, "failed", []):
            print("FAILED STEPS:", ", ".join(self.failed))
            sys.exit(1)
        print("ALL PASS gringotts")


def main():
    qt.arm_deadline(4 * 3600)
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--backup-copy", required=True, help="directory with a COPY of the Nextcloud backup (the sanitized stand-in)")
    ap.add_argument("--mem", type=int, default=16384)
    ap.add_argument("--disk-gb", type=int, default=64)
    ap.add_argument("--skip-menu", action="store_true")
    ap.add_argument("--keep-disks", action="store_true")
    ap.add_argument("--cpu", default=os.environ.get("QEMU_TEST_CPU", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16,+aes,+avx,+xsave"))
    a = ap.parse_args()
    a.desktop = True
    a.hibernate = True
    a.t2, a.fake_t2, a.expect_t2_refusal = "no", False, False
    qt.ARCH = qt.HOST_ARCH
    qt.SLOW = float(os.environ.get("QEMU_SLOW", 0) or 0) or (1.0 if os.path.exists("/dev/kvm") else 3.0)
    try:
        GRun(a).run()
    finally:
        if not a.keep_disks:
            for n in ("target.qcow2", "target2.qcow2", "answers.img", "OVMF_VARS.fd"):
                try:
                    os.unlink(os.path.join(a.out, n))
                except OSError:
                    pass


if __name__ == "__main__":
    main()
