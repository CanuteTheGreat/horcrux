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


if __name__ == "__main__":
    unittest.main()
