"""Unit tests for the shared guided installer (pure logic; no disks touched).
Run: python3 -m unittest discover -s build/live-iso/common/tests   (Python >= 3.11)
"""
import os
import sys
import tempfile
import textwrap
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import system as S  # noqa: E402
from liveinst import consolemenu as CM  # noqa: E402

PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs"], "swap_mb": 0, "network_mode": "management",
           "services": [], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}
FW = dict(PROFILE, name="patronus", title="Patronus", filesystems=["ext4"], network_mode="wan-lan")
GOOD_PW = "Kx9!mQv2-tle-77Zp"
HASH = "$6$salt$" + "a" * 86


class Passwords(unittest.TestCase):
    def test_accepts_strong(self):
        self.assertIsNone(S.password_problem(GOOD_PW, "node1.example.com"))

    def test_rejects_short_and_defaults(self):
        self.assertIn("at least 12", S.password_problem("Short1!"))
        self.assertIn("guessable", S.password_problem("horcrux-horcrux1A"))
        self.assertIn("guessable", S.password_problem("Patronus-2026!!"))
        self.assertIsNotNone(S.password_problem("aaaaaaaaaaaaaaaa"))
        self.assertIsNotNone(S.password_problem("alllowercaseletters"))
        self.assertIsNotNone(S.password_problem("abcdef123456XYZ!"))   # alphabet run

    def test_hostname_in_password(self):
        self.assertIn("host name", S.password_problem("Qm7!gulliver-77Zz", "gulliver.example.com"))

    def test_hash_never_plaintext(self):
        self.assertIsNone(S.crypt_hash_problem(HASH))
        self.assertIsNotNone(S.crypt_hash_problem("hunter2hunter2"))
        self.assertIsNotNone(S.crypt_hash_problem(""))


class Network(unittest.TestCase):
    def test_cidr(self):
        self.assertIsNone(S.cidr_problem("192.168.1.10/24"))
        self.assertIsNotNone(S.cidr_problem("192.168.1.10"))
        self.assertIsNotNone(S.cidr_problem("192.168.1.0/24"))
        self.assertIsNotNone(S.cidr_problem("10.0.0.1/31"))

    def test_gateway_inside(self):
        self.assertIsNone(S.ip_problem("10.0.2.2", "10.0.2.77/24"))
        self.assertIsNotNone(S.ip_problem("10.0.3.2", "10.0.2.77/24"))

    def test_hostname(self):
        self.assertIsNone(S.hostname_problem("node1.example.com"))
        self.assertIsNotNone(S.hostname_problem("Bad_Name"))
        self.assertIsNotNone(S.hostname_problem(""))

    def test_part_path(self):
        self.assertEqual(S.part_path("/dev/vda", 4), "/dev/vda4")
        self.assertEqual(S.part_path("/dev/nvme0n1", 4), "/dev/nvme0n1p4")
        self.assertEqual(S.part_path("/dev/mmcblk0", 2), "/dev/mmcblk0p2")


class Answers(unittest.TestCase):
    def cfg(self, text, profile=PROFILE):
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write(textwrap.dedent(text))
        try:
            return S.config_from_answers(profile, f.name)
        finally:
            os.unlink(f.name)

    def test_needs_secret(self):
        c = self.cfg("""
            [global]
            hostname = "a.example.com"
            [disk]
            disks = ["/dev/vda"]
        """)
        errs = S.validate_config(PROFILE, c)
        self.assertTrue(any("unreachable" in e for e in errs), errs)

    def test_plaintext_password_refused(self):
        c = self.cfg("""
            [global]
            hostname = "a.example.com"
            root_password_hash = "plainpassword"
            [disk]
            disks = ["/dev/vda"]
        """)
        self.assertTrue(any("never a plain password" in e for e in S.validate_config(PROFILE, c)))

    def test_hash_ok_and_defaults_secure(self):
        c = self.cfg(f"""
            [global]
            hostname = "a.example.com"
            root_password_hash = "{HASH}"
            [disk]
            disks = ["/dev/vda"]
        """)
        self.assertEqual(S.validate_config(PROFILE, c), [])
        self.assertFalse(c["ssh_password_auth"], "password SSH must default to OFF")

    def test_zfs_mirror_needs_two_disks(self):
        c = self.cfg(f"""
            [global]
            hostname = "a.example.com"
            root_password_hash = "{HASH}"
            [disk]
            filesystem = "zfs"
            zfs_raid = "mirror"
            disks = ["/dev/vda"]
        """)
        self.assertTrue(any("two different disks" in e for e in S.validate_config(PROFILE, c)))

    def test_patronus_wan_lan_sections(self):
        c = self.cfg(f"""
            [global]
            hostname = "fw.example.com"
            root_password_hash = "{HASH}"
            [disk]
            disks = ["/dev/vda"]
            [network.wan]
            mode = "static"
            address = "203.0.113.2/29"
            gateway = "203.0.113.1"
            dns = ["9.9.9.9"]
            [network.lan]
            address = "192.168.77.1/24"
        """, FW)
        self.assertEqual(S.validate_config(FW, c), [])
        self.assertEqual(c["lan"]["address"], "192.168.77.1/24")
        self.assertEqual(c["wan"]["dns"], "9.9.9.9")

    def test_filesystem_not_offered(self):
        c = self.cfg(f"""
            [global]
            hostname = "fw.example.com"
            root_password_hash = "{HASH}"
            [disk]
            filesystem = "zfs"
            disks = ["/dev/vda"]
        """, FW)
        self.assertTrue(any("not offered" in e for e in S.validate_config(FW, c)))


