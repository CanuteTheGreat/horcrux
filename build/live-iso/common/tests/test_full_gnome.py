"""Owner rule (2026-11-12): his non-server machines run FULL GNOME (variant `gnome`), the public edition stays gnome-light (small), and the
Surface/madammalkins image runs XFCE and never gets GNOME. These read the repo tree; Patronus' vendored copy of common/ has none of it, so they skip there."""
import os
import re
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
LIVE = os.path.join(REPO, "build", "live-iso")
VAR = os.path.join(LIVE, "project", "variants")
GENTOO = os.path.join(REPO, "gentoo")
HAVE = os.path.isdir(os.path.join(VAR, "gnome"))


def read(*p):
    return open(os.path.join(*p)).read()


def atoms(path):
    return [l.split("#")[0].split()[0] for l in read(path).splitlines() if l.split("#")[0].strip()]


def code(path):
    return "\n".join(l for l in read(path).splitlines() if not l.lstrip().startswith("#"))


def rdepend(path):
    m = re.search(r'^RDEPEND="(.*?)"', read(path), re.S | re.M)
    return [t for t in m.group(1).split() if not t.startswith("#")]


@unittest.skipUnless(HAVE, "needs the Horcrux repo tree")
class FullGnome(unittest.TestCase):
    EXTRAS = os.path.join(GENTOO, "gnome-extra", "horcrux-gnome-extras", "horcrux-gnome-extras-1.ebuild")

    def test_gnome_edition_is_full_and_sits_on_gnome_light(self):
        self.assertEqual(atoms(os.path.join(VAR, "gnome", "packages.txt")), ["gnome-base/gnome", "gnome-extra/horcrux-gnome-extras"])
        self.assertEqual(read(VAR, "gnome", "requires").split(), ["gnome-light"])
        self.assertIn("gnome-base/gnome -extras", read(VAR, "gnome", "portage", "package.use", "horcrux-gnome"))

    def test_public_gnome_light_stays_light(self):
        light = atoms(os.path.join(VAR, "gnome-light", "packages.txt"))
        for a in ("gnome-base/gnome", "gnome-extra/horcrux-gnome-extras", "gnome-base/gnome-core-apps", "gnome-extra/gnome-extra-apps"):
            self.assertNotIn(a, light)
        self.assertIn("gnome-base/gnome-light", light)
        self.assertFalse(os.path.exists(os.path.join(VAR, "gnome-light", "requires")))
        self.assertNotIn("gnome-base/gnome ", read(VAR, "gnome-light", "portage", "package.use", "desktop").replace("gnome-base/gnome-", "x-"))
        sh = read(LIVE, "common", "image-name.sh")           # both editions are known desktop environments with their own ISO names
        self.assertIn("gnome-light gnome", sh)

    def test_owner_layers_pull_the_full_gnome_meta(self):
        self.assertIn("gnome-base/gnome -extras", read(VAR, "owner-common", "portage", "package.use", "owner-apps"))
        # the public-edition atoms are NOT in owner-common/packages.txt (the test-machine denylist covers that file); each layer installs gnome/packages.txt
        common = atoms(os.path.join(VAR, "owner-common", "packages.txt"))
        self.assertNotIn("gnome-base/gnome", common)
        for layer, listing in (("gringotts/Dockerfile", "/tmp/packages-gringotts.txt /tmp/packages-owner.txt /tmp/packages-gnome.txt"),
                               ("hogshead/Dockerfile", "/tmp/packages-owner.txt /tmp/packages-gnome.txt"),
                               ("hogshead/Dockerfile.part2", "/tmp/packages-owner.txt /tmp/packages-gnome.txt"),
                               ("hogsmeade/Dockerfile", "/tmp/packages-desktop.txt /tmp/packages-gnome.txt")):
            df = read(VAR, layer)
            self.assertIn("COPY project/variants/gnome/packages.txt /tmp/packages-gnome.txt", df, layer)
            self.assertIn(listing, df, layer)
        self.assertIn("variants/gnome/portage/package.use/horcrux-gnome", read(VAR, "hogsmeade", "Dockerfile"))
        # none of them still builds on the light set alone
        for layer in ("gringotts", "hogshead", "hogsmeade"):
            self.assertIn("gnome", read(VAR, layer, "Dockerfile"))

    def test_test_machine_allowlist_accepts_the_full_gnome_edition(self):
        allow = read(GENTOO, "app-misc", "horcrux-test-machine", "files", "world.allowlist").splitlines()
        self.assertIn("gnome-base/gnome", allow)
        self.assertIn("gnome-extra/horcrux-*", allow)

    def test_extras_leave_out_epiphany_and_systemd_only_apps(self):
        dep = "\n".join(rdepend(self.EXTRAS))
        for a in ("www-client/epiphany", "gnome-extra/gnome-logs", "gnome-extra/gnome-software", "mail-client/evolution"):
            self.assertNotIn(a, dep)
        for a in ("gnome-extra/gnome-calendar", "gnome-extra/gnome-weather", "gnome-extra/gnome-clocks", "sci-geosciences/gnome-maps", "media-sound/gnome-music",
                  "media-gfx/shotwell", "sys-apps/baobab", "net-misc/gnome-connections"):
            self.assertIn(a, dep)
        self.assertNotIn("brave", read(self.EXTRAS).lower().replace("# ", "").split("rdepend")[1])

    def test_full_gnome_os_release_is_gnome(self):
        # gnome-light and gnome both render VARIANT "GNOME" (the edition label users see); the gnome layer's build gate checks VARIANT_ID
        self.assertIn("VARIANT_ID=gnome", read(VAR, "gnome", "Dockerfile"))
        self.assertIn("gnome-base/gnome", read(VAR, "gnome", "Dockerfile"))


