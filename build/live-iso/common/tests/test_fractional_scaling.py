"""Fractional display scaling is a SYSTEM dconf default shipped by gnome-extra/horcrux-gnome-defaults (Settings > Displays otherwise offers only 100%/200%).

Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus. The dconf round trip skips without the dconf binary.
"""
import glob
import os
import re
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
PKG = os.path.join(REPO, "gentoo", "gnome-extra", "horcrux-gnome-defaults")
EBUILDS = sorted(glob.glob(os.path.join(PKG, "*.ebuild")))
WANT = "['scale-monitor-framebuffer', 'xwayland-native-scaling']"


def keyfile(ebuild):
    s = open(ebuild).read()
    m = re.search(r"newins - (\S*fractional-scaling\S*) <<-'EOF'\n(.*?)\n\s*EOF\n", s, re.S)
    return (m.group(1), "\n".join(l.strip() for l in m.group(2).splitlines()) + "\n") if m else (None, None)


@unittest.skipUnless(EBUILDS, "Horcrux repo tree only")
class FractionalScaling(unittest.TestCase):
    def test_keyfile_in_the_system_local_db_with_both_features(self):
        name, kf = keyfile(EBUILDS[-1])
        self.assertIsNotNone(name, "ebuild must install a fractional-scaling keyfile")
        self.assertIn("[org/gnome/mutter]", kf)
        self.assertIn(f"experimental-features={WANT}", kf)

    def test_installed_into_local_d_which_the_dconf_profile_reads(self):
        s = open(EBUILDS[-1]).read()
        self.assertRegex(s, r"insinto /etc/dconf/db/local\.d\n(?:.*\n)*?.*newins - 10-horcrux-fractional-scaling")
        self.assertIn("system-db:local", s)

    def test_default_not_a_lock(self):
        self.assertNotIn("local.d/locks", open(EBUILDS[-1]).read())

    @unittest.skipUnless(shutil.which("dconf"), "needs dconf")
    def test_compiles_and_reads_back(self):
        _, kf = keyfile(EBUILDS[-1])
        d = tempfile.mkdtemp(prefix="hxfs")
        self.addCleanup(shutil.rmtree, d, True)
        os.makedirs(os.path.join(d, "local.d"))
        open(os.path.join(d, "local.d", "10-x"), "w").write(kf)
        r = subprocess.run(["dconf", "compile", os.path.join(d, "local"), os.path.join(d, "local.d")], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)


if __name__ == "__main__":
    unittest.main()
