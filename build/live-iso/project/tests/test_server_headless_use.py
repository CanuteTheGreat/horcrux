"""Server editions keep the console encoder's ffmpeg free of display/font libraries; desktop variants drop that trim.

Why: horcrux-bin RDEPENDs media-video/ffmpeg[vpx,x264] (Horcrux Console encoder). With ffmpeg's default USE the first update of a
server pulled libdrm, fontconfig, freetype, harfbuzz[cairo] and cairo (installer-VM gate server-has-no-display-stack-installed).
"""
import os
import re
import unittest

LIVE = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
TRIM = "/etc/portage/package.use/horcrux-server-headless"


def _rd(*p):
    return open(os.path.join(LIVE, *p), encoding="utf-8").read()


class ServerHeadlessUse(unittest.TestCase):
    def test_server_image_trims_ffmpeg(self):
        lines = [l.split() for l in _rd("project", "portage", "package.use", "horcrux-server-headless").splitlines()
                 if l.strip() and not l.startswith("#")]
        ff = [l for l in lines if l[0] == "media-video/ffmpeg"]
        self.assertEqual(len(ff), 1)
        for flag in ("-drm", "-fontconfig", "-libass", "-truetype"):
            self.assertIn(flag, ff[0][1:])
        # the encoders the console needs are never turned off here
        for flag in ("-vpx", "-x264", "-gpl"):
            self.assertNotIn(flag, ff[0][1:])

    def test_desktop_variants_remove_the_trim(self):
        for variant in ("gnome-light", "surface"):
            df = _rd("project", "variants", variant, "Dockerfile")
            self.assertIn("rm -f " + TRIM, df, variant)
        # the removal comes before the desktop profile / desktop package emerge
        gl = _rd("project", "variants", "gnome-light", "Dockerfile")
        self.assertLess(gl.index("rm -f " + TRIM), gl.index("eselect profile set"))

    def test_core_gate_still_refuses_display_packages(self):
        core = _rd("project", "variants", "core", "Dockerfile")
        self.assertRegex(core, r"grep -E '\^\(gui-libs\|gui-wm\|x11-base\|x11-libs")


if __name__ == "__main__":
    unittest.main()