@unittest.skipUnless(HAVE, "needs the Horcrux repo tree")
class MadammalkinsHasNoGnome(unittest.TestCase):
    """madammalkins (Surface Pro) runs XFCE + LightDM: its image is variants/surface-kernel + variants/surface (+ horcrux-surface-desktop). None of it may
    pull gnome-base/gnome (or gnome-light, or the owner-common list that now carries it), and its os-release edition is XFCE."""
    SURFACE = os.path.join(GENTOO, "app-misc", "horcrux-surface-desktop")
    GNOME_META = re.compile(r"gnome-base/(gnome|gnome-light|gnome-core-apps|gnome-core-libs|gnome-shell|gdm)(?![\w-])|horcrux-gnome|gnome-extra/gnome-extra-apps")

    def files(self):
        out = []
        for d in (os.path.join(VAR, "surface"), os.path.join(VAR, "surface-kernel"), self.SURFACE):
            for root, _, fs in os.walk(d):
                for f in fs:
                    if f.endswith((".ebuild", ".txt", "Dockerfile", ".sh")) or "package." in root or f.startswith("package"):
                        out.append(os.path.join(root, f))
        return out

    def test_surface_image_never_names_a_gnome_meta(self):
        fs = self.files()
        self.assertTrue(fs)
        for f in fs:
            for line in code(f).splitlines():
                self.assertIsNone(self.GNOME_META.search(line), f"{f}: {line.strip()}")

    def test_surface_layers_do_not_use_owner_common_or_gnome_variants(self):
        for f in (os.path.join(VAR, "surface", "Dockerfile"), os.path.join(VAR, "surface-kernel", "Dockerfile")):
            c = code(f)
            self.assertNotIn("owner-common", c, f)
            self.assertNotIn("variants/gnome", c, f)
        for f in ("de", "requires"):
            for v in ("surface", "surface-kernel"):
                self.assertFalse(os.path.exists(os.path.join(VAR, v, f)), f"{v}/{f}")

    def test_surface_build_has_no_desktop_edition_in_its_chain(self):
        sh = read(LIVE, "build.sh")
        self.assertIn('*" surface-kernel "*|*" surface "*) NO_CORE=${NO_CORE:-1}', sh)

    def test_madammalkins_os_release_edition_is_xfce_not_gnome(self):
        self.assertEqual(read(self.SURFACE, "files", "root", "etc", "portage", "package.use", "horcrux-release-edition").split("\n", 1)[1].split(), ["sys-apps/horcrux-release", "xfce"])
        self.assertNotIn("gnome", read(self.SURFACE, "files", "root", "etc", "portage", "package.use", "horcrux-release-edition").split("\n", 1)[1])


if __name__ == "__main__":
    unittest.main()
