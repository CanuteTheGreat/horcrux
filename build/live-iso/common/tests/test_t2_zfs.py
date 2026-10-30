"""Apple T2 support, ZFS-native-encryption install and boot environments (pure logic, no disks, no zfs binary)."""
import io
import os
import sys
import tarfile
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import be as BE  # noqa: E402
from liveinst import desktop as D  # noqa: E402
from liveinst import system as S  # noqa: E402
from liveinst import t2 as T2  # noqa: E402
from liveinst import zfsroot as Z  # noqa: E402

PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs"], "swap_mb": 4096, "network_mode": "management",
           "services": ["docker"], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}


def fake_pci(root, devs):
    for i, (v, d) in enumerate(devs):
        p = os.path.join(root, f"0000:00:{i:02x}.0")
        os.makedirs(p)
        open(os.path.join(p, "vendor"), "w").write(v + "\n")
        open(os.path.join(p, "device"), "w").write(d + "\n")


class Detect(unittest.TestCase):
    def test_t2_present(self):
        with tempfile.TemporaryDirectory() as d:
            fake_pci(d, [("0x8086", "0x8a14"), ("0x106b", "0x2005"), ("0x106b", "0x1801")])
            self.assertTrue(T2.detect(d))

    def test_t2_absent_even_with_apple_nvme(self):
        with tempfile.TemporaryDirectory() as d:
            fake_pci(d, [("0x106b", "0x2005"), ("0x8086", "0x8a14")])
            self.assertFalse(T2.detect(d))

    def test_empty(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertFalse(T2.detect(d))


class InitramfsConfig(unittest.TestCase):
    def test_dracut_conf_has_apple_bce_and_keyboard_drivers(self):
        c = T2.dracut_conf()
        for m in ("t2bce_core", "t2bce_dma", "t2bce_vhci", "hid_apple", "hid_appletb_kbd", "usbhid", "nvme"):
            self.assertIn(m, c)
        self.assertIn("force_drivers", c)
        # the early (force_drivers) list must contain the whole bce stack in dependency-safe order
        early = [l for l in c.splitlines() if l.startswith("force_drivers")][0]
        self.assertLess(early.index("t2bce_core"), early.index("t2bce_vhci"))

    def test_cmdline_pm_async_off(self):
        self.assertIn("pm_async=off", T2.grub_cmdline_extra().split())

    def test_initramfs_check(self):
        good = ["usr/lib/modules/7.2.13-t2/kernel/drivers/staging/apple-bce/t2bce_core.ko.zst",
                "usr/lib/modules/7.2.13-t2/kernel/drivers/staging/apple-bce/t2bce_vhci.ko.zst",
                "usr/lib/modules/7.2.13-t2/kernel/drivers/staging/apple-bce/t2bce_dma.ko.zst",
                "usr/lib/modules/7.2.13-t2/kernel/drivers/hid/hid-apple.ko.zst"]
        self.assertEqual(T2.initramfs_check("\n".join(good)), [])                     # real names: hid-apple.ko, t2bce_core.ko
        self.assertEqual(T2.initramfs_check("\n".join(good).replace("hid-apple", "hid_apple")), [])
        self.assertEqual(T2.initramfs_check(good[:2]), ["t2bce_dma", "hid_apple"])
        self.assertIn("t2bce_core", T2.initramfs_check("nothing"))


def make_tar(path, members):
    with tarfile.open(path, "w") as t:
        for name, data in members:
            ti = tarfile.TarInfo(name)
            ti.size = len(data)
            t.addfile(ti, io.BytesIO(data))


class Preflight(unittest.TestCase):
    def test_image_without_t2_kernel_refuses_before_touching_the_disk(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "lib/modules/6.18.57-gentoo-dist-bin"))
            self.assertFalse(T2.image_has_t2_kernel(d))
            os.makedirs(os.path.join(d, "lib/modules/7.2.13-t2-horcrux"))
            self.assertTrue(T2.image_has_t2_kernel(d))

    def test_bundled_kernel_tarball_counts(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "usr/share/horcrux/t2"))
            open(os.path.join(d, "usr/share/horcrux/t2/kernel-7.2.13-t2.tar.zst"), "w").close()
            self.assertTrue(T2.image_has_t2_kernel(d))

    def test_target_kernel_version_picks_t2(self):
        with tempfile.TemporaryDirectory() as d:
            for v in ("6.18.57-gentoo-dist-bin", "7.2.13-t2-horcrux"):
                os.makedirs(os.path.join(d, "lib/modules", v))
            self.assertEqual(T2.target_kernel_version(d), "7.2.13-t2-horcrux")


