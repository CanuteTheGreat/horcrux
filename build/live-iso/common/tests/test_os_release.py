"""Horcrux os-release: package-owned (sys-apps/horcrux-release), versioned, no live/installer text, no personal Forgejo URL."""
import os
import re
import subprocess
import tempfile
import unittest

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
PKG = os.path.join(REPO, "gentoo", "sys-apps", "horcrux-release")
GEN = os.path.join(PKG, "files", "render-os-release.sh")
LIVE = os.path.join(os.path.dirname(__file__), "..", "overlay", "usr", "libexec", "live-os-release")
HAVE = os.path.isfile(GEN)
PV = "0.1.0_p202611122159"


def render(*a):
    r = subprocess.run(["bash", GEN, *a], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return r.stdout


def parse(t):
    return dict(re.findall(r'^([A-Z_]+)=(?:"([^"]*)"|(\S*))$', t, re.M) and
                [(k, a or b) for k, a, b in re.findall(r'^([A-Z_]+)=(?:"([^"]*)"|(\S*))$', t, re.M)])


@unittest.skipUnless(HAVE, "no horcrux-release package")
class OsRelease(unittest.TestCase):
    CASES = [("server",), ("gnome-light",), ("gnome-light", "hogsmeade"), ("gnome-light", "hogshead"),
             ("gnome-light", "gringotts"), ("gnome-light", "surface"), ("server", "test-machine")]

    def test_every_variant_is_complete_and_clean(self):
        for c in self.CASES:
            t = render(PV, *c)
            kv = parse(t)
            with self.subTest(c=c):
                self.assertEqual(kv["NAME"], "Horcrux")
                self.assertEqual(kv["ID"], "horcrux")
                self.assertEqual(kv["ID_LIKE"], "gentoo")
                self.assertEqual(kv["VERSION_ID"], "0.1.0")
                self.assertEqual(kv["BUILD_ID"], "202611122159")
                self.assertEqual(kv["VARIANT_ID"], c[-1] if len(c) > 1 else c[0])
                self.assertTrue(kv["VARIANT"])
                self.assertNotRegex(kv["PRETTY_NAME"], r"(?i)live|installer")
                self.assertTrue(kv["PRETTY_NAME"].startswith("Horcrux 0.1 ("))
                self.assertNotIn("git.canutethegreat.com", t)
                for k in ("HOME_URL", "DOCUMENTATION_URL", "SUPPORT_URL", "BUG_REPORT_URL"):
                    self.assertRegex(kv[k], r"^https://(horcrux\.errant\.solutions/|github\.com/CanuteTheGreat/horcrux/issues$)")

    def test_release_without_stamp_has_no_build_id(self):
        self.assertNotIn("BUILD_ID", render("0.1.0", "server"))

    def test_bad_input_rejected(self):
        self.assertNotEqual(subprocess.run(["bash", GEN, PV, "bogus"], capture_output=True).returncode, 0)
        self.assertNotEqual(subprocess.run(["bash", GEN, PV, "server", "bogus"], capture_output=True).returncode, 0)

    def test_ebuild_installs_the_file_and_masks_baselayout(self):
        e = open(os.path.join(PKG, "horcrux-release-0.1.0.ebuild")).read()
        self.assertIn("doins \"${T}\"/os-release", e)
        self.assertIn("/usr/lib", e)
        self.assertIn("INSTALL_MASK=\"/usr/lib/os-release\"", open(os.path.join(PKG, "files", "horcrux-baselayout.conf")).read())
        self.assertIn("sys-apps/baselayout horcrux-baselayout.conf", open(os.path.join(PKG, "files", "package.env")).read())

    def test_newest_horcrux_bin_has_its_release(self):
        st = lambda d, pre: sorted(re.sub(r"-r\d+$", "", f[len(pre):-len(".ebuild")]) for f in os.listdir(d) if f.startswith(pre) and "_p" in f)
        bn = os.path.join(REPO, "gentoo", "app-emulation", "horcrux-bin")
        self.assertIn(st(bn, "horcrux-bin-")[-1], st(PKG, "horcrux-release-"))

    def test_variant_layers_name_their_flags(self):
        v = os.path.join(REPO, "build", "live-iso", "project", "variants")
        for var, flags in (("gnome-light", "gnome-light"), ("hogsmeade", "hogsmeade"), ("hogshead", "hogshead"), ("gringotts", "gringotts")):
            t = open(os.path.join(v, var, "portage", "package.use", "horcrux-release")).read()
            self.assertIn("sys-apps/horcrux-release", t)
            self.assertIn(flags, t)

    def test_no_forgejo_url_in_the_package(self):
        for r, _, fs in os.walk(PKG):
            for f in fs:
                if f.endswith(".png"):
                    continue
                self.assertNotIn("git.canutethegreat.com", open(os.path.join(r, f)).read(), f)


class LiveMarker(unittest.TestCase):
    def test_live_session_says_live_once(self):
        with tempfile.NamedTemporaryFile("w", suffix=".osr", delete=False) as f:
            f.write('NAME="Horcrux"\nPRETTY_NAME="Horcrux 0.1 (GNOME Light)"\nVERSION_ID=0.1.0\n')
        try:
            for _ in range(2):
                subprocess.run(["sh", LIVE, f.name], check=True)
            t = open(f.name).read()
            self.assertIn('PRETTY_NAME="Horcrux 0.1 (GNOME Light, Live)"', t)
            self.assertEqual(t.count("Live"), 1)
        finally:
            os.unlink(f.name)


if __name__ == "__main__":
    unittest.main()
