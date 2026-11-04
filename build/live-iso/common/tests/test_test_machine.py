"""'Test machine: NO disk encryption' (testmachine.py): explicit, warned, non-default; gates for the answer file, swap and initramfs."""
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import portableswap as PS  # noqa: E402
from liveinst import system as S  # noqa: E402
from liveinst import testmachine as TM  # noqa: E402
from liveinst import zfsswap as ZS  # noqa: E402

PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs", "xfs-luks"], "swap_mb": 4096, "network_mode": "management",
           "services": [], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}
INITRD_ENC = "usr/bin/hx-unlock\nusr/lib/dracut/hooks/pre-mount/50-hx-zfskey.sh\nusr/bin/cryptsetup\nusr/lib/modules/x/extra/zfs.ko\nusr/lib/dracut/resume.sh\n"
INITRD_PLAIN = "usr/lib/modules/x/extra/zfs.ko\nusr/lib/dracut/resume.sh\nusr/lib/dracut/hooks/mount/98-mount-zfs.sh\n"


class TestMachine(unittest.TestCase):
    def test_default_is_encrypted_world_not_test_machine(self):
        self.assertFalse(S.default_config(PROFILE).get(TM.KEY))

    def test_unencrypted_zfs_desktop_needs_the_explicit_choice(self):
        self.assertTrue(TM.problems({"install_type": "desktop", "fs": "zfs", "zfs_encrypt": False}))
        self.assertEqual(TM.problems({"install_type": "desktop", "fs": "zfs", "zfs_encrypt": False, TM.KEY: True}), [])
        self.assertEqual(TM.problems({"install_type": "server", "fs": "zfs", "zfs_encrypt": False}), [])      # servers/appliances unchanged
        self.assertEqual(TM.problems({"install_type": "desktop", "fs": "zfs", "zfs_encrypt": True}), [])

    def test_contradiction_refused(self):
        self.assertTrue(TM.problems({TM.KEY: True, "zfs_encrypt": True, "fs": "zfs"}))
        self.assertTrue(TM.problems({TM.KEY: True, "fs": "ext4"}))

    def test_xfs_luks_becomes_plain_xfs_without_passphrases(self):
        c = TM.normalize({TM.KEY: True, "fs": "xfs-luks", "luks_passphrase": "x" * 14, "zfs_encrypt": True, "zfs_passphrase": "y"})
        self.assertEqual((c["fs"], c["luks_passphrase"], c["zfs_encrypt"], c["zfs_passphrase"]), ("xfs", "", False, ""))
        self.assertTrue(PS.layout_supported({"fs": "xfs"}))

    def test_answer_file_key(self):
        import tempfile
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[disk]\nfilesystem = "zfs"\ntest_machine_no_encryption = true\n')
        try:
            r = S.config_from_answers(PROFILE, f.name)
        finally:
            os.unlink(f.name)
        c = r[0] if isinstance(r, tuple) else r
        self.assertTrue(c[TM.KEY])
        self.assertFalse(c["zfs_encrypt"])

    def test_zfs_gets_plain_swap_on_test_machine(self):
        self.assertTrue(PS.layout_supported({"fs": "zfs", TM.KEY: True}))
        self.assertFalse(PS.layout_supported({"fs": "zfs"}))
        self.assertIn("3:8200", ZS.sgdisk_part(4608, plain=True))
        self.assertIn("3:8309", ZS.sgdisk_part(4608))

    def test_bootconf_never_names_the_unlock_module_in_add(self):
        t = TM.bootconf("uuid")
        self.assertIn('add_dracutmodules+=" resume zfs "', t)
        self.assertIn("omit_dracutmodules", t)
        self.assertNotIn("add_dracutmodules+=\" crypt", t)
        self.assertIn('add_dracutmodules+=" zfs "', TM.bootconf(""))

    def test_initramfs_gate(self):
        self.assertEqual(ZS.plain_initramfs_problems(INITRD_PLAIN), [])
        bad = ZS.plain_initramfs_problems(INITRD_ENC)
        self.assertTrue(any("hx-unlock" in b for b in bad) and any("cryptsetup" in b for b in bad))
        self.assertTrue(ZS.plain_initramfs_problems("nothing"))


if __name__ == "__main__":
    unittest.main()
