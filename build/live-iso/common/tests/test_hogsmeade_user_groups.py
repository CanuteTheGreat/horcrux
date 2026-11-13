"""The owner's group memberships on HOGSMEADE are package-owned (app-misc/hogsmeade-user-groups in the hogsmeade variant's local repo): one list
file read by the installer's user creation and applied by pkg_postinst. Public images never carry it. Reads the Horcrux repo tree; skipped in the
vendored copy of common/ in Patronus."""
import os
import re
import sys
import tempfile
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
LIVE = REPO / "build" / "live-iso"
HSM = LIVE / "project" / "variants" / "hogsmeade"
PKG = HSM / "local-repo" / "app-misc" / "hogsmeade-user-groups"
EB = PKG / "hogsmeade-user-groups-1.ebuild"
sys.path.insert(0, str(LIVE / "common" / "overlay" / "usr" / "lib" / "live-installer"))

OWNER_NOW = {"lp", "wheel", "audio", "cdrom", "video", "usb", "input", "users", "kvm", "render", "lpadmin", "pipewire", "plugdev", "libvirt",
             "pcap", "pcscd"}
OWNER_ADDED = {"docker", "dialout", "uucp", "adm", "portage", "cdrw", "netdev", "seat", "comfyui", "qemu", "tss", "horcrux-admin"}
# groups with no acct-group package (created by the image / base-files)
NO_ACCT = {"cdrw", "wheel"}


def listed(text):
    m = re.search(r"newins - extra-groups\.list <<-'EOF2'\n(.*?)\n\tEOF2", text, re.S)
    assert m
    return [l.strip() for l in m.group(1).splitlines() if l.strip() and not l.strip().startswith("#")]


@unittest.skipUnless((REPO / "gentoo").is_dir(), "Horcrux repo tree only")
class HogsmeadeUserGroups(unittest.TestCase):
    def setUp(self):
        if not EB.is_file():
            self.skipTest("Horcrux repo tree only")
        self.t = EB.read_text()

    def test_group_list_is_complete(self):
        lines = listed(self.t)
        self.assertEqual(lines[0], "user canutethegreat")
        groups = lines[1:]
        self.assertEqual(len(groups), len(set(groups)), "duplicate group")
        self.assertEqual(set(groups), OWNER_NOW | OWNER_ADDED)

    def test_acct_group_deps_for_package_groups(self):
        deps = set(re.findall(r"acct-group/([a-z-]+)", self.t))
        for g in (OWNER_NOW | OWNER_ADDED) - NO_ACCT:
            # groups pulled in by his desktop/virt packages anyway are not repeated; the ones a hand usermod added must be explicit
            if g in OWNER_ADDED:
                self.assertIn(g, deps, g)
        self.assertIn("horcrux-admin", deps)
        self.assertTrue((REPO / "gentoo" / "acct-group" / "horcrux-admin").is_dir())

    def test_postinst_is_idempotent_and_guarded(self):
        post = self.t.split("pkg_postinst()", 1)[1]
        self.assertIn("id -nG", post)                  # skip groups he is already in
        self.assertIn("getent group", post)            # only groups that exist
        self.assertIn("getent passwd", post)           # no user yet (image build): not an error
        self.assertIn("usermod -aG", post)
        self.assertNotIn("usermod -G", post.replace("-aG", ""))  # never replaces the set

    def test_image_layer_installs_it(self):
        for d in ("Dockerfile", "Dockerfile.part2"):
            self.assertIn("app-misc/hogsmeade-user-groups", (HSM / d).read_text(), d)

    def test_installer_user_creation_uses_the_list(self):
        from liveinst import desktop
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / desktop.EXTRA_GROUPS.lstrip("/")
            f.parent.mkdir(parents=True)
            f.write_text("\n".join(listed(self.t)) + "\n")
            got = desktop.extra_groups(d, "canutethegreat")
            self.assertEqual(set(got), OWNER_NOW | OWNER_ADDED)
            self.assertEqual(desktop.extra_groups(d, "someoneelse"), [])
        with tempfile.TemporaryDirectory() as d:     # public image: no file, no extras
            self.assertEqual(desktop.extra_groups(d, "canutethegreat"), [])

    def test_leak_absent_from_public_images(self):
        self.assertIn("app-misc/hogsmeade-user-groups", (LIVE / "project" / "public-forbidden.txt").read_text().split())
        pat = re.compile(r"hogsmeade-user-groups|extra-groups\.list")
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_user_groups.py":
                    continue
                if HSM in p.parents or p.name in ("public-forbidden.txt", "personal-apps.denylist", "personal-paths.denylist", "desktop.py"):
                    continue
                try:
                    if pat.search(p.read_text()):
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [])
        # the installer hook is generic: no owner name in it
        self.assertNotIn("canutethegreat", (LIVE / "common/overlay/usr/lib/live-installer/liveinst/desktop.py").read_text())


if __name__ == "__main__":
    unittest.main()
