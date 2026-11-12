"""HOGSMEADE's three Backblaze B2 buckets are geesefs mounts under /mnt, owned by the HOGSMEADE-only package app-misc/hogsmeade-b2-mounts (hogsmeade
variant local repo). Keys are never packaged or committed (root-only /root/.aws/credentials, 0600); the package must not leak into public images.
Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus."""
import os
import re
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
LIVE = REPO / "build" / "live-iso"
HSM = LIVE / "project" / "variants" / "hogsmeade"
PKG = HSM / "local-repo" / "app-misc" / "hogsmeade-b2-mounts"
EB = PKG / "hogsmeade-b2-mounts-1.ebuild"
BUCKETS = ("ctg-storage", "ctg-media", "ctg-hacktheplanet")


@unittest.skipUnless((REPO / "gentoo").is_dir(), "Horcrux repo tree only")
class HogsmeadeB2Mounts(unittest.TestCase):
    def setUp(self):
        if not EB.is_file():
            self.skipTest("Horcrux repo tree only")
        self.t = EB.read_text()
        self.initd = (PKG / "files" / "hogsmeade-b2-mount.initd").read_text()
        self.wrap = (PKG / "files" / "hogsmeade-b2-mount").read_text()

    def test_three_buckets_one_instance_each_under_mnt(self):
        m = re.search(r"BUCKETS=\(([^)]*)\)", self.t)
        self.assertIsNotNone(m)
        self.assertEqual(tuple(m.group(1).split()), BUCKETS)
        self.assertIn('dosym hogsmeade-b2-mount "/etc/init.d/hogsmeade-b2-mount.${b}"', self.t)
        self.assertIn('/mnt/${bucket}', self.initd)
        self.assertNotIn("/home", self.initd + self.wrap + self.t.split("src_install", 1)[1])
        self.assertNotIn("$HOME/", self.wrap)

    def test_owner_uid_allow_other_and_crystalcave_options(self):
        self.assertIn("hb2_uid:=1000", self.initd)
        self.assertIn("hb2_gid:=1000", self.initd)
        self.assertIn("-o allow_other", self.wrap)
        for o in ("hb2_memory_limit:=1000", "hb2_read_ahead_large:=204800", "hb2_stat_cache_ttl:=10m"):
            self.assertIn(o, self.initd)
        self.assertIn("--file-mode", self.wrap)
        self.assertIn("--dir-mode", self.wrap)

    def test_low_priority(self):
        self.assertRegex(self.wrap, r"exec nice -n .* ionice -c .* /usr/bin/geesefs -f")
        self.assertRegex(self.initd, r"hb2_nice:=\d+")

    def test_boot_never_blocks_on_the_network_and_retries_forever(self):
        dep = self.initd.split("depend()", 1)[1].split("}", 1)[0]
        self.assertNotRegex(dep, r"\bneed\b.*\b(net|NetworkManager)\b")
        self.assertIn("use net", dep)
        self.assertIn("after net", dep)
        self.assertIn("supervisor=supervise-daemon", self.initd)
        self.assertIn("respawn_max=0", self.initd)
        self.assertRegex(self.initd, r"respawn_delay=")
        # a dead FUSE mount left by a killed geesefs would fail every respawn
        self.assertIn("fusermount -uz", self.wrap)

    def test_no_keys_in_the_package_or_variant(self):
        key = re.compile(r"\bK00[0-9A-Za-z+/]{28,}|aws_secret_access_key\s*=\s*\S{20,}|applicationKey\"?\s*[:=]\s*\"?[0-9A-Za-z+/]{20,}")
        bad = [str(p.relative_to(REPO)) for p in HSM.rglob("*") if p.is_file() and not p.name.endswith((".pyc",)) and _grep(p, key)]
        self.assertEqual(bad, [], "B2 keys must never be in git or images")
        self.assertIn("/root/.aws/credentials", self.initd)
        self.assertRegex(self.t, r"Credentials are not packaged")
        # nothing in the ebuild installs a credentials file
        self.assertNotRegex(self.t, r"credentials\"?\s*<<|newins .*credentials|doins .*credentials")

    def test_gnome_files_shows_the_mounts(self):
        # gvfs needs x-gvfs-show fstab entries (noauto: display hints only, the services mount) for mounts outside /media and $HOME
        self.assertIn("x-gvfs-show", self.t)
        self.assertIn("noauto", self.t)
        self.assertIn("pkg_postrm", self.t)
        self.assertIn("fuse.geesefs", self.t)

    def test_image_layer_installs_and_enables_it(self):
        for d in ("Dockerfile", "Dockerfile.part2"):
            self.assertIn("app-misc/hogsmeade-b2-mounts", (HSM / d).read_text(), d)
        p2 = (HSM / "Dockerfile.part2").read_text()
        for b in BUCKETS:
            self.assertIn("hogsmeade-b2-mount." + b, p2)
        self.assertIn("net-fs/geesefs ~amd64", (HSM / "portage" / "package.accept_keywords" / "hogsmeade").read_text())
        cfg = "hogsmeade-" + "apps-config"  # the package that owns the live host's keyword files (named so test_hogsmeade_obs' leak scan stays happy)
        self.assertTrue(any("net-fs/geesefs ~amd64" in e.read_text() for e in (HSM / "local-repo" / "app-misc" / cfg).glob("*.ebuild")))

    def test_leak_absent_from_public_images(self):
        self.assertIn("app-misc/hogsmeade-b2-mounts", (LIVE / "project" / "public-forbidden.txt").read_text().split())
        self.assertIn("app-misc/hogsmeade-b2-mounts*", (REPO / "gentoo" / "app-misc" / "horcrux-test-machine" / "files" / "personal-apps.denylist").read_text().split())
        pat = re.compile(r"hogsmeade-b2-mount")
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_b2_mounts.py":
                    continue
                if HSM in p.parents or p.name in ("public-forbidden.txt", "personal-apps.denylist", "personal-paths.denylist"):
                    continue
                if _grep(p, pat):
                    bad.append(str(p.relative_to(REPO)))
        self.assertEqual(bad, [], "the B2 mounts belong to the HOGSMEADE layer only")


def _grep(p, pat):
    try:
        return bool(pat.search(p.read_text()))
    except (UnicodeDecodeError, OSError):
        return False


if __name__ == "__main__":
    unittest.main()