class ConsoleMenuLogic(unittest.TestCase):
    def test_dhcpcd_static_replaces_installer_stanza(self):
        txt = "hostname\n\ninterface eth0\nstatic ip_address=10.0.0.5/24\nstatic routers=10.0.0.1\n\ninterface eth1\nstatic ip_address=1.1.1.1/24\n"
        out = CM.apply_iface_config(txt, "eth0", "static", "192.168.9.2/24", "192.168.9.1", "9.9.9.9")
        self.assertNotIn("10.0.0.5", out)
        self.assertIn("static ip_address=1.1.1.1/24", out)           # other NIC untouched
        self.assertIn("static ip_address=192.168.9.2/24", out)
        again = CM.apply_iface_config(out, "eth0", "dhcp")           # idempotent switch back to DHCP
        self.assertNotIn("192.168.9.2", again)
        self.assertEqual(again.count("BEGIN console-menu eth0"), 1)

    def test_roles(self):
        t = CM.apply_roles("[general]\nx = 1\n[network]\nlan_interface = \"old\"\n", "eth0", "eth1")
        self.assertEqual(CM.read_roles(t), ("eth0", "eth1"))
        self.assertEqual(t.count("[network]"), 1)
        t2 = CM.apply_roles("", "eth9", "")
        self.assertEqual(CM.read_roles(t2), ("eth9", ""))

    def test_host_validation(self):
        for good in ("example.com", "10.0.0.1", "fe80::1", "a"):
            self.assertTrue(CM.host_ok(good), good)
        for bad in ("", "-oProxy", "a b", "x;reboot", "$(id)", "a" * 300):
            self.assertFalse(CM.host_ok(bad), bad)

    def test_backup_member_paths(self):
        roots = ["/etc/horcrux", "/var/lib/horcrux/horcrux.db"]
        self.assertTrue(CM.member_allowed("etc/horcrux/ssl/a.pem", roots))
        self.assertTrue(CM.member_allowed("var/lib/horcrux/horcrux.db", roots))
        self.assertTrue(CM.member_allowed("MANIFEST.json", roots))
        for bad in ("etc/passwd", "etc/horcrux/../shadow", "../etc/shadow", "etc/horcruxevil/x", "root/.ssh/authorized_keys"):
            self.assertFalse(CM.member_allowed(bad, roots), bad)


