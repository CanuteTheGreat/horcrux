"""horcrux-boot-sync must pick the grub EFI target of the machine it runs on (aarch64 mirror installs died
on a hardcoded x86_64-efi: daily aarch64 runs 13704/14039)."""
import pathlib
import re
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "overlay/usr/bin/horcrux-boot-sync"


class BootSyncArch(unittest.TestCase):
    def setUp(self):
        self.s = SCRIPT.read_text()

    def test_target_follows_uname(self):
        self.assertIn("uname -m", self.s)
        self.assertRegex(self.s, r"aarch64\|arm64\)\s+efi_target=arm64-efi")
        self.assertRegex(self.s, r"riscv64\)\s+efi_target=riscv64-efi")

    def test_no_unconditional_x86_target(self):
        self.assertIsNone(re.search(r"^efi_target=x86_64-efi", self.s, re.M))

    def test_grub_install_errors_not_swallowed(self):
        self.assertNotIn(">/dev/null 2>&1 ||", self.s)


if __name__ == "__main__":
    unittest.main()
