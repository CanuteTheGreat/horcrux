"""An installed xfs-luks root asks for the disk passphrase on the console at every boot; the harness types it (lab installer-vm 2026-11-06: the
server-headless run waited 421 s at the prompt and failed 'installed-boots-to-login')."""
import os, re, sys, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = open(os.path.join(HERE, "..", "qemu-test.py")).read()


class TestLuksBootUnlock(unittest.TestCase):
    def test_prompt_regex_matches_the_usual_initramfs_prompts(self):
        m = re.search(r"^LUKS_PROMPT = (re\.compile\(.*\))$", SRC, re.M)
        self.assertTrue(m, "LUKS_PROMPT defined")
        rx = eval(m.group(1), {"re": re})
        for text in (b"Please enter passphrase for disk cryptroot:", b"Enter passphrase for /dev/vda3: ", b"enter key"):
            self.assertTrue(rx.search(text), text)
        self.assertFalse(rx.search(b"localhost login: "))

    def test_unlock_is_wired_for_horcrux_xfs_luks_only(self):
        self.assertIn('VM.LUKS_UNLOCK = PW_DISK if (args.fs == "xfs-luks" and args.project == "horcrux") else None', SRC)
        self.assertIn('self.name[:1] in ("2", "3")', SRC)      # installed boots (2-installed, 3-bootN), never the wrong-TPM phase (4-)


if __name__ == "__main__":
    unittest.main()
