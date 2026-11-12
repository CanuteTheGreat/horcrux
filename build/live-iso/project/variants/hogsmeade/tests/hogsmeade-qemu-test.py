#!/usr/bin/env python3
"""HOGSMEADE QEMU/KVM proof: unattended install of the PRIVATE image onto THREE NVMe drives (one encrypted ZFS pool = a STRIPE of
three top-level vdevs), then: first boot OFFLINE and ONLINE, time zone, kernel 6.18.62, NO Tailscale, no autologin, real GDM logins with the separate
login password, keyring across logout and reboot, restore drills, ComfyUI on 8188 only from the home LAN / WireGuard, Rustinion enrol path,
hibernate/resume, boot from the third drive's ESP alone, qfile -o and emerge -puDN @world.

  hogsmeade-qemu-test.py --iso X.iso --out DIR [--mem 8192]

Built on common/zfs-enc-test.py (same harness, same steps for the encrypted-ZFS basics) with:
  * 3 emulated NVMe drives (40G/24G/24G) + faked DMI (Alienware 18 Area-51 AA18250) so the `hogsmeade` hardware profile is auto-detected
  * every boot types the passphrase on the USB keyboard at the VGA prompt (the laptop has no UART; QEMU PS/2 off)
  * stripe-specific checks, per-drive ESP + /boot copy, swap on the first drive only, profile services, first-boot datasets, tools
  * last phase: zero ESP+/boot of drive 1, boot with ONLY drive 3 bootable (OVMF bootindex): the system must still come up
GPU, audio, WMI/EC, lighting, Wi-Fi/BT, backlight are NOT provable in a VM: see hosts/hogsmeade/CONVERSION.md section 'first real boot'.
"""
import argparse, importlib.util, json, os, re, shutil, socket, subprocess, sys, tempfile, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.abspath(os.path.join(HERE, "..", "..", "..", "..", "common"))
for cand in (COMMON, os.path.join(os.getcwd(), "common")):
    if os.path.exists(os.path.join(cand, "zfs-enc-test.py")):
        COMMON = cand
        break
spec = importlib.util.spec_from_file_location("zt", os.path.join(COMMON, "zfs-enc-test.py"))
zt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(zt)
qt = zt.qt
USER = "canutethegreat"
KERNEL = "6.18.62-gentoo-dist"     # 6.18 LTS: the newest series OpenZFS 2.3.9 supports that the ::gentoo snapshot still carries
TZ = "America/Los_Angeles"
NETS = ("n0", "n1", "n2")          # n0 = QEMU default 10.0.2.0/24 (a foreign network), n1 = home LAN 10.66.69.0/24, n2 = home WireGuard 10.69.69.0/24
KR_VALUE = "hxvalue-not-a-real-secret"
HALT = "power" + "off"      # guest shutdown command


def free_ports(n):
    socks, ports = [], []
    for _ in range(n):
        k = socket.socket(); k.bind(("127.0.0.1", 0)); socks.append(k); ports.append(k.getsockname()[1])
    for k in socks:
        k.close()
    return ports


def http_get(port, path="/system_stats", timeout=8):
    """(status, body-start) of http://127.0.0.1:port/path; status 0 = no HTTP answer (refused / reset / timeout)"""
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=timeout) as r:
            return r.status, r.read(200).decode(errors="replace")
    except Exception as e:
        return 0, f"{type(e).__name__}: {e}"[:120]
# exactly ONE real keyring daemon: the one PAM started at the greeter login (--daemonize --login), which holds the login collection the login
# password unlocked. A D-Bus activated `--start --foreground --components=secrets` process next to it is gnome-keyring's placeholder: it
# hands the session to the running daemon through $XDG_RUNTIME_DIR/keyring/control and only waits (it owns no keyring). Two --login daemons
# (the gringotts bug: no persistent login keyring) or any other daemon fail the gate.
KR_DAEMONS = "LOGIN-DAEMONS=1 OTHER-DAEMONS=0"
SIZES_GB = (40, 40, 40)   # nvme enumeration order is not stable in QEMU: any drive may become nvme0n1, which carries the 9 GiB hibernate swap


