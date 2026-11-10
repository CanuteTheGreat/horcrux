"""Mutter ScreenCast frame-limiter patch (remote desktop capped at 30-40 fps at 60 Hz): one patch, shipped by the image's portage tree AND by
sys-apps/horcrux-repo-config (installed machines), applied to source builds only, so make.conf must keep mutter out of binary packages.

Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus.
"""
import os
import re
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
GENTOO = os.path.join(REPO, "gentoo")
LIVE = os.path.join(REPO, "build", "live-iso")
HAVE = os.path.isdir(os.path.join(GENTOO, "sys-apps", "horcrux-repo-config"))

IMAGE_PATCH = os.path.join(LIVE, "project", "variants", "gnome-light", "portage", "patches", "x11-wm", "mutter",
                           "0001-screencast-round-to-nearest-interval.patch")
PKG_DIR = os.path.join(GENTOO, "sys-apps", "horcrux-repo-config")
PKG_PATCH = os.path.join(PKG_DIR, "files", "mutter-screencast-nearest-interval.patch")


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class MutterPatch(unittest.TestCase):
    def test_one_patch_two_homes(self):
        self.assertEqual(open(IMAGE_PATCH).read(), open(PKG_PATCH).read(), "the image copy and the package copy drifted")

    def test_patch_is_the_limiter_fix(self):
        p = open(IMAGE_PATCH).read()
        self.assertIn("+++ b/src/backends/meta-screen-cast-stream-src.c", p)
        self.assertIn("min_interval_us / 2 < min_interval_us", p)
        self.assertIn("too early (nearest interval boundary)", p, "the Dockerfile check greps libmutter for this string")
        self.assertIn("next_last_frame_timestamp_us", p)

    def test_repo_config_installs_it(self):
        ebs = sorted(f for f in os.listdir(PKG_DIR) if f.endswith(".ebuild"))
        text = open(os.path.join(PKG_DIR, ebs[-1])).read()
        self.assertIn("/etc/portage/patches/x11-wm/mutter", text)
        self.assertIn("mutter-screencast-nearest-interval.patch", text)
        self.assertEqual(len(ebs), 1, "only the newest repo-config revision stays: %s" % ebs)

    def test_binary_mutter_is_excluded_everywhere_the_patch_matters(self):
        mk = open(os.path.join(LIVE, "common", "portage", "make.conf")).read()
        self.assertRegex(mk, r"EMERGE_DEFAULT_OPTS=.*--usepkg-exclude=x11-wm/mutter")

    def test_image_build_fails_when_the_patch_is_not_applied(self):
        d = open(os.path.join(LIVE, "project", "variants", "gnome-light", "Dockerfile")).read()
        self.assertRegex(d, r"strings /usr/lib64/libmutter-\*\.so\.0 \| grep -q 'too early \(nearest interval boundary\)'")


if __name__ == "__main__":
    unittest.main()