class Firmware(unittest.TestCase):
    def test_import_from_flat_tar_and_filters_the_rest(self):
        with tempfile.TemporaryDirectory() as d:
            tp = os.path.join(d, "firmware.tar")
            make_tar(tp, [("brcmfmac4364b3-pcie.apple,borneo.bin", b"x" * 10), ("brcmbt4377b3-apple,formosa.ptb", b"y"),
                          ("brcmfmac4364b2-pcie.apple,ekans-HRPN-m-5.1.txt", b"z"),
                          ("../evil.bin", b"no"), ("/etc/passwd", b"no"), ("README", b"no"), ("sub/dir/brcmfmac-x.bin", b"ok")])
            tgt = os.path.join(d, "root")
            done = T2.import_firmware(tp, tgt)
            self.assertIn("brcmfmac4364b3-pcie.apple,borneo.bin", done)
            self.assertIn("brcmbt4377b3-apple,formosa.ptb", done)
            self.assertNotIn("evil.bin", done)
            self.assertNotIn("passwd", done)
            self.assertNotIn("README", done)
            self.assertFalse(os.path.exists(os.path.join(d, "evil.bin")))
            self.assertEqual(sorted(os.listdir(os.path.join(tgt, "lib/firmware/brcm"))), sorted(done))

    def test_symlinks_and_oversize_are_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            tp = os.path.join(d, "f.tar")
            with tarfile.open(tp, "w") as t:
                ti = tarfile.TarInfo("brcmfmac-link.bin")
                ti.type = tarfile.SYMTYPE
                ti.linkname = "/etc/shadow"
                t.addfile(ti)
                big = tarfile.TarInfo("brcmfmac-big.bin")
                big.size = T2.FW_MAX_FILE + 1
                t.addfile(big, io.BytesIO(b"\0" * big.size))
            self.assertEqual(T2.import_firmware(tp, os.path.join(d, "r")), [])

    def test_find_sources_dir_and_tar(self):
        with tempfile.TemporaryDirectory() as d:
            old = os.path.join(d, "old-os/lib/firmware/brcm")
            os.makedirs(old)
            open(os.path.join(old, "brcmfmac4364b3-pcie.apple,borneo.bin"), "w").write("fw")
            open(os.path.join(old, "brcmfmac43455-sdio.bin"), "w").write("generic")
            home = os.path.join(d, "old-os/home/u/macbookprointel")
            os.makedirs(home)
            make_tar(os.path.join(home, "firmware.tar"), [("brcmbt4378b1-apple,capri-m.bin", b"1")])
            found = T2.find_firmware_sources([d])
            kinds = sorted(f["kind"] for f in found)
            self.assertEqual(kinds, ["dir", "tar"])
            self.assertTrue(all(f["apple"] >= 1 for f in found))
            tgt = os.path.join(d, "t")
            self.assertEqual(T2.import_firmware(old, tgt), ["brcmfmac4364b3-pcie.apple,borneo.bin"])  # generic non-apple file not taken

    def test_no_apple_firmware_in_a_tree_means_clean_iso(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "lib/firmware/brcm"))
            open(os.path.join(d, "lib/firmware/brcm/brcmfmac43455-sdio.bin"), "w").write("x")
            self.assertFalse(T2.iso_ships_apple_firmware(d))
            open(os.path.join(d, "lib/firmware/brcm/brcmfmac4364b3-pcie.apple,borneo.bin"), "w").write("x")
            self.assertTrue(T2.iso_ships_apple_firmware(d))


