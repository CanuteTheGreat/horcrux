"""Regression (2026-11-17, every live-iso run since the per-job output dir iso-out-<image>-<arch>): QEMU refused the QMP/serial sockets under
/workspace/<owner>/<repo>/iso-out-server-systemd-x86_64/evidence/<scenario>/ with "UNIX socket path ... is too long" (sun_path is 108 bytes),
so every scenario died at start. Deep paths now get short per-process socket paths; short ones stay beside the logs."""
import importlib.util
import sys
import os
import unittest

COMMON = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, COMMON)


class SockPaths(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        sp = importlib.util.spec_from_file_location("qt_sockpaths", os.path.join(COMMON, "qemu-test.py"))
        cls.qt = importlib.util.module_from_spec(sp)
        sp.loader.exec_module(cls.qt)

    def test_short_path_stays_beside_the_logs(self):
        self.assertEqual(self.qt.sock_paths("/tmp/out", "2-boot"), ("/tmp/out/2-boot.qmp", "/tmp/out/2-boot.serial"))

    def test_ci_workspace_path_is_shortened_and_unique(self):
        out = "/workspace/CanuteTheGreat/horcrux/iso-out-server-systemd-x86_64/evidence/uefi-usb-tui-vga-zfs-mirror-static"
        q1, s1 = self.qt.sock_paths(out, "1-install")
        q2, s2 = self.qt.sock_paths(out, "2-installed")
        q3, _ = self.qt.sock_paths(out.replace("uefi-usb", "bios-cdrom"), "1-install")
        for p in (q1, s1, q2, s2, q3):
            self.assertLessEqual(len(p), 107, p)
        self.assertEqual(len({q1, s1, q2, s2, q3}), 5)
        self.assertEqual(self.qt.sock_paths(out, "1-install"), (q1, s1))      # stable for one VM
        self.assertTrue(os.path.isdir(os.path.dirname(q1)))
        self.assertEqual(oct(os.stat(os.path.dirname(q1)).st_mode & 0o777), "0o700")


if __name__ == "__main__":
    unittest.main()
