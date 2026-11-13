"""Owner rule (2026-11-12): x11-themes/gentoo-xcursors ships on every desktop edition (an extra cursor theme; Adwaita stays the default) and never on Server.
It rides as an RDEPEND of the desktop meta packages, so installed systems get it on update too. Reads the repo tree; Patronus' vendored common/ skips."""
import glob
import os
import re
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
GENTOO = os.path.join(REPO, "gentoo")
VAR = os.path.join(REPO, "build", "live-iso", "project", "variants")
HAVE = os.path.isdir(os.path.join(VAR, "gnome-light"))
ATOM = "x11-themes/gentoo-xcursors"


def newest(pattern):
    files = sorted(glob.glob(os.path.join(GENTOO, pattern)), key=lambda p: int((re.search(r"-r(\d+)\.ebuild$", p) or [0, 0])[1]))
    return files[-1]


def rdepend(path):
    return re.search(r'^RDEPEND="(.*?)"', open(path).read(), re.S | re.M).group(1).split()


@unittest.skipUnless(HAVE, "needs the Horcrux repo tree")
class DesktopCursors(unittest.TestCase):
    def test_gnome_meta_pulls_it(self):  # gnome-light, gnome (extends it), desktop, gringotts, hogshead, hogsmeade all install this meta
        self.assertIn(ATOM, rdepend(newest("gnome-extra/horcrux-gnome-defaults/*.ebuild")))

    def test_xfce_surface_meta_pulls_it(self):
        self.assertIn(ATOM, rdepend(newest("app-misc/horcrux-surface-desktop/*.ebuild")))

    def test_every_gnome_variant_installs_the_gnome_meta(self):
        for v in ("gnome-light", "desktop", "gringotts"):
            self.assertIn("horcrux-gnome-defaults", open(os.path.join(VAR, v, "Dockerfile")).read(), v)
        self.assertEqual(open(os.path.join(VAR, "gnome", "requires")).read().split(), ["gnome-light"])

    def test_server_and_core_do_not(self):
        self.assertNotIn(ATOM, " ".join(rdepend(newest("sys-apps/horcrux-core/*.ebuild"))))
        for f in glob.glob(os.path.join(VAR, "core", "**", "*"), recursive=True):
            if os.path.isfile(f):
                self.assertNotIn("gentoo-xcursors", open(f, errors="ignore").read(), f)

    def test_adwaita_stays_default(self):
        for f in glob.glob(os.path.join(GENTOO, "gnome-extra", "horcrux-gnome-defaults", "**", "*"), recursive=True) + glob.glob(os.path.join(GENTOO, "app-misc", "horcrux-surface-desktop", "**", "*"), recursive=True):
            if os.path.isfile(f):
                self.assertNotRegex(open(f, errors="ignore").read().replace("-r10: x11-themes/gentoo-xcursors", ""), r"(cursor-theme|XCURSOR_THEME|Inherits=).*gentoo", f)


if __name__ == "__main__":
    unittest.main()
