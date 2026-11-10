"""NetworkManager Wi-Fi carry-over: no pinned interface name, mode 0600 (hogshead first-boot lesson d)."""
import os
import stat
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import system as S  # noqa: E402
from liveinst import luksroot as L  # noqa: E402


class NMProfile(unittest.TestCase):
    def test_profile_not_pinned_and_private(self):
        with tempfile.TemporaryDirectory() as d:
            p = S.write_nm_wifi_profile(d, "Home Net", "secret123")
            self.assertEqual(stat.S_IMODE(os.stat(p).st_mode), 0o600)
            t = open(p).read()
            self.assertNotIn("interface-name", t)
            self.assertIn("autoconnect=true", t)
            self.assertIn("psk=secret123", t)

    def test_strip_pinned(self):
        with tempfile.TemporaryDirectory() as d:
            sc = os.path.join(d, "etc/NetworkManager/system-connections")
            os.makedirs(sc)
            open(os.path.join(sc, "a.nmconnection"), "w").write("[connection]\nid=a\ninterface-name=wlan0\ntype=wifi\n")
            self.assertEqual(S.strip_pinned_interfaces(d), 1)
            self.assertNotIn("interface-name", open(os.path.join(sc, "a.nmconnection")).read())
            self.assertEqual(S.strip_pinned_interfaces(d), 0)

    def test_early_kms_in_dracut_conf(self):
        c = L.dracut_conf("i915 dm_crypt")
        self.assertIn('force_drivers+=" i915 "', c)
        self.assertIn("i915", c.split("add_drivers")[1])


if __name__ == "__main__":
    unittest.main()