class ZfsEncryption(unittest.TestCase):
    def test_pool_args_encrypted(self):
        a = Z.pool_args(True, ["/dev/nvme0n1p4"], False, "/mnt/target")
        j = " ".join(a)
        for want in ("ashift=12", "autotrim=on", "compression=zstd", "encryption=aes-256-gcm", "keyformat=passphrase",
                     "-R /mnt/target rpool /dev/nvme0n1p4"):
            self.assertIn(want, j)
        self.assertNotIn("keylocation=prompt", j)       # file:// during creation, set to prompt afterwards
        self.assertNotIn("passphrase=", j)

    def test_pool_args_plain_mirror(self):
        a = Z.pool_args(False, ["/dev/vda4", "/dev/vdb4"], True, "/t")
        self.assertNotIn("encryption=aes-256-gcm", " ".join(a))
        self.assertEqual(a[-3:], ["mirror", "/dev/vda4", "/dev/vdb4"])

    def test_datasets(self):
        names = [n for n, _ in Z.datasets()]
        for n in ("ROOT/horcrux", "home", "var/log", "var/lib/libvirt"):
            self.assertIn(n, names)
        props = dict(Z.datasets())
        self.assertIn("recordsize=64K", props["var/lib/libvirt"])
        self.assertIn("mountpoint=/", props["ROOT/horcrux"])
        self.assertIn("canmount=noauto", props["ROOT/horcrux"])
        self.assertNotIn("var/lib/docker", names)
        self.assertIn("var/lib/docker", [n for n, _ in Z.datasets(True)])

    def test_passphrase_rules(self):
        self.assertIsNotNone(Z.passphrase_problem("short"))
        self.assertIsNotNone(Z.passphrase_problem("aaaaaaaaaaaaaaaa"))
        self.assertIsNone(Z.passphrase_problem("correct horse battery staple"))

    def test_config_validation(self):
        c = S.default_config(PROFILE)
        c.update(fs="zfs", disks=["/dev/vda"], root_password="Kx9!mQv2-tle-77Zp", zfs_encrypt=True, zfs_passphrase="short")
        errs = S.validate_config(PROFILE, c)
        self.assertTrue(any("disk passphrase" in e for e in errs))
        c["zfs_passphrase"] = "correct horse battery staple"
        self.assertEqual([e for e in S.validate_config(PROFILE, c) if "passphrase" in e], [])
        c["fs"] = "ext4"
        self.assertTrue(any("ZFS" in e for e in S.validate_config(PROFILE, c)))

    def test_answer_file_keys(self):
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[global]\nhostname="a.b"\nt2="no"\ninstall_type="desktop"\nroot_password_hash="$6$s$' + "a" * 86 + '"\n'
                    '[disk]\nfilesystem="zfs"\nencryption=true\npassphrase="correct horse battery staple"\n'
                    '[user]\nname="owner"\npassword_hash="$6$s$' + "b" * 86 + '"\n')
        c = S.config_from_answers(PROFILE, f.name)
        os.unlink(f.name)
        self.assertTrue(c["zfs_encrypt"])
        self.assertEqual((c["t2"], c["install_type"], c["user_name"]), ("no", "desktop", "owner"))
        self.assertEqual([e for e in S.validate_config(PROFILE, dict(c, disks=["/dev/vda"])) if "user" in e], [])


class BootEnvironments(unittest.TestCase):
    def test_grub_cfg_default_is_active_be_and_root_is_zfs_dataset(self):
        t = BE.render_grub(["horcrux", "rollback-1"], "rollback-1", "rpool", "1111-2222", "pm_async=off", 5,
                           "serial --unit=0 --speed=115200", kernels={"horcrux": "7.2.13-t2", "rollback-1": "7.2.12-t2"})
        self.assertIn('set default="be-rollback-1"', t)
        self.assertIn("root=ZFS=rpool/ROOT/rollback-1", t)
        self.assertIn("root=ZFS=rpool/ROOT/horcrux", t)
        self.assertIn("search --no-floppy --fs-uuid --set=root 1111-2222", t)
        self.assertIn("linux /be/horcrux/vmlinuz", t)
        self.assertIn("pm_async=off", t)
        self.assertIn("terminal_output console serial", t)
        self.assertLess(t.index("rollback-1 (default)"), t.index("Horcrux - horcrux"))

    def test_names(self):
        self.assertTrue(BE.valid_name("pre-upgrade-2026"))
        for bad in ("", "a@b", "../x", "a b", "-x"):
            self.assertFalse(BE.valid_name(bad))

    def test_kernel_selection_prefers_matching_glob(self):
        with tempfile.TemporaryDirectory() as d:
            for k in ("6.12.30-gentoo-dist", "7.2.13-t2", "7.2.9-t2"):
                open(os.path.join(d, f"vmlinuz-{k}"), "w").write("k")
                open(os.path.join(d, f"initramfs-{k}.img"), "w").write("i")
            open(os.path.join(d, "vmlinuz-7.9.0-orphan"), "w").write("k")      # no initramfs: ignored
            self.assertEqual(BE.kernel_files(d, "*t2*")[0], "7.2.13-t2")
            self.assertEqual(BE.kernel_files(d)[0], "7.2.13-t2")
            self.assertEqual(BE.kernel_files(d, "*gentoo*")[0], "6.12.30-gentoo-dist")

    def test_conf(self):
        with tempfile.NamedTemporaryFile("w", delete=False) as f:
            f.write('POOL="tank"\nKEEP="3"\nKERNEL_GLOB="*t2*"\n')
        c = BE.conf(f.name)
        os.unlink(f.name)
        self.assertEqual((c["POOL"], c["KEEP"], c["KERNEL_GLOB"], c["AUTO_SNAPSHOT"]), ("tank", "3", "*t2*", "yes"))