class ConsoleMenuFlows(unittest.TestCase):
    """Drive the real Console class with scripted input against a temp sqlite DB."""
    def setUp(self):
        import sqlite3
        self.d = tempfile.mkdtemp()
        self.db = os.path.join(self.d, "x.db")
        con = sqlite3.connect(self.db)
        con.execute("create table users (username text, password_hash text)")
        con.execute("insert into users values ('admin','OLD')")
        con.execute("create table sessions (id text)")
        con.execute("insert into sessions values ('s1')")
        con.commit()
        con.close()
        self.prof = dict(PROFILE, db_path=self.db, db_table="users", web_service="", config_paths=[self.d + "/etc", self.db],
                         admin_password_file=self.d + "/first-pw", admin_password_owner="root")
        self.orig = CM.argon2_encode
        CM.argon2_encode = lambda pw, run=None: "$argon2id$stub$" + pw

    def tearDown(self):
        CM.argon2_encode = self.orig

    def console(self, answers):
        it = iter(answers)
        import io
        c = CM.Console(self.prof, inp=lambda prompt="": next(it), out=io.StringIO())
        c.secret = lambda prompt: next(it)
        return c

    def test_reset_password_updates_db_and_drops_sessions(self):
        import sqlite3
        c = self.console([GOOD_PW, GOOD_PW, "n"])
        c.reset_password()
        con = sqlite3.connect(self.db)
        self.assertEqual(con.execute("select password_hash from users").fetchone()[0], "$argon2id$stub$" + GOOD_PW)
        self.assertEqual(con.execute("select count(*) from sessions").fetchone()[0], 0)

    def test_weak_password_rejected_three_times_changes_nothing(self):
        import sqlite3
        c = self.console(["short", "short", "short"])
        c.reset_password()
        self.assertEqual(sqlite3.connect(self.db).execute("select password_hash from users").fetchone()[0], "OLD")

    def test_backup_restore_roundtrip_and_refusals(self):
        import sqlite3
        os.makedirs(self.d + "/etc")
        open(self.d + "/etc/a.conf", "w").write("one")
        c = self.console([])
        c.backup_dir = self.d + "/bk"
        b = c.make_backup("t")
        ok, msg = c.check_backup(b)
        self.assertTrue(ok, msg)
        # an archive for another product, and one with a path escape, are refused
        import json as _j, tarfile as _t, io as _io
        for man, member, why in (({"project": "patronus"}, "etc/x", "not horcrux"), ({"project": "horcrux"}, "etc/passwd", "outside")):
            fn = self.d + "/evil.tar.gz"
            with _t.open(fn, "w:gz") as tf:
                for name, data in (("MANIFEST.json", _j.dumps(man).encode()), (member, b"x")):
                    ti = _t.TarInfo(name)
                    ti.size = len(data)
                    tf.addfile(ti, _io.BytesIO(data))
            ok, msg = c.check_backup(fn)
            self.assertFalse(ok, why)


class ProfileDefaults(unittest.TestCase):
    def test_console_open_from_answers(self):
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[global]\nconsole_menu_open = true\n')
        self.assertTrue(S.config_from_answers(PROFILE, f.name)["console_open"])


if __name__ == "__main__":
    unittest.main()


# ---------------------------------------------------------------- hardware profiles + XFS-on-LUKS
from liveinst import hwprofile as HW  # noqa: E402
from liveinst import luksroot as LR  # noqa: E402

SURFACE_TOML = os.path.join(HERE, "..", "..", "..", "..", "gentoo", "surface", "hwprofile.toml")


class HwProfile(unittest.TestCase):
    def profiles(self):
        import tomllib
        return [tomllib.load(open(SURFACE_TOML, "rb"))]

    def test_surface_pro_1_names(self):
        for name in ("Surface with Windows 8 Pro", "Surface Pro", "surface pro", "Surface  Pro 2"):
            p, m = HW.detect({"sys_vendor": "Microsoft Corporation", "product_name": name}, self.profiles())
            self.assertEqual((p["id"], m["id"]), ("surface", "surface-pro-1"), name)
            self.assertIn("button.lid_init_state=open", HW.kernel_cmdline(p, m))

    def test_other_surface_generic_and_non_surface_none(self):
        p, m = HW.detect({"sys_vendor": "Microsoft Corporation", "product_name": "Surface Laptop 4"}, self.profiles())
        self.assertEqual((p["id"], m), ("surface", None))
        self.assertEqual(HW.kernel_cmdline(p, m), [])
        self.assertEqual(HW.detect({"sys_vendor": "Microsoft Corporation", "product_name": "Virtual Machine"}, self.profiles()), (None, None))
        self.assertEqual(HW.detect({"sys_vendor": "Apple Inc.", "product_name": "Surface Pro"}, self.profiles()), (None, None))
        self.assertEqual(HW.detect({"sys_vendor": "QEMU", "product_name": "Standard PC (Q35 + ICH9, 2009)"}, self.profiles()), (None, None))

    def test_default_fs(self):
        fs = ["ext4", "xfs", "zfs", "xfs-luks"]
        self.assertEqual(HW.default_fs(fs, 3800), "xfs-luks")          # 4 GB machine
        self.assertEqual(HW.default_fs(fs, 5800), "xfs-luks")          # 6 GB
        self.assertEqual(HW.default_fs(fs, 7700), "zfs")               # "8 GB" machine: MemTotal ~7.7 GB
        self.assertEqual(HW.default_fs(fs, 64000), "zfs")
        self.assertEqual(HW.default_fs(["ext4"], 3800), "ext4")        # Patronus
        p = self.profiles()[0]
        self.assertEqual(HW.default_fs(fs, 64000, p), "xfs-luks")      # a profile can force it
        self.assertEqual(HW.default_fs(["ext4", "zfs"], 3800), "zfs")  # image without xfs-luks keeps old behaviour


