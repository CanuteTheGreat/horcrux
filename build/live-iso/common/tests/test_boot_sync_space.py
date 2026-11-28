"""horcrux-boot-sync keeps hardlinks and is safe on a full boot copy (kernel 6.18.65 install died with ENOSPC on 2026-11-27:
rsync -aqx without -H needed 2.1 GiB for a 1.2 GiB /boot on 2 GiB copies)."""
import os
import pathlib
import subprocess
import unittest

D = pathlib.Path(__file__).resolve().parent
SCRIPT = D.parent / "overlay/usr/bin/horcrux-boot-sync"
PKG = D.parents[3] / "gentoo/sys-apps/horcrux-be/files/horcrux-boot-sync"


class BootSyncSpace(unittest.TestCase):
    def test_keeps_hardlinks(self):
        self.assertIn("RSYNC_OPTS=(-aHqx)", SCRIPT.read_text())

    def test_failed_copy_keeps_old_menu(self):
        s = SCRIPT.read_text()
        self.assertLess(s.index("if sync_tree /boot"), s.index("grub.cfg.new"))

    @unittest.skipUnless(PKG.exists(), "the sys-apps/horcrux-be copy exists only in the Horcrux repository (common/ is vendored into Patronus)")
    def test_package_copy_identical(self):
        self.assertEqual(SCRIPT.read_text(), PKG.read_text())

    @unittest.skipUnless(os.geteuid() == 0 and os.path.exists("/dev/loop-control"), "needs root + loop devices")
    def test_full_copy_repair(self):
        r = subprocess.run([str(D / "boot_sync_space_test.sh")], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)


if __name__ == "__main__":
    unittest.main()
