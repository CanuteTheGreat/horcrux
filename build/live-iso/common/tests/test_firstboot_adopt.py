"""horcrux-firstboot --adopt (run from the ebuild's pkg_postinst): an already-initialised host is marked done, a fresh one is not."""
import os, re, subprocess, tempfile, unittest
from pathlib import Path

PKG = Path(__file__).resolve().parents[4] / "gentoo/sys-apps/horcrux-firstboot"
SCRIPT = PKG / "files/horcrux-firstboot"


def adopt(state):
    env = dict(os.environ, FIRSTBOOT_STATE_DIR=str(state))
    return subprocess.run(["sh", str(SCRIPT), "--adopt"], env=env, capture_output=True, text=True, timeout=20)


class FirstbootAdopt(unittest.TestCase):
    def test_initialised_host_marked_done(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "horcrux.db").write_text("x")
            r = adopt(d)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertTrue((Path(d) / "firstboot-done").exists())

    def test_fresh_host_untouched(self):
        with tempfile.TemporaryDirectory() as d:
            r = adopt(d)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertFalse((Path(d) / "firstboot-done").exists())

    def test_existing_marker_kept(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "horcrux.db").write_text("x")
            (Path(d) / "firstboot-done").write_text("orig")
            adopt(d)
            self.assertEqual((Path(d) / "firstboot-done").read_text(), "orig")

    def test_ebuild_calls_adopt_and_skips_image_builds(self):
        e = next(PKG.glob("*.ebuild")).read_text()
        self.assertIn("pkg_postinst", e)
        self.assertIn("--adopt", e)
        self.assertRegex(e, r"ROOT.*return 0")

    def test_both_init_systems_skip_when_marker_exists(self):
        self.assertIn("firstboot-done", (PKG / "files/horcrux-firstboot.initd").read_text())
        self.assertIn("ConditionPathExists=!/var/lib/horcrux/firstboot-done", (PKG / "files/horcrux-firstboot.service").read_text())


if __name__ == "__main__":
    unittest.main()
