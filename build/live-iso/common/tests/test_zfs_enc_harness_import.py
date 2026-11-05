"""Regression (pqa 2026-11-05): zfs-enc-test.py loaded zfsswap.py standalone via spec_from_file_location, which broke
as soon as zfsswap gained `from . import testmachine` (daily gnome-light failed at qemu-zfs-enc-desktop-hibernate with
ImportError after every other hibernate step had passed). The harness must import it as part of the liveinst package."""
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.dirname(HERE)
PKG_ROOT = os.path.join(COMMON, "overlay/usr/lib/live-installer")


class HarnessImport(unittest.TestCase):
    def test_zfsswap_imports_as_package(self):
        sys.path.insert(0, PKG_ROOT)
        try:
            from liveinst import zfsswap
        finally:
            sys.path.remove(PKG_ROOT)
        self.assertTrue(callable(zfsswap.initramfs_problems) and callable(zfsswap.hook_order_ok))

    def test_harness_does_not_load_zfsswap_standalone(self):
        src = open(os.path.join(COMMON, "zfs-enc-test.py")).read()
        self.assertNotIn('spec_from_file_location("zfsswap"', src)
        self.assertIn("from liveinst import zfsswap", src)


if __name__ == "__main__":
    unittest.main()
