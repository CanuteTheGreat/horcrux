#!/usr/bin/env python3
"""HOGSMEADE QEMU/KVM proof: unattended install of the PRIVATE image onto THREE NVMe drives (one encrypted ZFS pool = a STRIPE of
three top-level vdevs), then boots, GNOME login, hibernate/resume, boot from the third drive's ESP alone, qfile -o and emerge -puDN @world.

  hogsmeade-qemu-test.py --iso X.iso --out DIR [--mem 8192]

Built on common/zfs-enc-test.py (same harness, same steps for the encrypted-ZFS basics) with:
  * 3 emulated NVMe drives (40G/24G/24G) + faked DMI (Alienware 18 Area-51 AA18250) so the `hogsmeade` hardware profile is auto-detected
  * every boot types the passphrase on the USB keyboard at the VGA prompt (the laptop has no UART; QEMU PS/2 off)
  * stripe-specific checks, per-drive ESP + /boot copy, swap on the first drive only, profile services, first-boot datasets, tools
  * last phase: zero ESP+/boot of drive 1, boot with ONLY drive 3 bootable (OVMF bootindex): the system must still come up
GPU, audio, WMI/EC, lighting, Wi-Fi/BT, backlight are NOT provable in a VM: see hosts/hogsmeade/CONVERSION.md section 'first real boot'.
"""
import argparse, importlib.util, json, os, re, subprocess, sys, time

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
SIZES_GB = (40, 40, 40)   # nvme enumeration order is not stable in QEMU: any drive may become nvme0n1, which carries the 9 GiB hibernate swap