class LuksRoot(unittest.TestCase):
    def test_argon_memory_capped(self):
        self.assertEqual(LR.pbkdf_memory_kib(3800) // 1024, 712)
        self.assertEqual(LR.pbkdf_memory_kib(4096) // 1024, 768)
        self.assertEqual(LR.pbkdf_memory_kib(8192), 1024 * 1024)
        self.assertEqual(LR.pbkdf_memory_kib(65536), 1024 * 1024)      # cap 1 GiB
        self.assertGreaterEqual(LR.pbkdf_memory_kib(100), 64 * 1024)
        self.assertLessEqual(LR.pbkdf_memory_kib(4096) * 1024, 4096 * 1024 * 1024 // 4)   # never more than a quarter of RAM

    def test_format_cmd(self):
        c = LR.luks_format_cmd("/dev/sda3", 4096)
        self.assertIn("luks2", c)
        self.assertEqual(c[c.index("--pbkdf") + 1], "argon2id")
        self.assertEqual(c[c.index("--pbkdf-memory") + 1], str(768 * 1024))
        self.assertIn("--key-file=-", c)                                  # passphrase on stdin, never in argv
        self.assertNotIn("discard", " ".join(LR.luks_format_cmd("/dev/sda3", 4096)))
        self.assertIn("--allow-discards", LR.luks_open_cmd("/dev/sda3"))

    def test_sgdisk_layout(self):
        a = " ".join(LR.sgdisk_args("/dev/sda", True))
        self.assertIn("1:0:+1G", a)
        self.assertIn("1:ef00", a)
        self.assertIn("3:0:0", a)
        self.assertIn("3:8309", a)                                        # Linux LUKS
        self.assertNotIn("8200", a)                                       # no swap partition: zram only
        self.assertIn("1:ef02", " ".join(LR.sgdisk_args("/dev/sda", False)))

    def test_kernel_args_and_fstab(self):
        k = LR.kernel_args("1234-abcd")
        for part in ("rd.luks.uuid=1234-abcd", "rd.luks.name=1234-abcd=hxroot", "rd.luks.options=1234-abcd=discard", "root=/dev/mapper/hxroot"):
            self.assertIn(part, k)
        f = LR.fstab_text("UUID=r", "b", "e")
        self.assertIn("/\txfs\tdefaults", f)
        self.assertNotIn("discard", f)                                    # XFS mount options: defaults only
        self.assertNotIn("swap", f)

    def test_dracut_has_keyboard_stack(self):
        d = LR.dracut_conf()
        for m in ("crypt", "usbhid", "hid_multitouch", "hid_microsoft", "xhci_pci", "i915", "dm_crypt"):
            self.assertIn(m, d)

    def test_verify_initramfs(self):
        ok = "cryptsetup\nusbhid.ko\nhid-multitouch.ko\nhid-microsoft.ko\nxhci-pci.ko\ndm-crypt.ko"
        self.assertEqual(LR.verify_initramfs(ok), [])
        self.assertEqual(LR.verify_initramfs("cryptsetup\nusbhid.ko"), ["hid-multitouch", "hid-microsoft", "xhci", "dm-crypt"])

    def test_validate(self):
        c = {"fs": "xfs-luks", "luks_passphrase": "short", "disks": ["/dev/sda"]}
        self.assertTrue(LR.validate(c))
        c["luks_passphrase"] = GOOD_PW
        self.assertEqual(LR.validate(c), [])
        self.assertEqual(LR.validate({"fs": "ext4"}), [])

    def test_config_defaults_and_answers(self):
        prof = dict(PROFILE, filesystems=["ext4", "xfs", "zfs", "xfs-luks"])
        c = S.default_config(prof)
        self.assertIn(c["fs"], ("zfs", "xfs-luks"))
        self.assertEqual(c["luks_passphrase"], "")
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[global]\nroot_password_hash = "%s"\n[disk]\nfilesystem = "xfs-luks"\nluks_passphrase = "%s"\ndisks = ["/dev/vda"]\n'
                    '[desktop]\nenable = true\nuser = "surface"\nautologin = true\n[hardware]\nprofile = "surface"\n' % (HASH, GOOD_PW))
        c = S.config_from_answers(prof, f.name)
        os.unlink(f.name)
        self.assertEqual((c["fs"], c["luks_passphrase"], c["hw"]), ("xfs-luks", GOOD_PW, "surface"))
        self.assertTrue(c["desktop"]["enable"] and c["desktop"]["autologin"])
        self.assertEqual(S.validate_config(prof, c), [])
        c["luks_passphrase"] = "x"
        self.assertTrue(any("LUKS" in e for e in S.validate_config(prof, c)))
