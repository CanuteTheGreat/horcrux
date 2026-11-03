import os, sys, unittest
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import tpmunlock, zfsroot, system, hwprofile  # noqa: E402

PROFILE = {"name": "patronus", "filesystems": ["zfs", "xfs", "ext4"], "encryption": "tpm-or-none", "default_hostname": "p.lan", "services": []}


class TpmPolicy(unittest.TestCase):
    def test_key_is_64_hex(self):
        k = tpmunlock.new_key()
        self.assertEqual(len(k), 64)
        int(k, 16)
        self.assertNotEqual(k, tpmunlock.new_key())

    def test_working_tpm_encrypts(self):
        enc, why = tpmunlock.decide(PROFILE, (True, "ok"))
        self.assertTrue(enc)
        self.assertIn("without a passphrase", why)

    def test_no_tpm_no_encryption(self):
        enc, why = tpmunlock.decide(PROFILE, (False, "no TPM 2.0 device"))
        self.assertFalse(enc)
        self.assertIn("NOT encrypted", why)
        self.assertIn("no TPM", why)

    def test_other_profiles_untouched(self):
        self.assertFalse(tpmunlock.decide({"filesystems": ["zfs"]}, (True, "x"))[0])

    def test_policy_on_config(self):
        c = {"fs": "zfs", "zfs_encrypt": True, "zfs_passphrase": "x"}
        system.apply_encryption_policy(PROFILE, c, (True, "tpm"))
        self.assertTrue(c["zfs_encrypt"]); self.assertEqual(c["zfs_keymode"], "tpm"); self.assertEqual(c["zfs_passphrase"], "")
        c = {"fs": "zfs"}
        system.apply_encryption_policy(PROFILE, c, (False, "none here"))
        self.assertFalse(c["zfs_encrypt"]); self.assertEqual(c["zfs_keymode"], "tpm")
        c = {"fs": "xfs"}                      # the XFS fallback is never an encrypted-with-prompt volume
        system.apply_encryption_policy(PROFILE, c, (True, "tpm"))
        self.assertFalse(c["zfs_encrypt"])
        c = {"fs": "zfs", "enc_request": "none"}
        system.apply_encryption_policy(PROFILE, c, (True, "tpm"))
        self.assertFalse(c["zfs_encrypt"])

    def test_never_a_prompt_profile_fs_list(self):
        self.assertNotIn("xfs-luks", PROFILE["filesystems"])

    def test_pool_args_hex_key(self):
        a = zfsroot.pool_args(True, ["/dev/vda4"], False, "/mnt/t", "hex")
        self.assertIn("keyformat=hex", a)
        self.assertFalse([x for x in a if x.startswith("pbkdf2iters")])
        a = zfsroot.pool_args(True, ["/dev/vda4"], False, "/mnt/t")
        self.assertIn("keyformat=passphrase", a); self.assertIn("pbkdf2iters=1000000", a)

    def test_seal_commands_use_pcr7_policy(self):
        cmds = tpmunlock.seal_cmds("/w", "/w/k")
        flat = [" ".join(c) for c in cmds]
        self.assertTrue(any("tpm2_createpolicy --policy-pcr -l sha256:7" in f for f in flat))
        self.assertTrue(any(c[0] == "tpm2_create" and "fixedtpm|fixedparent|noda" in c for c in cmds))
        u = tpmunlock.unseal_cmds("/w", "/e")[-1]
        self.assertIn("pcr:sha256:7", u)

    def test_initramfs_problems(self):
        self.assertTrue(tpmunlock.initramfs_problems("nothing"))
        ok = "tpm2_unseal tpm2_load tpm2_createprimary etc/tpm-unlock/key.priv etc/tpm-unlock/key.pub libtss2-tcti-device tpm-zfskey"
        self.assertEqual(tpmunlock.initramfs_problems(ok), [])

    def test_default_fs_per_machine(self):
        f = PROFILE["filesystems"]
        self.assertEqual(hwprofile.choose_fs(f, 16384, None, True)[0], "zfs")
        self.assertEqual(hwprofile.choose_fs(f, 4096, None, True)[0], "xfs")
        self.assertEqual(hwprofile.choose_fs(f, 16384, None, False)[0], "xfs")
        for ram in (1024, 4096, 8192, 65536):
            for ok in (True, False):
                self.assertNotEqual(hwprofile.choose_fs(f, ram, None, ok)[0], "ext4")


if __name__ == "__main__":
    unittest.main()