class Desktop(unittest.TestCase):
    def test_services_list(self):
        self.assertEqual(D.parse_services("# x\ndbus default\nelogind boot  # c\n\nNetworkManager\n"),
                         [("dbus", "default"), ("elogind", "boot"), ("NetworkManager", "default")])

    def test_user(self):
        self.assertIsNone(D.user_problem("owner"))
        for bad in ("", "Root", "root", "a b", "9x"):
            self.assertIsNotNone(D.user_problem(bad))


if __name__ == "__main__":
    unittest.main()


class ZfsHibernateSwap(unittest.TestCase):
    """encrypted-ZFS laptop: LUKS2 swap partition, ONE passphrase, resume BEFORE the pool import (zfsswap.py + dracut module)"""

    def test_plan_zfs_portable(self):
        from liveinst import portableswap as PS
        cfg = {"fs": "zfs", "zfs_encrypt": True, "install_type": "desktop"}
        p = PS.plan(cfg, 16000)
        self.assertEqual((p["mode"], p["mb"]), ("hibernate", 18432))         # 16 GB RAM -> 18 GiB
        self.assertEqual(PS.plan(dict(cfg, install_type="server"), 16000, detect=lambda: (False, "server"))["mode"], "none")
        self.assertEqual(PS.plan(dict(cfg, zfs_encrypt=False), 16000)["mode"], "none")   # never an unencrypted RAM image
        self.assertEqual(PS.plan(dict(cfg, swap="none"), 16000)["mode"], "none")

    def test_pure_helpers(self):
        from liveinst import zfsswap as ZS
        self.assertEqual(ZS.sgdisk_part(18432), ["-n", "3:0:+18432M", "-t", "3:8309", "-c", "3:cryptswap"])
        self.assertEqual(ZS.cmdline("LU", "SW"), "resume=UUID=SW rd.hx.swap=LU")
        self.assertIn("pri=10", ZS.fstab_line())
        self.assertIn("/dev/mapper/hxswap", ZS.fstab_line())
        self.assertIn("horcrux-unlock", ZS.dracut_conf())
        self.assertIn("resume", ZS.dracut_conf())
        cmd = ZS.luks_format_cmd("/dev/vda3", 16000)
        self.assertIn("luks2", cmd)
        self.assertIn("--key-file=-", cmd)                  # passphrase on stdin, never argv
        self.assertTrue(int(cmd[cmd.index("--pbkdf-memory") + 1]) <= 512 * 1024)

    def test_initramfs_listing_checks(self):
        from liveinst import zfsswap as ZS
        good = ("usr/lib/dracut/hooks/cmdline/40-parse-hx.sh\nusr/bin/hx-unlock\nusr/lib/dracut/hooks/pre-mount/50-hx-zfskey.sh\n"
                "usr/lib/dracut/hooks/pre-mount/90-zfs-load-key.sh\nusr/lib/dracut/hooks/mount/98-mount-zfs.sh\nusr/lib/dracut/resume.sh\n"
                "usr/bin/cryptsetup\nlib/modules/7.2.13/extra/zfs.ko\nlib/modules/7.2.13/kernel/drivers/md/dm-crypt.ko\n")
        self.assertEqual(ZS.initramfs_problems(good), [])
        self.assertTrue(ZS.hook_order_ok(good))
        self.assertIn("hx-unlock", ZS.initramfs_problems(good.replace("usr/bin/hx-unlock\n", "")))
        # the import must NOT come before the resume attempt: a zfs hook below pre-mount 50 would be a corruption risk
        bad = good.replace("mount/98-mount-zfs.sh", "mount/40-mount-zfs.sh")
        self.assertFalse(ZS.hook_order_ok(bad))

    def test_dracut_module_files(self):
        base = os.path.join(HERE, "..", "overlay", "usr", "lib", "dracut", "modules.d", "91horcrux-unlock")
        names = sorted(os.listdir(base))
        for n in ("module-setup.sh", "parse-hx.sh", "hx-unlock.sh", "hx-try.sh", "hx-zfskey.sh", "hx-cleanup.sh"):
            self.assertIn(n, names)
            self.assertTrue(os.access(os.path.join(base, n), os.X_OK), n)
        key = open(os.path.join(base, "hx-zfskey.sh")).read()
        self.assertIn("zfs load-key -L file:///run/hx/key", key)
        self.assertLess(key.index("zpool import"), key.index("load-key"))
        self.assertIn("shred -u /run/hx/key", key)            # the key never survives into the real system
        setup = open(os.path.join(base, "module-setup.sh")).read()
        self.assertIn("inst_hook pre-mount 50", setup)        # after 74resume's pre-mount 10, before 90zfs's mount 98
        self.assertNotIn("passphrase", open(os.path.join(base, "hx-try.sh")).read().split("read -r pp")[0].replace("# stdin = the passphrase", ""))

    def test_zfsroot_declares_support(self):
        self.assertTrue(Z.PORTABLE_SWAP)