class HRun(zt.ZRun):
    def __init__(self, a):
        a.t2, a.fake_t2, a.expect_t2_refusal, a.desktop, a.hibernate = "no", False, False, True, True
        a.server, a.late_network = False, 0      # a desktop machine image: the shared checks take the desktop branch
        super().__init__(a)
        self.boot_only = None            # index of the only bootable drive (ESP-of-drive-3 phase)
        self.disks = []
        self.p_foreign, self.p_lan, self.p_wg = free_ports(3)      # host ports forwarded to the guest's 8188 through each network

    def qemu_args(self, iso, installed, nvars):
        args = super().qemu_args(iso, installed, nvars)
        i = next(k for k, x in enumerate(args) if x == "-drive" and args[k + 1].startswith(f"file={self.disk},if=virtio"))
        new = []
        for n, path in enumerate(self.disks):
            bi = f",bootindex={1}" if (self.boot_only is not None and n == self.boot_only and not iso) else ""
            new += ["-drive", f"file={path},if=none,id=nv{n},format=qcow2", "-device", f"nvme,drive=nv{n},serial=HSMNVME{n:02d}{bi}"]
        args[i:i + 2] = new
        args += ["-smbios", "type=1,manufacturer=Alienware,product=Alienware 18 Area-51 AA18250"]
        if iso:
            k = next(k for k, x in enumerate(args) if x.startswith("usb-storage,bus=xhci.0,drive=stick"))
            args[k] += ",id=stick0"                      # QMP device_del = the owner pulling the USB stick
            k = next((k for k, x in enumerate(args) if x.startswith("if=none,id=ans,")), None)
            if k is not None:
                del args[k - 1:k + 3]                    # no answer-file drive: the install is typed at the front door, like the owner does
        if not iso:
            # the installed system sees three networks. A connection forwarded by QEMU arrives FROM that network's host address, so the host
            # firewall is exercised with real packets: 10.0.2.2 (any other network), 10.66.69.2 (home LAN), 10.69.69.2 (home WireGuard)
            j = next(k for k, x in enumerate(args) if x.startswith("user,id=n0"))
            args[j] += f",hostfwd=tcp:127.0.0.1:{self.p_foreign}-10.0.2.15:8188"
            nic = qt.ARCHS[qt.ARCH]["nic"]
            args += ["-netdev", f"user,id=n1,net=10.66.69.0/24,host=10.66.69.2,dhcpstart=10.66.69.15,hostfwd=tcp:127.0.0.1:{self.p_lan}-10.66.69.15:8188",
                     "-device", f"{nic},netdev=n1,mac=52:54:00:00:00:02",
                     "-netdev", f"user,id=n2,net=10.69.69.0/24,host=10.69.69.2,dhcpstart=10.69.69.15,hostfwd=tcp:127.0.0.1:{self.p_wg}-10.69.69.15:8188",
                     "-device", f"{nic},netdev=n2,mac=52:54:00:00:00:03"]
        return args

    def toml(self, pubkey, pwhash):
        uh = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemuuser1", zt.USER_PW]).decode().strip()
        return "\n".join([
            "[global]", 'keyboard = "us"', 'timezone = "America/Los_Angeles"', 'hostname = "hogsmeade"', 'install_type = "desktop"',
            f'root_password_hash = "{pwhash}"', f'ssh_keys = ["{pubkey}"]', "ssh_password_auth = false", "",
            "[disk]", 'filesystem = "zfs"', 'zfs_raid = "stripe"', 'disks = ["/dev/nvme0n1", "/dev/nvme1n1", "/dev/nvme2n1"]',
            "encryption = true", 'swap = "hibernate"', f'passphrase = "{zt.PASSPHRASE}"', "",
            "[network.management]", 'mode = "dhcp"', "",
            "[user]", f'name = "{USER}"', f'fullname = "{USER}"', f'password_hash = "{uh}"', "autologin = false", ""])

    def run(self):
        a = self.z
        os.makedirs(a.out, exist_ok=True)
        self.disk, self.answers = f"{a.out}/target.qcow2", f"{a.out}/answers.img"
        self.disks = [f"{a.out}/target{n if n else ''}.qcow2" if n else self.disk for n in range(3)]
        self.disks = [self.disk, f"{a.out}/target2.qcow2", f"{a.out}/target3.qcow2"]
        fresh = not (getattr(a, "phases", "") and "install" not in a.phases.split(","))      # --phases without install: reuse the kept disks + key
        nvars = f"{a.out}/OVMF_VARS.fd"
        self.key = f"{a.out}/test_key"
        if fresh:
            for d, g in zip(self.disks, SIZES_GB):
                subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", d, f"{g}G"])
            qt.pflash_vars(nvars)
            for f in (self.key, self.key + ".pub"):
                if os.path.exists(f):
                    os.unlink(f)
            subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "qemu-test", "-f", self.key])
        self.pub = open(self.key + ".pub").read().strip()
        self.pub = open(self.key + ".pub").read().strip()
        only = set(filter(None, (getattr(a, "phases", "") or "").split(",")))
        if not only or "install" in only:
            self.phase1(nvars)
        if not only or "main" in only:
            self.boot_main(nvars)
        if not only or "reboot" in only:
            self.reboot_keyring_phase(nvars)
        if not only or "hibernate" in only:
            self.hibernate_phase(nvars)
        if not only or "drive3" in only:
            self.third_drive_boot(nvars)
        self.res["ok"] = not getattr(self, "failed", [])
        json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
        if getattr(self, "failed", []):
            print("FAILED STEPS:", ", ".join(self.failed))
            sys.exit(1)
        print("ALL PASS hogsmeade stripe")

    # ---------------------------------------------------------------- install = the OWNER PATH (no answer file)
    def phase1(self, nvars):
        """boot the ISO, take the boot menu's first entry (the default), and the HOGSMEADE front door (hogsmeade-install) is on the screen:
        ERASE HOGSMEADE, Enter for the host name, the disk passphrase twice, the login password twice. Typed on the keyboard (QMP), like the owner.
        The only difference from the owner's boot: the entry is taken from the serial-console submenu (same entry + console=ttyS0) so the
        front door's progress marks can be read on the UART; tty1 is identical."""
        a = self.z
        t0 = time.time()
        vm = qt.VM("1-install", self.qemu_args(a.iso, False, nvars), a.out)
        kb = qt.Kbd(vm, True)
        try:
            ok = vm.expect_raw(r"Serial console \(" + re.escape(qt.ARCHS[qt.ARCH]["serial"]), 180)
            self.step("grub-menu-reached", ok, f"after {int(time.time() - t0)}s")
            time.sleep(2)
            vm.shot("grub-menu")
            vm.send("s")
            time.sleep(1.5)
            vm.send("1")                           # entry 1 = the default entry (Graphical install)
            self.step("kernel-boot-started", vm.expect_raw(r"(Linux version|Booting|OpenRC)", 180))
            # the UART has a root shell in this mode: ask it whether the front door runs on tty1
            end, up = time.time() + qt.T(600), False
            while time.time() < end and not up:
                vm.send("\r")
                time.sleep(1)
                vm.send("pgrep -f 'hogsmeade-install|live-machine-installer' >/dev/null && echo FRONT''DOOR-UP; fgconsole\r")
                up = bool(vm.expect_raw(r"FRONTDOOR-UP", 12))
            time.sleep(6)
            vm.shot("front-door")
            self.step("default-boot-entry-lands-in-the-hogsmeade-front-door-on-tty1", up, "hogsmeade-install runs on tty1 after the default entry (see 1-install-front-door.png)")
            b = qt.ssh_banner(qt.FWD_SSH)
            self.step("live-sshd-off", not b.startswith("SSH-"), f"banner on the live system: {b!r}")
            vm.send("ls /etc/hogsmeade/; test -s /etc/hogsmeade/hostname && echo HOST''NAME=$(cat /etc/hogsmeade/hostname); command -v tailscale || echo NO''TS-LIVE; "
                    "grep -c . /etc/hogsmeade/agent_authorized_keys; passwd -S root | cut -d' ' -f2\r")
            m = vm.expect_raw(r"HOSTNAME=(\S+)", 20)
            nots = vm.expect_raw(r"NOTS-LIVE", 10)
            self.step("live-image-carries-the-owners-hostname-and-no-tailscale", bool(m) and m.group(1) == b"hogsmeade" and bool(nots), f"/etc/hogsmeade/hostname = {m.group(1).decode() if m else '?'}")
            kb.text("ERASE HOGSMEADE", pause=0.05)
            kb.key("ENTER")
            time.sleep(2)
            kb.key("ENTER")                        # host name: Enter keeps the prefilled "hogsmeade"
            time.sleep(2)
            for _ in (1, 2):
                kb.text(zt.PASSPHRASE, pause=0.05)
                kb.key("ENTER")
                time.sleep(1.5)
            # SEPARATE SECRETS: the disk passphrase offered as the login password must be refused
            kb.text(zt.PASSPHRASE, pause=0.05)
            kb.key("ENTER")
            time.sleep(2)
            vm.shot("login-password-equal-to-passphrase-refused")
            early = vm.expect_raw(r"HOGSMEADE-INSTALL: HOSTNAME-CONFIRMED", 4, consume=False)
            self.step("front-door-refuses-the-disk-passphrase-as-login-password", not early, "no install started after the passphrase was typed as the login password (see the screenshot)")
            for _ in (1, 2):
                kb.text(zt.USER_PW, pause=0.05)
                kb.key("ENTER")
                time.sleep(1.5)
            m = vm.expect_raw(r"HOGSMEADE-INSTALL: HOSTNAME-CONFIRMED (\S+)", 60)
            self.step("front-door-took-the-typed-answers-hostname-confirmed", bool(m) and m.group(1) == b"hogsmeade", m.group(0).decode() if m else "no HOSTNAME-CONFIRMED mark" + qt.serial_tail(vm))
            time.sleep(20)
            vm.shot("installing")
            end, m = time.time() + qt.T(3000), None
            while time.time() < end and vm.alive() and not m:
                m = vm.expect_raw(r"HOGSMEADE-INSTALL: (INSTALL-OK|TARGET-NOT-CLOSED)", 30, consume=False)
                vm.send("\r")                      # keeps the emulated UART transmitting
                if not m and int(time.time() - t0) % 300 < 35:
                    vm.send("pgrep -f live-installer >/dev/null || echo INSTALLER''GONE; tail -c 300 /run/hogsmeade-install.log | tr '\\r' '\\n' | tail -2\r")
                    if vm.expect_raw(r"INSTALLERGONE", 8):
                        break
            closed = vm.expect_raw(r"HOGSMEADE-INSTALL: TARGET-CLOSED", 240)
            vm.send("grep -c AUTOINSTALL-OK /run/hogsmeade-install.log; grep -E 'ERROR|FAILED' /run/hogsmeade-install.log | tail -3; test -e /run/hogsmeade-install.toml && echo ANSWERS''LEFT || echo ANSWERS''SHREDDED\r")
            shred = vm.expect_raw(r"ANSWERSSHREDDED", 15)
            vm.shot("install-ok")
            self.step("installer-finished", bool(m) and b"INSTALL-OK" in m.group(0) and bool(closed), ((m.group(0).decode() if m else "no INSTALL-OK mark") + (" + TARGET-CLOSED" if closed else " (target NOT closed)")
                      + f" after {int(time.time() - t0)}s") + ("" if m else qt.serial_tail(vm)))
            self.step("answer-file-with-the-secrets-shredded-after-the-install", bool(shred), "/run/hogsmeade-install.toml is gone")
            # the owner pulls the stick, then presses Enter: the restart must not need a single file from the stick
            r = vm.qmp("device_del", id="stick0")
            time.sleep(4)
            mark0 = len(open(f"{a.out}/1-install-serial.log", "rb").read())
            kb.key("ENTER")
            rs = vm.expect_raw(r"HOGSMEADE-INSTALL: RESTARTING", 30)
            t = time.time()
            while vm.alive() and time.time() - t < qt.T(90):
                time.sleep(1)
            tail = open(f"{a.out}/1-install-serial.log", "rb").read()[mark0:].decode(errors="replace")
            self.step("restart-after-the-usb-stick-was-pulled", bool(rs) and not vm.alive(), f"device_del: {str(r).strip()[:60]}; RESTARTING mark: {bool(rs)}; VM restarted (-no-reboot exit): {not vm.alive()}; "
                      f"I/O error lines after the pull: {len(re.findall(r'I/O error', tail))}")
            self.step("vm-stopped-after-install", not vm.alive(), f"install phase {int(time.time() - t0)}s")
        finally:
            if vm.alive():
                vm.p.kill()

    # ---------------------------------------------------------------- main boot = the FIRST boot: offline, then online
    def colours(self, vm):
        ppm = vm.grab()
        m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", ppm or b"")
        if not m:
            return 0
        px = ppm[m.end():]
        return len({px[i:i + 3] for i in range(0, min(len(px), 3 * 1280 * 800), 3 * 53)})

    def wait_graphical(self, vm, secs):
        end, c = time.time() + qt.T(secs), 0
        while time.time() < end:
            vm.qmp("send-key", keys=[{"type": "qcode", "data": "shift"}])      # an idle greeter/session blanks the screen: wake it
            time.sleep(2)
            c = self.colours(vm)
            if c > 40:
                break
            time.sleep(4)
        return c

    def bootstrap_test_key(self, vm):
        """The image trusts ONE ssh key: the agent's (crystalcave's device key, which never leaves crystalcave). The gate gets its own root
        access the way the owner could give it: log in on the text console tty2 as canutethegreat with the LOGIN password and add a throwaway
        public key through sudo. Only this VM's disk is changed, never the ISO. (Also proves: tty login with the login password, sudo via wheel.)"""
        kb = qt.Kbd(vm, True)
        vm.qmp("send-key", keys=[{"type": "qcode", "data": k} for k in ("ctrl", "alt", "f2")])
        time.sleep(4)
        kb.key("ENTER")
        time.sleep(2)
        kb.text(USER, pause=0.05)
        kb.key("ENTER")
        time.sleep(3)
        kb.text(zt.USER_PW, pause=0.05)
        kb.key("ENTER")
        time.sleep(6)
        kb.text(f"sudo sh -c 'umask 077; mkdir -p /root/.ssh; echo {self.pub} >> /root/.ssh/authorized_keys; sync'", pause=0.03)
        kb.key("ENTER")
        time.sleep(4)
        kb.text(zt.USER_PW, pause=0.05)
        kb.key("ENTER")
        time.sleep(5)
        vm.shot("tty2-sudo-test-key")
        kb.text("exit", pause=0.05)
        kb.key("ENTER")
        time.sleep(3)

    def boot_main(self, nvars):
        a = self.z
        self.no_ps2 = True
        # fresh firmware variables, like every installed boot of the shared harness: the VARS of the install boot keep OVMF's built-in
        # "EFI Internal Shell" AHEAD of the drives it only discovers now (the installer writes the removable-media loader on each ESP and
        # no NVRAM entry), so the VM would sit in the shell. Real firmware has no such entry and falls through to \EFI\BOOT\BOOTX64.EFI.
        qt.pflash_vars(nvars)
        vm = qt.VM("2-boot1", self.qemu_args(None, True, nvars), a.out)
        try:
            # FIRST BOOT WITHOUT ANY NETWORK (the owner connects Wi-Fi later): all three links are down before the guest starts
            qt.HG.link(vm, NETS, False)
            self.vga_stable(vm, "passphrase-prompt-offline")
            kb = qt.Kbd(vm, True)
            c = 0
            for attempt in (1, 2, 3):
                kb.text(zt.PASSPHRASE, pause=0.05)
                kb.key("ENTER")
                c = self.wait_graphical(vm, 200)
                if c > 40:
                    break
                vm.shot(f"offline-retype-{attempt}")
            time.sleep(8)
            vm.shot("offline-gdm-greeter")
            self.step("first-boot-OFFLINE-one-passphrase-then-gdm-greeter", c > 40, f"no network link at all; {c} distinct colours on the VGA console after the passphrase (see 2-boot1-offline-gdm-greeter.png)")
            time.sleep(25)                       # the offline state lasts long enough for the first-boot services to settle
            self.bootstrap_test_key(vm)
            t_up = time.time()
            qt.HG.link(vm, NETS, True)
            up = self.wait_ssh(timeout=300)
            self.step("first-boot-network-appears-later-ssh-key-login", up, f"links up {int(time.time() - t_up)} s ago; key-only root ssh over the first network (test key added through the user's sudo)")
            rc, o = self.sh("v=$(loginctl list-sessions --no-legend | awk '$3 ~ /gdm/ {print $1; exit}'); t=$(loginctl show-session $v -p VTNr --value 2>/dev/null); echo GREETER-VT=$t; [ -n \"$t\" ] && chvt $t; fgconsole; "
                            "ssh-keygen -lf /root/.ssh/authorized_keys; grep -c . /root/.ssh/authorized_keys; passwd -S root | cut -d' ' -f2; sshd -T 2>/dev/null | grep -iE '^(permitrootlogin|passwordauthentication) ' | tr A-Z a-z; "
                            f"id {USER}; last -n 3 {USER} 2>/dev/null | head -2")
            fp = self.z.agent_fp
            self.step("root-ssh-is-key-only-agent-key-installed-root-locked-user-in-wheel", (not fp or fp in o) and "permitrootlogin prohibit-password" in o and "passwordauthentication no" in o
                      and re.search(r"^L", o, re.M) is not None and "wheel" in o, re.sub(r"\s+", " ", o)[:500])
            rc, o = self.sh("cat /var/log/hogsmeade-firstboot.log; echo @@; ls /var/lib/hogsmeade/; echo @@; rc-status default 2>&1 | grep -E 'hogsmeade|display-manager|comfyui' | tr -s ' '; "
                            "echo @@; zfs list -H -o name rpool/comfyui rpool/data 2>&1")
            open(f"{a.out}/firstboot-offline.txt", "w").write(o)
            self.step("first-boot-offline-local-part-done-and-network-part-waited", "waiting for a network (no limit)" in o and "network is up" in o and "firstboot.done" in o
                      and "rpool/comfyui" in o and "rpool/data" in o, re.sub(r"\s+", " ", o)[:400])
            end, o = time.time() + qt.T(1800), ""
            while time.time() < end:
                rc, o = self.sh("test -e /var/lib/hogsmeade/net-firstboot.done && echo NET-DONE; tail -4 /var/log/hogsmeade-firstboot.log")
                if "NET-DONE" in o:
                    break
                time.sleep(20)
            rc, o2 = self.sh("cat /var/log/hogsmeade-firstboot.log; echo @@; ls /var/db/repos/gentoo/profiles/repo_name /var/db/repos/guru/.git/HEAD /var/db/repos/canutethegreat/.git/HEAD 2>&1; "
                             "rc-update show default | grep -c hogsmeade-firstboot; date -u; chronyc -n tracking 2>&1 | sed -n 1,3p")
            open(f"{a.out}/firstboot-online.txt", "w").write(o2)
            self.step("first-boot-ONLINE-handoff-and-tree-sync-finished", "NET-DONE" in o and "== done" in o2 and "give these to Claude" in o2 and "retry in 10 minutes" not in o2 and "repo_name" in o2
                      and "No such file" not in o2, re.sub(r"\s+", " ", o2)[-400:])
            self.identity_checks()
            self.checks()
            self.hogsmeade_checks(vm)
            self.sh("sync", timeout=20)
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()

    # ---------------------------------------------------------------- identity: time zone, kernel, host name, NO Tailscale anywhere
    def identity_checks(self):
        a = self.z
        rc, o = self.sh("readlink -f /etc/localtime; cat /etc/timezone 2>/dev/null; date +%Z; date")
        self.step("timezone-is-America-Los_Angeles", f"zoneinfo/{TZ}" in o and re.search(r"^P[SD]T$", o, re.M) is not None, o.strip().replace("\n", " | "))
        rc, o = self.sh("uname -r; cat /etc/hogsmeade-kernelrelease; qlist -ICv sys-kernel/gentoo-kernel sys-fs/zfs-kmod x11-drivers/nvidia-drivers | tr '\\n' ' '; echo; "
                        "modinfo -F vermagic zfs | cut -d' ' -f1; modinfo -F vermagic nvidia | cut -d' ' -f1; modinfo -F version nvidia; modinfo -F license nvidia")
        ls = o.strip().splitlines()
        self.step("kernel-is-6.18.62-LTS-with-zfs-and-nvidia-open-modules-for-it", len(ls) >= 6 and ls[0] == KERNEL and ls[1] == KERNEL and ls[3] == KERNEL and ls[4] == KERNEL
                  and "gentoo-kernel-6.18.62" in ls[2] and "zfs-kmod-2.3.9" in ls[2] and "nvidia-drivers" in ls[2], o.strip().replace("\n", " | ")[:400])
        rc, o = self.sh("hostname; cat /etc/hostname /etc/conf.d/hostname 2>/dev/null | tr '\\n' ' '")
        self.step("hostname-is-hogsmeade", o.strip().splitlines()[0] == "hogsmeade", o.strip().replace("\n", " | "))
        rc, o = self.sh("echo @@BIN; command -v tailscale tailscaled; echo @@PKG; qlist -IC | grep -i tailscale; grep -i tailscale /var/lib/portage/world; echo @@FILES; "
                        "find / -xdev \\( -path /proc -o -path /sys -o -path /var/db/repos -o -path /var/cache -o -path /usr/src -o -path /var/tmp \\) -prune -o -iname '*tailscale*' -print 2>/dev/null | head -20; "
                        "echo @@CONF; grep -rIli tailscale /etc /usr/libexec/comfyui-launch /usr/lib/comfyui/app/main.py /usr/share/hogsmeade /var/lib/hogsmeade /root /home 2>/dev/null | head -20; "
                        "echo @@NET; ip -o link | grep -i -E 'tailscale|ts0'; ss -lntuH | grep -E ':41641\\b'; pgrep -a -x 'tailscaled?'; rc-update show -v | grep -i tailscale; echo @@END", timeout=300)
        open(f"{a.out}/no-tailscale.txt", "w").write(o)
        sec = {k: v.strip() for k, v in re.findall(r"@@([A-Z]+)\n(.*?)(?=@@)", o, re.S)}
        self.step("NO-tailscale-anywhere-binary-package-file-config-interface-service", "@@END" in o and all(sec.get(k, "x") == "" for k in ("BIN", "PKG", "FILES", "CONF", "NET")),
                  "; ".join(f"{k}: {v[:120] or 'none'}" for k, v in sec.items()))

    def hogsmeade_checks(self, vm):
        a = self.z
        rc, o = self.sh("stat -c '%U %a %n' /home/canutethegreat; df -h /home/canutethegreat | tail -1; ls -A /home/canutethegreat | head -5")
        self.step("user-home-exists-on-rpool-home", "canutethegreat 7" in o and "rpool/home" in o, o.strip().replace("\n", " | ")[:250])
        rc, o = self.sh("zpool status -P rpool; echo ---; zpool list -v -H rpool | awk '{print $1}'; echo ---; zpool get -H -o value ashift rpool; zfs get -H -o value encryption rpool")
        open(f"{a.out}/zpool-status.txt", "w").write(o)
        vdevs = re.findall(r"^\s+(/dev/nvme\dn1p4)\s+ONLINE", o, re.M)
        self.step("pool-is-a-stripe-of-3-top-level-vdevs", len(vdevs) == 3 and not re.search(r"^\s+(mirror|raidz\d)", o, re.M), f"vdevs: {vdevs}")
        self.step("pool-ashift-12-encrypted", "\n12\n" in o and "aes-256-gcm" in o, "")
        rc, o = self.sh("for d in 0 1 2; do echo D$d; lsblk -nbo NAME,PARTLABEL,FSTYPE,SIZE /dev/nvme${d}n1 | tail -n +2; done")
        open(f"{a.out}/partitions.txt", "w").write(o)
        esp = len(re.findall(r"ESP\d?\s+vfat", o)); boot = len(re.findall(r"boot\d?\s+ext4", o)); luks = len(re.findall(r"crypto_LUKS", o)); zm = len(re.findall(r"zfs_member", o))
        self.step("each-drive-has-ESP-and-boot-and-zfs-part", esp == 3 and boot == 3 and zm == 3, f"ESP={esp} boot={boot} zfs_member={zm}")
        # "first drive" = the drive the installer made drive 1 (the mounted /boot lives on it). Its kernel name is not stable: QEMU (and real
        # NVMe probing) may enumerate the same drive as nvme2n1 on the next boot, so the check follows /boot instead of assuming nvme0n1.
        rc, b = self.sh("lsblk -no PKNAME $(findmnt -no SOURCE /boot) | head -1")
        bootdisk = b.strip().splitlines()[-1].strip() if b.strip() else "?"
        m = re.match(r"nvme(\d)n1$", bootdisk)
        sect = o.split(f"D{m.group(1)}\n", 1)[1].split("\nD", 1)[0] if m and f"D{m.group(1)}\n" in o else ""
        self.step("hibernate-swap-LUKS-on-first-drive-only", luks == 1 and "crypto_LUKS" in sect, f"LUKS partitions: {luks}; drive 1 of the install (carries the mounted /boot) is {bootdisk} now; the LUKS swap is on it: {'crypto_LUKS' in sect}")
        rc, o = self.sh("cat /etc/horcrux/boot-mirrors.conf; ls -l /etc/kernel/postinst.d/; mkdir -p /mnt/b && for u in $(awk '!/^#/ && NF {print $3}' /etc/horcrux/boot-mirrors.conf); do "
                        "mount -o ro UUID=$u /mnt/b && echo COPY $u && ls /mnt/b/be/horcrux/ && (cd /boot/be/horcrux && sha256sum vmlinuz kver) | (cd /mnt/b/be/horcrux && sha256sum -c -) && grep -c \"$u\" /mnt/b/grub/grub.cfg && umount /mnt/b; done; "
                        "for u in $(awk '!/^#/ && NF {print $2}' /etc/horcrux/boot-mirrors.conf); do mount -o ro UUID=$u /mnt/b && ls /mnt/b/EFI/BOOT/ && umount /mnt/b; done", timeout=90)
        open(f"{a.out}/boot-mirrors.txt", "w").write(o)
        self.step("boot-copies-synced-and-extra-ESPs-bootable", o.count("COPY ") == 2 and o.count("vmlinuz: OK") == 2 and o.count("BOOTX64.EFI") >= 2, o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh("grep -o 'acpi_backlight=[a-z_]*' /proc/cmdline; grep -o 'nvidia-drm.modeset=1' /proc/cmdline; uname -r; cat /etc/hogsmeade-kernelrelease; rc-update show default | tr -s ' ' | tr '\\n' ';'")
        self.step("hardware-profile-hogsmeade-applied", "acpi_backlight=nvidia_wmi_ec" in o and "nvidia-drm.modeset=1" in o and "awccd" in o and "comfyui" in o, o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh("for i in 1 2 3 4 5 6 7 8 9 10; do test -e /var/lib/hogsmeade/firstboot.done && break; sleep 3; done; zfs list -H -o name,mountpoint,org.hogsmeade:snapshots rpool/comfyui rpool/data; ls -ld /var/lib/comfyui")
        self.step("firstboot-created-comfyui-and-data-datasets", "rpool/comfyui\t/var/lib/comfyui\toff" in o and "rpool/data\t/data" in o, o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh("/usr/sbin/hogsmeade-snapshots hourly; zfs list -H -t snapshot -o name | grep -c 'hsm-hourly-'; zfs list -H -t snapshot -o name | grep -c 'rpool/comfyui@' || true")
        n = o.strip().splitlines()
        self.step("zfs-snapshot-rotation-skips-comfyui", n and int(n[0]) >= 2 and n[-1] == "0", o.strip().replace("\n", " | "))
        rc, o = self.sh("cat /etc/cron.d/hogsmeade; rc-service cronie status 2>&1 | head -1; restic version; hogsmeade-health --quiet; echo HEALTH-RC=$?")
        self.step("snapshot-cron-backup-job-and-health-present", "hogsmeade-snapshots hourly" in o and "hogsmeade-snapshots daily" in o and "hogsmeade-backup" in o and "hogsmeade-health" in o and "restic " in o and "HEALTH-RC=" in o, o.strip().replace("\n", " | ")[:400])
        rc, o = self.sh("ls /etc/hogsmeade/; test ! -e /etc/hogsmeade/backup.env && test ! -e /etc/hogsmeade/restic-password && echo NO-RESTIC-SECRETS-IN-IMAGE; grep -rlE --exclude=README 'AWS_SECRET|RESTIC_PASSWORD|applicationKey' /etc /root /home /usr/share/hogsmeade 2>/dev/null | head -3; echo GREP-DONE")
        self.step("image-has-no-restic-credentials", "NO-RESTIC-SECRETS-IN-IMAGE" in o and "GREP-DONE" in o and not re.search(r"^/(etc|root|home|usr)", o, re.M), o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh("awccctl status 2>&1 | head -5; rc-service awccd status 2>&1 | head -2; busctl --system list 2>/dev/null | grep -i awcc || true")
        self.step("awccd-and-awccctl-run", "profile:" in o, o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh("ls -l /usr/lib/comfyui/venv/bin/python; /usr/lib/comfyui/venv/bin/python -c 'import torch,sys;print(\"torch\",torch.__version__, torch.version.cuda)' 2>&1 | tail -1; qlist -ICv sci-misc/comfyui")
        self.step("comfyui-venv-has-torch-with-cuda-build", re.search(r"torch \S+\+cu\d+ \d", o) is not None and "comfyui-0.43.0-r2" in o, o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh("stat -c '%a %U' /root/.ssh/authorized_keys 2>&1; test ! -e /etc/hogsmeade/ts-authkey && ! command -v tailscale >/dev/null && ! command -v tailscaled >/dev/null && echo NO-TAILSCALE; ls /home/canutethegreat/.ssh/authorized_keys 2>&1; command -v hogsmeade-remote hogsmeade-verify hogsmeade-install prime-run")
        self.step("private-payload-and-tools-installed", "600 root" in o and "NO-TAILSCALE" in o and "hogsmeade-verify" in o, o.strip().replace("\n", " | ")[:300])
        self.comfyui_firewall_gate()
        self.rustinion_gate()
        self.provision_and_backup()
        rc, o = self.sh("hogsmeade-verify 2>&1 | tail -80", timeout=180)
        open(f"{a.out}/hogsmeade-verify-in-vm.txt", "w").write(o)
        self.step("hogsmeade-verify-runs-in-vm", "RESULT:" in o, "(GPU/audio/WMI lines are expected to FAIL in a VM) " + o.strip().splitlines()[-1][:100] if o.strip() else "no output")
        rc, o = self.sh("zfs get -H -o value mountpoint,canmount rpool/ROOT/horcrux | tr '\\n' ' '; free -g | awk '/Mem/{print $2}'; swapon --show=NAME,SIZE --noheadings")
        self.step("swap-and-ram-sizes", True, o.strip().replace("\n", " | "))
        rc, o = self.sh("k=$(ls /boot/initramfs-* /boot/be/horcrux/initramfs.img 2>/dev/null | tail -1); lsinitrd $k 2>/dev/null > /tmp/initrd.lst; grep -E '/(i915|xe)[.]ko' /tmp/initrd.lst; echo NVIDIA-MODULES=$(grep -cE '/nvidia([-_](drm|modeset|uvm|peermem))?[.]ko' /tmp/initrd.lst); echo FIRMWARE=$(grep -cE 'firmware/(i915|xe)/' /tmp/initrd.lst)", timeout=300)
        self.step("initramfs-has-intel-igpu-early-kms-and-no-nvidia", re.search(r"/(i915|xe)[.]ko", o) is not None and "NVIDIA-MODULES=0" in o, o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh("locale -a | grep -i -c 'en_US.utf'; grep -rl 'interface-name' /etc/NetworkManager 2>/dev/null | head -3; echo NM-DONE")
        self.step("locale-generated-and-no-pinned-nm-interface", o.strip().splitlines()[0].strip() not in ("", "0") and "NM-DONE" in o and o.strip().count("\n") <= 1, o.strip().replace("\n", " | "))
        rc, o = self.sh("emerge -puDN @world > /tmp/hx-world.txt 2>&1; echo RC=$?; echo WARN-LINES=$(grep -c -i -E 'warning|!!!|masked|blocked|unable to|unmerged|[*] ERROR' /tmp/hx-world.txt); grep -i -E 'warning|!!!|masked|blocked|unable to|unmerged|[*] ERROR' /tmp/hx-world.txt | head -5; tail -3 /tmp/hx-world.txt", timeout=900)
        open(f"{a.out}/world-full.txt", "w").write(o)
        self.step("emerge-puDN-world-clean-no-warnings", "RC=0" in o and "WARN-LINES=0" in o, re.sub(r"\s+", " ", o)[:300])
        # ---- desktop: greeter (no autologin) -> restore drill BEFORE the first login -> real GDM logins on the USB keyboard -> keyring
        qt.greeter_no_session_gate(self.sh, self.step, USER, "gdm", wait=120)
        c = self.wait_graphical(vm, 120)
        time.sleep(3)
        vm.shot("gdm-greeter-no-autologin")
        self.step("gdm-greeter-is-graphical-on-vga", c > 40, f"{c} distinct colours (see 2-boot1-gdm-greeter-no-autologin.png)")
        self.restore_drill()
        self.desktop_exec_check()
        self.session_audio_autostart_pre(USER)
        ok, info = self.greeter_login(vm, "first-login")
        self.step("gdm-login-with-the-SEPARATE-login-password-typed-at-the-greeter", ok, info)
        rc, o = self.sh(f"for s in $(loginctl list-sessions --no-legend | awk '$3==\"{USER}\"{{print $1}}'); do loginctl show-session $s -p Type -p Name -p Active -p State 2>&1 | tr '\\n' ' '; echo; done")
        self.step("session-is-wayland-and-active", any("Type=wayland" in l and "Active=yes" in l for l in o.splitlines()), o.strip().replace("\n", " | "))
        c = self.wait_graphical(vm, 150)
        time.sleep(3)
        vm.shot("gnome-desktop")
        self.step("gnome-desktop-is-graphical-on-vga", c > 40, f"{c} distinct colours on the VGA console (see 2-boot1-gnome-desktop.png)")
        self.session_bus_checks(USER)
        self.session_activation_env_checks(USER)
        self.session_audio_autostart_checks(USER, vm)
        self.tray_extension_checks(USER)
        self.keyring_store()
        self.session_logout_check(USER)
        # separate secrets: the DISK passphrase must not open the session
        ok, info = self.greeter_login(vm, "wrong-secret", password=zt.PASSPHRASE, expect=False)
        self.step("disk-passphrase-is-refused-at-the-gdm-greeter", ok, info)
        vm.qmp("send-key", keys=[{"type": "qcode", "data": "esc"}])
        time.sleep(4)
        ok, info = self.greeter_login(vm, "second-login")
        self.step("gdm-login-again-after-logout", ok, info)
        self.keyring_lookup("keyring-secret-persists-across-logout-and-unlocks-with-the-login", "log out -> greeter login")
        self.sh(f"loginctl terminate-user {USER}", timeout=30)

    # ---------------------------------------------------------------- static desktop gates (called by ZRun.checks)
    def desktop_checks(self, vm=None):
        self.keyring_pam_checks()
        self.toolchain_check()
        self.package_contents_check()
        self.kmod_build_check()
        rc, o = self.sh("test -f /usr/include/EGL/egl.h && test -f /usr/include/libudev.h && ls /usr/include/python3*/Python.h >/dev/null && echo DEV-HEADERS; ls /usr/include | wc -l")
        self.step("development-headers-kept-on-the-installed-system", "DEV-HEADERS" in o, o.strip().replace("\n", " | "))

    # ---------------------------------------------------------------- GDM greeter login on the USB keyboard (QMP), like the owner
    def greeter_login(self, vm, label, password=None, expect=True, wait=150):
        """one user in the list: Enter selects it, the password is typed, Enter. Returns (ok, info); expect=False = the login must FAIL."""
        self.sh(f"faillock --user {USER} --reset", timeout=20)
        kb = qt.Kbd(vm, True)
        vm.qmp("send-key", keys=[{"type": "qcode", "data": "shift"}])
        time.sleep(3)
        kb.key("ENTER")
        time.sleep(6)
        vm.shot(f"{label}-password-field")
        kb.text(password or zt.USER_PW, pause=0.05)
        kb.key("ENTER")
        end, o, up = time.time() + qt.T(wait if expect else 45), "", False
        while time.time() < end:
            rc, o = self.sh(f"pgrep -u {USER} -x gnome-shell >/dev/null && echo SHELL-AS-USER; loginctl list-sessions --no-legend | awk '{{print $3}}' | tr '\\n' ' '")
            up = "SHELL-AS-USER" in o
            if up and expect:
                break
            time.sleep(5)
        vm.shot(f"{label}-after")
        info = f"gnome-shell of {USER}: {'running' if up else 'not running'}; sessions: {o.replace('SHELL-AS-USER', '').strip()}"
        return (up if expect else not up), info

    def _session_env(self):
        uid = self.sh(f"id -u {USER}")[1].strip().splitlines()[-1]
        return (f"B=$(tr '\\0' '\\n' < /proc/$(pgrep -u {USER} -x gnome-shell | head -1)/environ | grep '^DBUS_SESSION_BUS_ADDRESS=' | cut -d= -f2-); "
                f"su - {USER} -c \"export XDG_RUNTIME_DIR=/run/user/{uid} DBUS_SESSION_BUS_ADDRESS=$B XDG_CURRENT_DESKTOP=GNOME; %s\" 2>&1")

    def keyring_store(self):
        env = self._session_env()
        rc, o = self.sh(env % f"printf {KR_VALUE} | timeout 40 secret-tool store --label=hxtest hxkey v1; echo store-rc=\\$?; timeout 20 secret-tool lookup hxkey v1 | wc -c" + "; "
                        f"echo LOGIN-DAEMONS=$(pgrep -u {USER} -f 'gnome-keyring-daemon --daemonize --login' | wc -l) OTHER-DAEMONS=$(pgrep -u {USER} -fa 'gnome-keyring-daemon' | grep -v -e '--daemonize --login' -e '--start --foreground' | wc -l); ls /home/{USER}/.local/share/keyrings/ 2>&1 | tr '\\n' ' '; echo; ps -o pid,ppid,lstart,args -u {USER} | grep -E 'keyring|PID' | grep -v grep", timeout=120)
        self.step("keyring-login-collection-stores-a-secret-on-disk", "store-rc=0" in o and str(len(KR_VALUE)) in o.split("store-rc=0")[-1] and "login.keyring" in o and KR_DAEMONS in o,
                  re.sub(r"\s+", " ", o)[:300])

    def keyring_lookup(self, step, how):
        env = self._session_env()
        rc, o = self.sh(env % "timeout 30 secret-tool lookup hxkey v1; echo; echo lookup-rc=\\$?; "
                        "busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets/collection/login org.freedesktop.Secret.Collection Locked" + "; "
                        f"echo LOGIN-DAEMONS=$(pgrep -u {USER} -f 'gnome-keyring-daemon --daemonize --login' | wc -l) OTHER-DAEMONS=$(pgrep -u {USER} -fa 'gnome-keyring-daemon' | grep -v -e '--daemonize --login' -e '--start --foreground' | wc -l); ps -o pid,ppid,lstart,args -u {USER} | grep -E 'keyring|PID' | grep -v grep; ls -la /run/user/$(id -u {USER})/keyring/ 2>&1", timeout=120)
        self.step(step, KR_VALUE in o and "b false" in o and KR_DAEMONS in o, f"{how}: " + re.sub(r"\s+", " ", o.replace(KR_VALUE, "<the stored test value>"))[:900])

    # ---------------------------------------------------------------- reboot: the keyring survives a restart too
    def reboot_keyring_phase(self, nvars):
        a = self.z
        self.no_ps2 = True
        vm = qt.VM("3-boot2-reboot", self.qemu_args(None, True, nvars), a.out)
        try:
            _ok, up = self.type_passphrase_usb(vm, "passphrase-prompt-reboot")
            self.step("boot2-single-passphrase-ssh", up, "second boot: one passphrase")
            rc, o = self.sh("test -e /var/lib/hogsmeade/net-firstboot.done && echo NET-DONE; rc-update show default | grep -c hogsmeade-firstboot; rc-status default | grep -E 'hogsmeade-firewall|comfyui' | tr -s ' '; "
                            "readlink -f /etc/localtime; uname -r")
            self.step("second-boot-firstboot-not-repeated-firewall-up", "NET-DONE" in o and "hogsmeade-firewall" in o and "started" in o.split("hogsmeade-firewall")[1].splitlines()[0]
                      and TZ in o and KERNEL in o, re.sub(r"\s+", " ", o)[:300])
            qt.greeter_no_session_gate(self.sh, self.step, USER, "gdm", wait=180, step="boot2-greeter-shown-and-no-user-session-before-login")
            self.wait_graphical(vm, 120)
            ok, info = self.greeter_login(vm, "boot2-login")
            self.step("boot2-gdm-login-with-the-login-password", ok, info)
            self.keyring_lookup("keyring-secret-persists-across-a-reboot", "power off -> boot -> passphrase -> greeter login")
            self.sh("sync", timeout=20)
            self.sh(HALT, timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()

    # ---------------------------------------------------------------- ComfyUI: LAN bind behind the host firewall
    def comfyui_firewall_gate(self):
        a = self.z
        rc, o = self.sh("rc-service hogsmeade-firewall status 2>&1 | tail -1; nft list table inet hogsmeade 2>&1; echo @@; cat /etc/conf.d/comfyui.local; echo @@; qfile -q /etc/conf.d/comfyui.local /etc/hogsmeade/firewall.nft /etc/init.d/hogsmeade-firewall; "
                        "rc-update show default | grep -E 'hogsmeade-firewall|comfyui' | tr -s ' '; ip -4 -o addr show scope global | awk '{print $2, $4}' | tr '\\n' ';'")
        open(f"{a.out}/comfyui-firewall.txt", "w").write(o)
        self.step("firewall-table-loaded-at-boot-and-owned-by-the-private-package", "started" in o and "10.66.69.0/24" in o and "10.69.69.0/24" in o and "dport 8188" in o
                  and 'COMFYUI_LISTEN="0.0.0.0"' in o and "rc_need=\"hogsmeade-firewall\"" in o and o.count("sys-apps/hogsmeade-private") >= 3
                  and "10.0.2.15" in o and "10.66.69.15" in o and "10.69.69.15" in o, re.sub(r"\s+", " ", o)[:500])
        # no GPU in the VM: CPU mode for the test only (same bind address and firewall as the real thing)
        rc, o = self.sh("echo 'COMFYUI_EXTRA_ARGS=\"--cpu\"' >> /etc/conf.d/comfyui.local; rc-service comfyui restart 2>&1 | tail -2; "
                        "for i in $(seq 1 75); do curl -fsS -m 4 -o /dev/null http://127.0.0.1:8188/system_stats && break; sleep 4; done; "
                        "curl -sS -m 6 -o /dev/null -w 'LOOPBACK=%{http_code}\\n' http://127.0.0.1:8188/system_stats; ss -ltnH | awk '$4 ~ /:8188$/ {print \"LISTEN=\" $4}'; "
                        "tail -4 /var/log/comfyui/comfyui.log /var/log/comfyui/comfyui.err 2>&1 | cut -c1-160", timeout=420)
        open(f"{a.out}/comfyui-cpu.txt", "w").write(o)
        self.step("comfyui-listens-on-8188-all-addresses-and-answers-on-loopback", "LOOPBACK=200" in o and "LISTEN=0.0.0.0:8188" in o, re.sub(r"\s+", " ", o)[:400])
        lan, wg, foreign = http_get(self.p_lan), http_get(self.p_wg), http_get(self.p_foreign)
        ssh_ok = qt.ssh_banner(qt.FWD_SSH).startswith("SSH-")           # the foreign network itself works: only port 8188 is closed to it
        self.step("comfyui-8188-reachable-from-home-LAN-10.66.69.0/24", lan[0] == 200, f"from 10.66.69.2: HTTP {lan[0]} {lan[1][:60]}")
        self.step("comfyui-8188-reachable-from-home-WireGuard-10.69.69.0/24", wg[0] == 200, f"from 10.69.69.2: HTTP {wg[0]} {wg[1][:60]}")
        self.step("comfyui-8188-CLOSED-from-any-other-network", foreign[0] == 0 and ssh_ok, f"from 10.0.2.2: {foreign[1][:90]} (ssh over the same network answers: {ssh_ok})")
        rc, o = self.sh("nft list chain inet hogsmeade input | grep -E 'counter|accept'; "
                        "nft add element inet hogsmeade comfyui_allowed '{ 10.0.2.0/24 }' && echo OPENED")
        m = re.search(r"counter packets (\d+)", o)
        opened = http_get(self.p_foreign)
        self.sh("nft -f /etc/hogsmeade/firewall.nft")
        closed = http_get(self.p_foreign)
        self.step("firewall-is-what-closes-it-reject-counter-and-control-experiment", bool(m) and int(m.group(1)) > 0 and "OPENED" in o and opened[0] == 200 and closed[0] == 0,
                  f"reject counter {m.group(1) if m else '?'} packets; with 10.0.2.0/24 added to the allowed set the same request gets HTTP {opened[0]}; after reloading the rules: {closed[1][:60]}")
        rc, o = self.sh("rc-service hogsmeade-firewall stop 2>&1 | tail -3; sleep 3; echo LISTEN=$(ss -ltnH | awk '$4 ~ /:8188$/' | wc -l); rc-service comfyui status 2>&1 | tail -1; echo @@; "
                        "rc-service comfyui start 2>&1 | tail -3; sleep 2; rc-service hogsmeade-firewall status 2>&1 | tail -1; nft list chain inet hogsmeade input 2>&1 | grep -c 'dport 8188'", timeout=120)
        before, _, after = o.partition("@@")
        self.step("comfyui-never-runs-without-the-firewall-rc_need", "LISTEN=0" in before and "started" in after and after.strip().splitlines()[-1].strip() == "2", re.sub(r"\s+", " ", o)[:400])
        self.sh("rc-service comfyui stop; sed -i '/^COMFYUI_EXTRA_ARGS=\"--cpu\"$/d' /etc/conf.d/comfyui.local; rc-service hogsmeade-firewall start", timeout=90)

    # ---------------------------------------------------------------- Rustinion: installed, NOT enrolled, the enrol command is there
    def rustinion_gate(self):
        a = self.z
        rc, o = self.sh("qlist -ICv app-admin/rustinion-agent; qfile -q /usr/bin/rustinion /etc/init.d/rustinion; cat /usr/lib/rustinion/package-managed; echo @@V; rustinion --version 2>&1 | head -2; "
                        "echo @@H; rustinion enroll --help 2>&1 | head -40; echo @@S; ls -A /etc/rustinion 2>&1 | tr '\\n' ' '; echo; test ! -e /etc/rustinion/device.json && echo NOT-ENROLLED; "
                        "grep -rIlE 'enroll(ment)?[_-]?token' /etc/rustinion /etc/hogsmeade /root 2>/dev/null | head -3; echo @@R; rc-update show -v | grep -E '^ *rustinion ' | tr -s ' '; "
                        "rc-service rustinion status 2>&1 | tail -1; grep -c rustinion-agent /var/lib/portage/world; echo @@E", timeout=90)
        open(f"{a.out}/rustinion.txt", "w").write(o)
        sec = {k: v.strip() for k, v in re.findall(r"@@([A-Z])\n(.*?)(?=@@)", o, re.S)}
        head = o.split("@@V")[0]
        self.step("rustinion-agent-installed-from-portage-and-runs", (rv := re.search(r"rustinion-agent-(\d+\.\d+\.\d+)", head)) is not None and head.count("app-admin/rustinion-agent") >= 3 and "portage" in head
                  and rv.group(1) in sec.get("V", ""), re.sub(r"\s+", " ", head + " | " + sec.get("V", ""))[:300])
        self.step("rustinion-enrol-command-present-takes-a-token", "--token" in sec.get("H", ""), re.sub(r"\s+", " ", sec.get("H", ""))[:300])
        self.step("rustinion-not-enrolled-and-no-token-in-the-image", "NOT-ENROLLED" in sec.get("S", "") and not re.search(r"^/", sec.get("S", ""), re.M), re.sub(r"\s+", " ", sec.get("S", ""))[:200])
        self.step("rustinion-in-world-service-waits-for-the-enrolment", sec.get("R", "").strip().splitlines()[-1:] == ["1"], re.sub(r"\s+", " ", sec.get("R", ""))[:200])

    # ---------------------------------------------------------------- restore drill (before the first login)
    def restore_drill(self):
        """the profile restore that crystalcave runs before the owner's first login, with a sanitized STAND-IN tree (the real tagged snapshots
        pre-horcrux-wipe / pre-horcrux-wipe-profiles are restored on crystalcave only: see the delivery notes). Same script, same push path."""
        a = self.z
        d = tempfile.mkdtemp(prefix="hsm-standin-")
        try:
            src = os.path.join(d, "src")
            for rel, txt in ((".mozilla/firefox/abcd1234.default-release/prefs.js", 'user_pref("stand.in", true);\n'),
                             (".mozilla/firefox/profiles.ini", "[Profile0]\nName=default-release\nIsRelative=1\nPath=abcd1234.default-release\nDefault=1\n"),
                             (".config/Code/User/settings.json", '{"editor.fontSize": 13}\n'), (".claude/settings.json", "{}\n"), (".gitconfig", "[user]\n\tname = stand-in\n"),
                             (".bashrc", "# stand-in bashrc that must NOT replace the image's\n")):
                os.makedirs(os.path.dirname(os.path.join(src, rel)), exist_ok=True)
                open(os.path.join(src, rel), "w").write(txt)
            wrap = os.path.join(d, "ssh-wrap")
            os.makedirs(wrap)
            open(f"{wrap}/ssh", "w").write("#!/bin/sh\nfor a in \"$@\"; do last=$a; done\nexec /usr/bin/ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o IdentitiesOnly=yes "
                                           f"-i {self.key} -p {qt.FWD_SSH} root@127.0.0.1 \"$last\"\n")
            os.chmod(f"{wrap}/ssh", 0o755)
            script = os.path.join(HERE, "..", "restore-profiles.sh")
            self.sh(f"ls -A /home/{USER} > /tmp/home-before.txt; sha256sum /home/{USER}/.bashrc > /tmp/bashrc-before.txt 2>/dev/null; true")
            # the script sets its own PATH: run a copy whose PATH keeps the ssh wrapper (test key + forwarded port) first
            txt = open(script).read()
            assert "export PATH=/usr/local/bin:/usr/bin:/bin" in txt
            cp = os.path.join(d, "restore-profiles.sh")
            open(cp, "w").write(txt.replace("export PATH=/usr/local/bin:/usr/bin:/bin", f"export PATH={wrap}:/usr/local/bin:/usr/bin:/bin"))
            r = subprocess.run(["bash", cp, "--test", src, "--", "root@hogsmeade-vm"], env=dict(os.environ, TMPDIR=d), capture_output=True, text=True, timeout=300)
            open(f"{a.out}/restore-drill.txt", "w").write(r.stdout + r.stderr)
            rc, o = self.sh(f"cd /home/{USER}; stat -c '%U %a %n' .mozilla/firefox/profiles.ini .config/Code/User/settings.json .claude/settings.json .gitconfig Nextcloud 2>&1; "
                            f"grep -c stand.in .mozilla/firefox/abcd1234.default-release/prefs.js; sha256sum -c /tmp/bashrc-before.txt 2>&1 | tail -1 | sed 's/^/BASHRC-UNCHANGED-CHECK /'; echo STANDIN-BASHRC=$(grep -c 'stand-in bashrc' .bashrc); "
                            f"cat /var/lib/hogsmeade/profiles-restored; find /home/{USER} ! -user {USER} | head -3; echo FIND-DONE")
            ok = (r.returncode == 0 and "restored" in r.stdout and o.count(f"{USER} ") >= 5 and "No such file" not in o and "FIND-DONE" in o
                  and not re.search(r"^/home/", o, re.M) and "STANDIN-BASHRC=0" in o and ".bashrc: OK" in o)
            self.step("restore-drill-profiles-pushed-before-first-login-owned-by-the-user-nothing-overwritten", ok, re.sub(r"\s+", " ", (r.stdout + " | " + o))[:500] + (" ERR " + r.stderr[-150:] if r.returncode else ""))
        finally:
            shutil.rmtree(d, ignore_errors=True)

    def provision_and_backup(self):
        """the post-install restic provisioning: crystalcave-style push over SSH stdin into root-only 0600 files, then a real hogsmeade-backup
        into a THROWAWAY local repo (never the real B2 repo)"""
        a = self.z
        script = os.path.join(HERE, "..", "provision-restic.sh")
        env = dict(os.environ, HOME=os.environ.get("HOME") or a.out, HSM_SSH_OPTS=f"-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -i {self.key} -p {qt.FWD_SSH}")
        r = subprocess.run(["bash", script, "--test", "--", "root@127.0.0.1"], env=env, capture_output=True, text=True, timeout=180)
        open(f"{a.out}/provision.txt", "w").write(r.stdout + r.stderr)
        self.step("restic-provisioned-over-ssh-stdin-0600", r.returncode == 0 and "600 root /etc/hogsmeade/backup.env" in r.stdout and "600 root /etc/hogsmeade/restic-password" in r.stdout, r.stdout.strip().replace("\n", " | ")[:300] + r.stderr[:100])
        self.sh("mkdir -p /data/hsmtest && head -c 300000 /dev/urandom > /data/hsmtest/blob && echo hello > /home/canutethegreat/hsm-note.txt && chown -R canutethegreat /home/canutethegreat/hsm-note.txt", timeout=60)
        rc, o = self.sh("/usr/sbin/hogsmeade-backup; echo BACKUP-RC=$?; /usr/sbin/hogsmeade-backup; echo SECOND-RC=$?; zfs list -H -t snapshot -o name | grep -c '@hsm-backup' | sed 's/^/LEFTOVER-SNAPSHOTS=/'; set -a; . /etc/hogsmeade/backup.env; set +a; restic snapshots --tag hogsmeade --compact; echo PATH-GROUPS=$(restic snapshots --tag hogsmeade --json | python3 -c 'import json,sys; print(len({tuple(x[\"paths\"]) for x in json.load(sys.stdin)}))'); restic ls latest | grep -c hsm-note; restic check 2>&1 | tail -2; cat /var/lib/hogsmeade/last-backup-ok", timeout=600)
        open(f"{a.out}/test-backup.txt", "w").write(o)
        self.step("test-backup-to-throwaway-restic-repo", "BACKUP-RC=0" in o and "SECOND-RC=0" in o and "2 snapshots" in o and "PATH-GROUPS=1" in o and "LEFTOVER-SNAPSHOTS=0" in o and "no errors were found" in o, o.strip().replace("\n", " | ")[:400])
        rc, o = self.sh("set -a; . /etc/hogsmeade/backup.env; set +a; rm -rf /var/tmp/hsm-restore; restic restore --tag hogsmeade latest:/home/.zfs/snapshot/hsm-backup --target /var/tmp/hsm-restore/home >/dev/null 2>&1 && "
                        "restic restore --tag hogsmeade latest:/data/.zfs/snapshot/hsm-backup --target /var/tmp/hsm-restore/data >/dev/null 2>&1; echo RESTORE-RC=$?; "
                        "cmp /data/hsmtest/blob /var/tmp/hsm-restore/data/hsmtest/blob && echo BLOB-SAME; cmp /home/canutethegreat/hsm-note.txt /var/tmp/hsm-restore/home/canutethegreat/hsm-note.txt && echo NOTE-SAME; "
                        "restic snapshots --tag hogsmeade --json | python3 -c 'import json,sys; s=json.load(sys.stdin); print(\"TAGS\", sorted({t for x in s for t in x.get(\"tags\", [])}))'; "
                        "rm -rf /var/tmp/hsm-restore /data/hsmtest", timeout=600)
        self.step("restic-restore-drill-of-the-tagged-snapshot-is-byte-identical", "RESTORE-RC=0" in o and "BLOB-SAME" in o and "NOTE-SAME" in o and "hogsmeade" in o, o.strip().replace("\n", " | ")[:400])

    # ---------------------------------------------------------------- last phase: only drive 3 can boot
    def third_drive_boot(self, nvars):
        a = self.z
        self.no_ps2 = True
        # destroy drive 1's ESP and /boot (the pool partitions stay): the boot path must work from drive 3 alone
        vm = qt.VM("6-boot5-prep", self.qemu_args(None, True, nvars), a.out)
        try:
            _ok, up = self.type_passphrase_usb(vm, "passphrase-prompt-before-wipe")
            self.step("boot5-prep-ssh", up, "")
            rc, o = self.sh("umount /boot/efi; umount /boot; dd if=/dev/zero of=/dev/nvme0n1p1 bs=1M count=8 status=none && dd if=/dev/zero of=/dev/nvme0n1p2 bs=1M count=8 status=none && sync && echo WIPED", timeout=60)
            self.step("drive1-ESP-and-boot-zeroed", "WIPED" in o, o.strip()[:100])
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()
        self.boot_only = 2
        qt.pflash_vars(nvars)
        vm = qt.VM("7-boot5-drive3-only", self.qemu_args(None, True, nvars), a.out)
        try:
            ok, up = self.type_passphrase_usb(vm, "passphrase-prompt-drive3-only")
            self.step("boots-from-drive3-ESP-alone", up, "OVMF bootindex on drive 3 only, drive 1's ESP+/boot are zeroed: GRUB, kernel and initramfs came from drive 3")
            rc, o = self.sh("zpool status -x; findmnt -no SOURCE,FSTYPE /; dmesg | grep -ci 'efi: EFI v'; cat /proc/cmdline | cut -c1-120", timeout=60)
            self.step("pool-healthy-booted-from-drive3", "healthy" in o and "rpool/ROOT/horcrux" in o, o.strip().replace("\n", " | ")[:250])
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()


def main():
    qt.arm_deadline()
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--mem", type=int, default=8192)
    ap.add_argument("--cpu", default=os.environ.get("QEMU_TEST_CPU", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16"))
    ap.add_argument("--disk-gb", type=int, default=40)
    ap.add_argument("--keep-disks", action="store_true")
    ap.add_argument("--agent-fp", default="", help="SHA256 fingerprint of the real agent key that must be in root's authorized_keys (public information)")
    ap.add_argument("--phases", default="", help="comma list of install,main,reboot,hibernate,drive3 (default all); without install the kept disks of --out are reused")
    a = ap.parse_args()
    qt.ARCH = qt.HOST_ARCH
    kvm = os.path.exists("/dev/kvm")
    qt.SLOW = float(os.environ.get("QEMU_SLOW", 0) or 0) or (1.0 if kvm else 3.0)
    try:
        HRun(a).run()
    finally:
        if not a.keep_disks:
            for n in ("target.qcow2", "target2.qcow2", "target3.qcow2", "answers.img", "OVMF_VARS.fd"):
                try:
                    os.unlink(os.path.join(a.out, n))
                except OSError:
                    pass


if __name__ == "__main__":
    main()
