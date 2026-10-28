"""Per-architecture facts: boot method, GRUB target, serial console detection."""
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import arch as A  # noqa: E402


class ArchTests(unittest.TestCase):
    def test_boot_targets(self):
        self.assertEqual(A.boot("x86_64")["efi_target"], "x86_64-efi")
        self.assertEqual(A.boot("aarch64")["efi_target"], "arm64-efi")
        self.assertEqual(A.boot("aarch64")["efi_file"], "BOOTAA64.EFI")
        self.assertEqual(A.boot("riscv64")["efi_target"], "riscv64-efi")

    def test_bios_only_on_x86(self):
        self.assertTrue(A.has_bios("x86_64"))
        self.assertFalse(A.has_bios("aarch64"))
        self.assertFalse(A.has_bios("riscv64"))

    def test_serial_detection(self):
        self.assertEqual(A.serial_console("quiet console=tty0 console=ttyS0,115200n8 x=1"), ("ttyS0", 115200))
        self.assertEqual(A.serial_console("console=ttyAMA0,115200n8 quiet"), ("ttyAMA0", 115200))
        self.assertEqual(A.serial_console("console=hvc0"), ("hvc0", 115200))
        self.assertEqual(A.serial_console("console=ttyS1,9600n8"), ("ttyS1", 9600))
        self.assertIsNone(A.serial_console("quiet root=live:LABEL=X console=tty0"))
        self.assertIsNone(A.serial_console("quiet"))

    def test_console_args(self):
        self.assertEqual(A.console_args("console=ttyAMA0,115200n8"), "console=tty0 console=ttyAMA0,115200n8")
        self.assertEqual(A.console_args("quiet"), "")

    def test_grub_serial_x86_only(self):
        self.assertEqual(A.grub_serial_lines("console=ttyS0,115200n8", "x86_64"), "serial --unit=0 --speed=115200")
        self.assertEqual(A.grub_serial_lines("console=ttyS1,9600n8", "x86_64"), "serial --unit=1 --speed=9600")
        self.assertIsNone(A.grub_serial_lines("console=ttyAMA0,115200n8", "aarch64"))
        self.assertIsNone(A.grub_serial_lines("quiet", "x86_64"))

    def test_machine_normalised(self):
        self.assertIn(A.machine(), ("x86_64", "aarch64", "riscv64"))


class ShellHelperTests(unittest.TestCase):
    def test_live_serial_dev_matches_python(self):
        import subprocess
        script = os.path.join(HERE, "..", "overlay", "usr", "libexec", "live-serial-dev")
        src = open(script).read()
        for cl, want in (("console=tty0 console=ttyAMA0,115200n8", "ttyAMA0"), ("console=ttyS0,115200n8", "ttyS0"), ("quiet", None)):
            r = subprocess.run(["sh", "-c", src.replace("/proc/cmdline", "/dev/stdin")], input=cl, capture_output=True, text=True)
            self.assertEqual(r.stdout.strip() or None, want)


if __name__ == "__main__":
    unittest.main()
