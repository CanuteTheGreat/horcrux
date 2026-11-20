"""The TPM-unlock gate runs lsinitrd over every initramfs on the guest: it must not use ssh()'s 40 s default (run 21014 timed out)."""
import os
import re
import unittest

SRC = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "qemu-test.py")).read()


class TpmInitrdTimeout(unittest.TestCase):
    def test_lsinitrd_call_has_long_timeout_and_retry(self):
        m = re.search(r"for _try in range\(2\):\n\s+rc, t = ssh\(FWD_SSH, self\.key, \"ls -l /etc/tpm-unlock;.*?\)\n\s+if t\.strip\(\) != \"timeout\":", SRC, re.S)
        self.assertTrue(m, "retry loop around the lsinitrd ssh call is missing")
        self.assertIn("timeout=int(T(240))", m.group(0))


if __name__ == "__main__":
    unittest.main()
