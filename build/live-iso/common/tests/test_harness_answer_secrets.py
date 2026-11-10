"""The QEMU harness's own answer file must pass the installer's secret rules (lab 2026-11-06: installer-vm server-headless on 4 GB laptops
defaulted to xfs-luks and sent the disk passphrase equal to root's password -> ANSWER-FILE-ERROR before anything was installed;
CI never saw it because its default run has >= 8 GB, so no disk passphrase)."""
import argparse
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.join(HERE, "..")
sys.path.insert(0, os.path.join(COMMON, "overlay", "usr", "lib", "live-installer"))
sys.path.insert(0, COMMON)
from liveinst import secretcheck as SC  # noqa: E402
from liveinst import system as S  # noqa: E402

PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs", "xfs-luks"], "swap_mb": 0, "network_mode": "management",
           "services": [], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}


def load_harness():
    spec = importlib.util.spec_from_file_location("hx_qemu_test", os.path.join(COMMON, "qemu-test.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


class HarnessAnswerSecrets(unittest.TestCase):
    def test_disk_passphrase_differs_from_every_account_password(self):
        qt = load_harness()
        self.assertNotEqual(qt.PW_DISK, qt.PW)
        for fs in ("xfs-luks", "zfs", "xfs"):
            for fs_default in (False, True):
                a = argparse.Namespace(project="horcrux", fs=fs, fs_default=fs_default, zfs_raid="single", ssh_password_opt_in=False,
                                       console_open=False, static_ip="", lan_ip="10.0.0.1", mode="uefi")
                pwhash = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemutest1", qt.PW], text=True).strip()
                with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
                    f.write(qt.answer_toml(a, "ssh-ed25519 AAAA test", pwhash))
                try:
                    c = S.config_from_answers(PROFILE, f.name)
                finally:
                    os.unlink(f.name)
                self.assertEqual(SC.reuse_problems(c), [], f"fs={fs}")
                if fs == "xfs-luks":
                    self.assertEqual(c["luks_passphrase"], qt.PW_DISK)


if __name__ == "__main__":
    unittest.main()