class HRun(zt.ZRun):
    def __init__(self, a):
        a.t2, a.fake_t2, a.expect_t2_refusal, a.desktop, a.hibernate = "no", False, False, True, True
        super().__init__(a)
        self.boot_only = None            # index of the only bootable drive (ESP-of-drive-3 phase)
        self.disks = []

    def qemu_args(self, iso, installed, nvars):
        args = super().qemu_args(iso, installed, nvars)
        i = next(k for k, x in enumerate(args) if x == "-drive" and args[k + 1].startswith(f"file={self.disk},if=virtio"))
        new = []
        for n, path in enumerate(self.disks):
            bi = f",bootindex={1}" if (self.boot_only is not None and n == self.boot_only and not iso) else ""
            new += ["-drive", f"file={path},if=none,id=nv{n},format=qcow2", "-device", f"nvme,drive=nv{n},serial=HSMNVME{n:02d}{bi}"]
        args[i:i + 2] = new
        args += ["-smbios", "type=1,manufacturer=Alienware,product=Alienware 18 Area-51 AA18250"]
        return args

    def toml(self, pubkey, pwhash):
        uh = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemuuser1", zt.USER_PW]).decode().strip()
        return "\n".join([
            "[global]", 'keyboard = "us"', 'timezone = "America/Los_Angeles"', 'hostname = "hogsmeade"', 'install_type = "desktop"',
            f'root_password_hash = "{pwhash}"', f'ssh_keys = ["{pubkey}"]', "ssh_password_auth = false", "",
            "[disk]", 'filesystem = "zfs"', 'zfs_raid = "stripe"', 'disks = ["/dev/nvme0n1", "/dev/nvme1n1", "/dev/nvme2n1"]',
            "encryption = true", 'swap = "hibernate"', f'passphrase = "{zt.PASSPHRASE}"', "",
            "[network.management]", 'mode = "dhcp"', "",
            "[user]", f'name = "{USER}"', f'fullname = "{USER}"', f'password_hash = "{uh}"', "autologin = true", ""])

    def run(self):
        a = self.z
        os.makedirs(a.out, exist_ok=True)
        self.disk, self.answers = f"{a.out}/target.qcow2", f"{a.out}/answers.img"
        self.disks = [f"{a.out}/target{n if n else ''}.qcow2" if n else self.disk for n in range(3)]
        self.disks = [self.disk, f"{a.out}/target2.qcow2", f"{a.out}/target3.qcow2"]
        for d, g in zip(self.disks, SIZES_GB):
            subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", d, f"{g}G"])
        nvars = f"{a.out}/OVMF_VARS.fd"
        qt.pflash_vars(nvars)
        self.key = f"{a.out}/test_key"
        for f in (self.key, self.key + ".pub"):
            if os.path.exists(f):
                os.unlink(f)
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "qemu-test", "-f", self.key])
        self.pub = open(self.key + ".pub").read().strip()
        pwhash = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemutest1", qt.PW]).decode().strip()
        qt.make_answer_image(self.answers, "horcrux", self.toml(self.pub, pwhash))
        self.step("answer-file-built", True, "stripe over 3 NVMe, encryption=true, swap=hibernate, user canutethegreat, passphrase typed by the harness only")
        self.phase1(nvars)
        self.boot_main(nvars)
        self.hibernate_phase(nvars)
        self.third_drive_boot(nvars)
        self.res["ok"] = not getattr(self, "failed", [])
        json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
        if getattr(self, "failed", []):
            print("FAILED STEPS:", ", ".join(self.failed))
            sys.exit(1)
        print("ALL PASS hogsmeade stripe")

    # ---------------------------------------------------------------- main boot (USB keyboard at the VGA prompt)
    def boot_main(self, nvars):
        a = self.z
        self.no_ps2 = True
        qt.pflash_vars(nvars)
        vm = qt.VM("2-boot1", self.qemu_args(None, True, nvars), a.out)
        try:
            ok, up = self.type_passphrase_usb(vm, "passphrase-prompt")
            self.step("boot1-vga-prompt-then-ONE-passphrase-ssh", up, "single prompt (LUKS swap + ZFS stripe), typed on the USB keyboard; no serial console")
            self.checks()
            self.hogsmeade_checks(vm)
            self.sh("sync", timeout=20)
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()

    def desktop_checks(self, vm=None):
        end = time.time() + qt.T(300)
        o = ""
        while time.time() < end:
            rc, o = self.sh(f"pgrep -x gdm >/dev/null && echo GDM; pgrep -u {USER} -f gnome-shell >/dev/null && echo SHELL-AS-USER; id {USER}; loginctl list-sessions --no-legend")
            if "GDM" in o and "SHELL-AS-USER" in o:
                break
            time.sleep(6)
        self.step("gnome-login-as-canutethegreat", "GDM" in o and "SHELL-AS-USER" in o, o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh(f"loginctl show-session $(loginctl list-sessions --no-legend | awk '$3==\"{USER}\"{{print $1; exit}}') -p Type -p Name -p Active 2>&1")
        self.step("session-is-wayland-and-active", "Type=wayland" in o and "Active=yes" in o, o.strip().replace("\n", " "))
        if vm:
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
            vm.shot("gnome-desktop")
            self.step("gnome-desktop-is-graphical-on-vga", colours > 40, f"{colours} distinct colours on the VGA console (see 2-boot1-gnome-desktop.png)")

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
        d0 = o.split("D1")[0]
        self.step("hibernate-swap-LUKS-on-first-drive-only", luks == 1 and "crypto_LUKS" in d0, f"LUKS partitions: {luks} (all on D0: {'crypto_LUKS' in d0})")
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
        rc, o = self.sh("ss -ltnH | awk '$4 ~ /:8188$/' | wc -l; rc-service comfyui status 2>&1 | head -1; ls -l /usr/lib/comfyui/venv/bin/python; /usr/lib/comfyui/venv/bin/python -c 'import torch,sys;print(\"torch\",torch.__version__)' 2>&1 | tail -1")
        self.step("comfyui-not-listening-without-tailscale-and-torch-imports", o.strip().splitlines()[0] == "0" and "torch " in o, o.strip().replace("\n", " | ")[:300])
        rc, o = self.sh("stat -c '%a %U' /etc/hogsmeade/ts-authkey /root/.ssh/authorized_keys 2>&1; ls /home/canutethegreat/.ssh/authorized_keys 2>&1; command -v hogsmeade-remote hogsmeade-verify hogsmeade-install prime-run")
        self.step("private-payload-and-tools-installed", "600 root" in o and "hogsmeade-verify" in o, o.strip().replace("\n", " | ")[:300])
        self.comfyui_cpu()
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
        if a.desktop:
            self.desktop_checks(vm)
            self.desktop_exec_check()
            self.session_audio_autostart_pre(USER)      # markers that must run when the session is restarted below
            self.session_after_password_change()
            # the session now running is the one the owner gets: user bus, PipeWire + volume key, tray, then Log Out (last: it ends the session)
            self.session_bus_checks(USER)
            self.session_audio_autostart_checks(USER, vm)
            self.tray_extension_checks(USER)
            self.session_logout_check(USER)

    def session_after_password_change(self):
        """hogshead lesson: after the owner's first-login password change the GNOME session must still start (home mounted + writable,
        locale present). Change the password the way passwd does, end the session, restart the display manager, wait for gnome-shell."""
        rc, o = self.sh(f"echo '{USER}:Granite-Meadow-Lantern-47!' | chpasswd && loginctl terminate-user {USER}; sleep 3; rc-service display-manager restart 2>&1 | tail -1; "
                        f"for i in $(seq 1 60); do pgrep -u {USER} -f gnome-shell >/dev/null && break; sleep 4; done; pgrep -u {USER} -f gnome-shell >/dev/null && echo SHELL-UP; "
                        f"loginctl list-sessions --no-legend | grep {USER}; touch /home/{USER}/.hsm-write-test && echo HOME-WRITABLE; ls -d /home/{USER}/.config /home/{USER}/.local 2>&1 | head -2", timeout=400)
        self.step("gnome-session-starts-after-first-login-password-change", "SHELL-UP" in o and "HOME-WRITABLE" in o, o.strip().replace("\n", " | ")[:300])

    def comfyui_cpu(self):
        rc, o = self.sh("sed -i '/^COMFYUI_LISTEN=/d;/^COMFYUI_EXTRA_ARGS=/d' /etc/conf.d/comfyui; echo COMFYUI_LISTEN=127.0.0.1 >> /etc/conf.d/comfyui; echo 'COMFYUI_EXTRA_ARGS=\"--cpu\"' >> /etc/conf.d/comfyui; "
                        "rc-service comfyui restart 2>&1 | tail -2; for i in $(seq 1 60); do curl -fsS -m 4 http://127.0.0.1:8188/system_stats && break; sleep 4; done; echo; "
                        "tail -5 /var/log/comfyui/comfyui.log /var/log/comfyui/comfyui.err 2>&1 | cut -c1-160", timeout=360)
        open(f"{self.z.out}/comfyui-cpu.txt", "w").write(o)
        self.step("comfyui-service-starts-in-cpu-mode", '"comfyui_version"' in o or '"system"' in o, o.strip().replace("\n", " | ")[:400])
        self.sh("rc-service comfyui stop; sed -i '/^COMFYUI_LISTEN=/d;/^COMFYUI_EXTRA_ARGS=/d' /etc/conf.d/comfyui", timeout=60)

    def provision_and_backup(self):
        """the post-install restic provisioning: crystalcave-style push over SSH stdin into root-only 0600 files, then a real hogsmeade-backup
        into a THROWAWAY local repo (never the real B2 repo)"""
        a = self.z
        script = os.path.join(HERE, "..", "provision-restic.sh")
        env = dict(os.environ, HSM_SSH_OPTS=f"-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -i {self.key} -p {qt.FWD_SSH}")
        r = subprocess.run(["bash", script, "--test", "--", "root@127.0.0.1"], env=env, capture_output=True, text=True, timeout=180)
        open(f"{a.out}/provision.txt", "w").write(r.stdout + r.stderr)
        self.step("restic-provisioned-over-ssh-stdin-0600", r.returncode == 0 and "600 root /etc/hogsmeade/backup.env" in r.stdout and "600 root /etc/hogsmeade/restic-password" in r.stdout, r.stdout.strip().replace("\n", " | ")[:300] + r.stderr[:100])
        self.sh("mkdir -p /data/hsmtest && head -c 300000 /dev/urandom > /data/hsmtest/blob && echo hello > /home/canutethegreat/hsm-note.txt && chown -R canutethegreat /home/canutethegreat/hsm-note.txt", timeout=60)
        rc, o = self.sh("/usr/sbin/hogsmeade-backup; echo BACKUP-RC=$?; set -a; . /etc/hogsmeade/backup.env; set +a; restic snapshots --tag hogsmeade --compact; restic ls latest | grep -c hsm-note; restic check 2>&1 | tail -2; cat /var/lib/hogsmeade/last-backup-ok", timeout=600)
        open(f"{a.out}/test-backup.txt", "w").write(o)
        self.step("test-backup-to-throwaway-restic-repo", "BACKUP-RC=0" in o and "1 snapshots" in o and "no errors were found" in o, o.strip().replace("\n", " | ")[:400])

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
