"""The Hermes agent has its own HOGSMEADE-only login (acct-user/hermes-agent + app-misc/hogsmeade-agent-account in the hogsmeade variant's local repo),
so agent ssh sessions never are pam_openrc sessions of the owner's user (they kept his OpenRC user runlevel and a dead gnome-shell alive across a GNOME
logout, 2026-11-13). Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus."""
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
LIVE = REPO / "build" / "live-iso"
HSM = LIVE / "project" / "variants" / "hogsmeade"
LR = HSM / "local-repo"
ACC = LR / "app-misc" / "hogsmeade-agent-account" / "hogsmeade-agent-account-1.ebuild"
USR = LR / "acct-user" / "hermes-agent" / "hermes-agent-0.ebuild"
GRP = LR / "acct-group" / "hermes-agent" / "hermes-agent-0.ebuild"


@unittest.skipUnless((REPO / "gentoo").is_dir(), "Horcrux repo tree only")
class HogsmeadeAgentAccount(unittest.TestCase):
    def setUp(self):
        if not ACC.is_file():
            self.skipTest("Horcrux repo tree only")
        self.t = ACC.read_text()

    def test_account_is_a_locked_acct_user(self):
        u = USR.read_text()
        self.assertIn("inherit acct-user", u)
        self.assertIn("acct-user_add_deps", u)
        self.assertRegex(u, r"ACCT_USER_GROUPS=\( hermes-agent ")
        self.assertNotRegex(u, r"ACCT_USER_ID=1000\b")  # never the owner's uid
        self.assertIn("inherit acct-group", GRP.read_text())
        self.assertIn("acct-user/hermes-agent", self.t)

    def test_sudo_dropin_sorts_last_and_is_valid(self):
        self.assertGreater("zz-hermes-agent", "wheel")
        self.assertRegex(self.t, r"insopts -m0?440")
        m = re.search(r"newins - zz-hermes-agent <<-'EOF2'\n(.*?)\n\tEOF2", self.t, re.S)
        self.assertIsNotNone(m)
        self.assertEqual(m.group(1).strip(), "hermes-agent ALL=(ALL) NOPASSWD: ALL")
        self.assertIn("visudo -c", self.t.split("pkg_preinst()", 1)[1])
        if shutil.which("visudo"):
            with tempfile.TemporaryDirectory() as d:
                f = Path(d) / "zz-hermes-agent"
                f.write_text(m.group(1).strip() + "\n")
                f.chmod(0o440)
                r = subprocess.run(["visudo", "-c", "-q", "-f", str(f)], capture_output=True, text=True)
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_ssh_is_certificate_only(self):
        # no packaged key: login works only with a personal-CA certificate carrying the principal hermes-agent
        for p in (USR, ACC):
            self.assertNotIn("authorized_keys", re.sub(r"(?m)^#.*$", "", p.read_text()))
        self.assertIn("net-misc/ctg-ssh-ca-fleet", self.t)

    def test_image_layer_installs_it_and_public_images_refuse_it(self):
        for d in ("Dockerfile", "Dockerfile.part2"):
            self.assertIn("app-misc/hogsmeade-agent-account", (HSM / d).read_text(), d)
        forb = (LIVE / "project" / "public-forbidden.txt").read_text().split()
        self.assertIn("app-misc/hogsmeade-agent-account", forb)
        self.assertIn("acct-user/hermes-agent", forb)
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == Path(__file__).name:
                    continue
                if HSM in p.parents or p.name in ("public-forbidden.txt", "personal-apps.denylist", "personal-paths.denylist"):
                    continue
                try:
                    if re.search(r"hogsmeade-agent-account|zz-hermes-agent|acct-user/hermes-agent", p.read_text()):
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "the agent account belongs to the HOGSMEADE layer only")


if __name__ == "__main__":
    unittest.main()