class WifiToIwd(unittest.TestCase):
    """NetworkManager's iwd backend only autoconnects networks that have a /var/lib/iwd file (hogshead first boot)."""

    NM = ("[connection]\nid=Hogsmead Station\ntype=wifi\ninterface-name=wlp229s0\nautoconnect=true\n\n"
          "[wifi]\nssid=Hogsmead Station\nmode=infrastructure\n\n[wifi-security]\nkey-mgmt=wpa-psk\npsk=correct horse battery\n")
    EAP = "[connection]\nid=corp\ntype=wifi\n\n[wifi]\nssid=Corp\n\n[wifi-security]\nkey-mgmt=wpa-eap\n"

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.nm = os.path.join(self.root, "etc/NetworkManager/system-connections")
        os.makedirs(self.nm)
        for fn, txt in (("Hogsmead Station.nmconnection", self.NM), ("Corp.nmconnection", self.EAP), ("wired.nmconnection", "[connection]\nid=w\ntype=ethernet\n")):
            open(os.path.join(self.nm, fn), "w").write(txt)

    def tearDown(self):
        self.tmp.cleanup()

    def test_parse_skips_enterprise_and_wired(self):
        self.assertEqual(S.nm_wifi_networks(self.nm), [("Hogsmead Station", "correct horse battery")])

    def test_psk_file_written_0600_and_pin_dropped(self):
        out = S.carry_wifi_to_iwd(self.root, sources=(self.nm,))
        path = os.path.join(self.root, "var/lib/iwd/Hogsmead Station.psk")
        self.assertEqual(out, [path])
        self.assertEqual(open(path).read(), "[Security]\nPassphrase=correct horse battery\n")
        self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
        self.assertNotIn("interface-name", open(os.path.join(self.nm, "Hogsmead Station.nmconnection")).read())

    def test_existing_iwd_file_is_kept(self):
        os.makedirs(os.path.join(self.root, "var/lib/iwd"))
        p = os.path.join(self.root, "var/lib/iwd/Hogsmead Station.psk")
        open(p, "w").write("[Security]\nPassphrase=other\n")
        self.assertEqual(S.carry_wifi_to_iwd(self.root, sources=(self.nm,)), [])
        self.assertIn("other", open(p).read())

    def test_missing_source_is_fine(self):
        self.assertEqual(S.carry_wifi_to_iwd(self.root, sources=("/nonexistent/x",)), [])


if __name__ == "__main__":
    unittest.main()
