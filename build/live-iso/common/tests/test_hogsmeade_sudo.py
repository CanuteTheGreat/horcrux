"""Passwordless sudo for the owner is a HOGSMEADE-only, package-owned drop-in (app-misc/hogsmeade-sudo in the hogsmeade variant's local repo).
Public images keep the password prompt and root locked (owner rule: no default passwordless sudo). Reads the Horcrux repo tree; skipped in the
vendored copy of common/ in Patronus."""
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
PKG = HSM / "local-repo" / "app-misc" / "hogsmeade-sudo"
EB = PKG / "hogsmeade-sudo-1.ebuild"
NAME = "zz-canutethegreat"


@unittest.skipUnless((REPO / "gentoo").is_dir(), "Horcrux repo tree only")
class HogsmeadeSudo(unittest.TestCase):
    def setUp(self):
        if not EB.is_file():
            self.skipTest("Horcrux repo tree only")
        self.t = EB.read_text()

    def test_dropin_name_sorts_after_wheel_and_mode_is_0440(self):
        # sudoers.d is read in lexical order, the last match wins: it must come after the base image's wheel rule
        self.assertIn("/etc/sudoers.d/wheel", (LIVE / "common" / "setup-rootfs.sh").read_text())
        self.assertGreater(NAME, "wheel")
        self.assertRegex(self.t, r"insinto /etc/sudoers\.d")
        self.assertRegex(self.t, r"insopts -m0?440")
        self.assertRegex(self.t, r"newins - " + NAME + r" <<-'EOF2'")
        self.assertRegex(self.t, r"(?m)^\t\tcanutethegreat ALL=\(ALL\) NOPASSWD: ALL$")

    def test_content_is_one_exact_line(self):
        m = re.search(r"newins - " + NAME + r" <<-'EOF2'\n(.*?)\n\tEOF2", self.t, re.S)
        self.assertIsNotNone(m)
        self.assertEqual(m.group(1).strip(), "canutethegreat ALL=(ALL) NOPASSWD: ALL")

    def test_visudo_check_in_pkg_preinst_and_file_is_valid(self):
        pre = self.t.split("pkg_preinst()", 1)[1]
        self.assertIn("visudo -c", pre)
        self.assertIn("|| die", pre)
        self.assertIn("app-admin/sudo", self.t)
        if shutil.which("visudo"):
            with tempfile.TemporaryDirectory() as d:
                f = Path(d) / NAME
                f.write_text("canutethegreat ALL=(ALL) NOPASSWD: ALL\n")
                f.chmod(0o440)
                r = subprocess.run(["visudo", "-c", "-q", "-f", str(f)], capture_output=True, text=True)
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_image_layer_installs_it(self):
        for d in ("Dockerfile", "Dockerfile.part2"):
            self.assertIn("app-misc/hogsmeade-sudo", (HSM / d).read_text(), d)

    def test_leak_absent_from_public_images(self):
        # the public-image gate (qlist -I) lists it, and nothing outside the HOGSMEADE layer mentions the package or the drop-in
        self.assertIn("app-misc/hogsmeade-sudo", (LIVE / "project" / "public-forbidden.txt").read_text().split())
        pat = re.compile(r"hogsmeade-sudo|" + NAME)
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_sudo.py":
                    continue
                if HSM in p.parents or p.name in ("public-forbidden.txt", "personal-apps.denylist", "personal-paths.denylist"):
                    continue
                try:
                    if pat.search(p.read_text()):
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "passwordless sudo belongs to the HOGSMEADE layer only")
        # and the shared base keeps the prompt: no NOPASSWD anywhere in common/ or the other variants' images
        base = (LIVE / "common" / "setup-rootfs.sh").read_text()
        self.assertNotIn("NOPASSWD", base)
        self.assertIn("passwd -l root", base)
        for p in (LIVE / "project" / "variants").glob("*/Dockerfile*"):
            if p.parent.name != "hogsmeade":
                self.assertNotIn("NOPASSWD", p.read_text(), str(p))


if __name__ == "__main__":
    unittest.main()
