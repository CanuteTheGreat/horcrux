"""An open console (console_open) must not be scrolled by kernel INFO messages: the installer writes a sysctl that limits the console loglevel,
and the QEMU harness recognises sudo's pty-run menu on the VGA console.
Run: python3 -m unittest discover -s build/live-iso/common/tests   (Python >= 3.11)
"""
import os
import re
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import system as S  # noqa: E402


class ConsoleOpenPrintk(unittest.TestCase):
    def test_sysctl_limits_console_to_errors(self):
        m = re.search(r"^kernel\.printk\s*=\s*(\d+)", S.CONSOLE_OPEN_SYSCTL, re.M)
        self.assertTrue(m)
        self.assertLessEqual(int(m.group(1)), 4)     # INFO (6) and NOTICE (5) never reach the console

    def test_installer_writes_it_only_for_open_console(self):
        src = open(os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer", "liveinst", "system.py")).read()
        blk = src[src.index('if c.get("console_open"):'):]
        blk = blk[:blk.index("def cfg_use_flags")]
        self.assertIn("CONSOLE_OPEN_SYSCTL", blk)


if __name__ == "__main__":
    unittest.main()
