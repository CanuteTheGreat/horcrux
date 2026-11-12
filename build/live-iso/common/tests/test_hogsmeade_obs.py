"""OBS is the owner's personal app on HOGSMEADE only: it lives in the hogsmeade variant's local repo (package app-misc/hogsmeade-obs = the layer's set + the
machine config it needs) and nowhere public. Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus."""
import os
import re
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
LIVE = REPO / "build" / "live-iso"
HSM = LIVE / "project" / "variants" / "hogsmeade"
EB = HSM / "local-repo" / "app-misc" / "hogsmeade-obs" / "hogsmeade-obs-1.ebuild"


@unittest.skipUnless(EB.parent.parent.parent.parent.is_dir() or (REPO / "gentoo").is_dir(), "Horcrux repo tree only")
class HogsmeadeObs(unittest.TestCase):
    def setUp(self):
        if not EB.is_file():
            self.skipTest("Horcrux repo tree only")
        self.t = EB.read_text()

    def test_package_is_the_set_and_owns_the_config(self):
        for dep in ("media-video/obs-studio[nvenc,screencast,v4l]", "media-video/ffmpeg[opus,x264,nvenc]", "media-plugins/obs-pipewire-audio-capture", "media-video/v4l2loopback"):
            self.assertIn(dep, self.t)
        for f in ("package.accept_keywords", "package.use", "/etc/modprobe.d", "/etc/modules-load.d"):
            self.assertIn(f, self.t)
        self.assertIn("devices=1 video_nr=10", self.t)
        self.assertIn("exclusive_caps=1", self.t)
        self.assertIn("media-video/v4l2loopback dist-kernel", self.t)
        self.assertRegex(self.t, r"media-plugins/obs-pipewire-audio-capture ~amd64")

    def test_image_layer_installs_it_and_resolves_the_keywords(self):
        for d in ("Dockerfile", "Dockerfile.part2"):
            self.assertIn("app-misc/hogsmeade-obs", (HSM / d).read_text(), d)
            self.assertIn("local/app-misc/hogsmeade-obs/*.ebuild", (HSM / d).read_text(), d + ": its Manifest")
        kw = (HSM / "portage" / "package.accept_keywords" / "hogsmeade").read_text()
        self.assertIn("media-video/obs-studio ~amd64", kw)
        self.assertIn("media-plugins/obs-pipewire-audio-capture ~amd64", kw)
        use = (HSM / "portage" / "package.use" / "zzz-hogsmeade").read_text()
        self.assertIn("media-video/ffmpeg opus x264 nvenc", use)

    def test_not_in_any_other_machine_or_public_layer(self):
        pat = re.compile(r"obs-studio|hogsmeade-obs|v4l2loopback")
        roots = [REPO / "gentoo", LIVE / "common", LIVE / "project"]
        bad = []
        for r in roots:
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_obs.py":
                    continue
                if HSM in p.parents:
                    continue
                try:
                    if pat.search(p.read_text()):
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "OBS belongs to the HOGSMEADE layer only")


NC = HSM / "local-repo" / "app-misc" / "hogsmeade-nextcloud" / "hogsmeade-nextcloud-1.ebuild"


@unittest.skipUnless(NC.is_file(), "Horcrux repo tree only")
class HogsmeadeNextcloud(unittest.TestCase):
    """The Nextcloud desktop client is the owner's personal app: package-owned in the HOGSMEADE layer only, never in a public image or a lab test machine."""

    def test_package_owns_the_client_and_image_installs_it(self):
        t = NC.read_text()
        self.assertIn("net-misc/nextcloud-client[nautilus]", t)
        self.assertIn("package.use", t)
        for d in ("Dockerfile", "Dockerfile.part2"):
            txt = (HSM / d).read_text()
            self.assertIn("app-misc/hogsmeade-nextcloud", txt, d)
            self.assertIn("local/app-misc/hogsmeade-nextcloud/*.ebuild", txt, d + ": its Manifest")
        self.assertIn("net-misc/nextcloud-client nautilus", (HSM / "portage" / "package.use" / "zzz-hogsmeade").read_text())

    def test_not_leaked_into_public_or_test_machine_layers(self):
        # the package name exists nowhere outside the hogsmeade variant (and this test)
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_obs.py" or HSM in p.parents:
                    continue
                try:
                    if "hogsmeade-nextcloud" in p.read_text():
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "hogsmeade-nextcloud belongs to the HOGSMEADE layer only")
        # public images and lab test machines forbid the client outright
        forb = [l.strip() for l in (LIVE / "project" / "public-forbidden.txt").read_text().splitlines() if l.strip() and not l.startswith("#")]
        self.assertIn("net-misc/nextcloud-client", forb)
        deny = REPO / "gentoo" / "app-misc" / "horcrux-test-machine" / "files" / "personal-apps.denylist"
        if deny.is_file():
            self.assertIn("net-misc/nextcloud*", deny.read_text())
        # no active (non-comment) shared package list or public Dockerfile installs it
        shared = [LIVE / "project" / "packages.txt", LIVE / "project" / "packages-late.txt", LIVE / "common" / "packages.txt",
                  LIVE / "project" / "variants" / "gnome-light" / "packages.txt"]
        for f in shared:
            if f.is_file():
                self.assertNotRegex(f.read_text(), r"(?m)^[^#\n]*nextcloud", str(f))


if __name__ == "__main__":
    unittest.main()
