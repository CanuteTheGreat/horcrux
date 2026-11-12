"""Chromium/Electron 43+ StatusNotifierItems (Slack, Signal, Docker Desktop) must survive the GNOME AppIndicator extension.

They answer Properties.Get/GetAll only on the item's well-known name and fail on the unique bus name ("error occurred in Get"); ::gentoo's
gnome-shell-extension-appindicator 64/66 destroys such an item right after registration, so the tray icon flashes "..." and vanishes.
Horcrux ships gnome-extra/gnome-shell-extension-appindicator-66-r1 (patched) and horcrux-gnome-defaults depends on it.

Reads the Horcrux repo tree (skipped in the vendored copy of common/ in Patronus). The headless gnome-shell round trip skips without gnome-shell.
"""
import glob
import os
import re
import subprocess
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.dirname(HERE)
REPO = os.path.abspath(os.path.join(COMMON, "..", "..", ".."))
PKG = os.path.join(REPO, "gentoo", "gnome-extra", "gnome-shell-extension-appindicator")
DEFAULTS = sorted(glob.glob(os.path.join(REPO, "gentoo", "gnome-extra", "horcrux-gnome-defaults", "*.ebuild")))
EBUILD = os.path.join(PKG, "gnome-shell-extension-appindicator-66-r1.ebuild")
PATCH = os.path.join(PKG, "files", "gnome-shell-extension-appindicator-66-chromium-sni.patch")


@unittest.skipUnless(os.path.exists(EBUILD), "Horcrux repo tree only")
class PatchedPackage(unittest.TestCase):
    def test_desktop_defaults_require_the_patched_revision(self):
        self.assertRegex(open(DEFAULTS[-1]).read(), r"(?m)^\t>=gnome-extra/gnome-shell-extension-appindicator-66-r1$")

    def test_ebuild_applies_the_patch_and_is_selectable(self):
        s = open(EBUILD).read()
        self.assertIn('PATCHES=( "${FILESDIR}/${PN}-${MY_PV}-chromium-sni.patch" )', s)
        self.assertRegex(s, r'(?m)^KEYWORDS="amd64 arm64"$')   # stable keywords: beats ::gentoo's 64 (amd64) and 66 (~amd64) with no accept_keywords
        self.assertTrue(os.path.exists(PATCH))

    def test_patch_covers_all_unique_name_paths(self):
        p = open(PATCH).read()
        self.assertIn("DO_NOT_LOAD_PROPERTIES", p)                 # GDBusProxy's own GetAll goes to the unique name
        self.assertIn("this._proxy.gName, this._proxy.gObjectPath", p)   # Introspect on the well-known name
        self.assertIn("BUS_SNI_NAME_REGEX", p)                      # startup brute-force scan registers under the well-known name

    def test_manifest_has_the_tarball(self):
        self.assertIn("gnome-shell-extension-appindicator-66.tar.gz", open(os.path.join(PKG, "Manifest")).read())

    def test_qemu_gate_runs_the_chromium_fake(self):
        s = open(os.path.join(COMMON, "zfs-enc-test.py")).read()
        self.assertIn("chromium-style-status-notifier-item-is-kept-by-the-appindicator-extension", s)
        self.assertTrue(os.path.exists(os.path.join(COMMON, "sni_chromium_fake.py")))


class FakeItem(unittest.TestCase):
    def test_fake_behaves_like_chromium(self):
        s = open(os.path.join(COMMON, "sni_chromium_fake.py")).read()
        self.assertIn("error occurred in Get", s)
        self.assertRegex(s, r"startswith\(\":\"\)")     # unique-name calls fail


@unittest.skipUnless(subprocess.call("command -v gnome-shell dbus-run-session >/dev/null", shell=True) == 0, "needs gnome-shell")
class HeadlessShell(unittest.TestCase):
    """Throwaway headless gnome-shell in a private session bus with the INSTALLED extension (what the image ships)."""

    def run_mode(self, mode):
        r = subprocess.run([os.path.join(HERE, "sni_headless_gate.sh"), "", mode], capture_output=True, text=True, timeout=180)
        if r.returncode == 77:
            self.skipTest("headless shell unavailable here")
        self.assertEqual(r.returncode, 0, r.stdout[-1500:] + r.stderr[-500:])

    def test_item_registered_after_shell_start_is_kept(self):
        self.run_mode("late")

    def test_item_already_running_at_shell_start_is_kept(self):
        self.run_mode("early")


if __name__ == "__main__":
    unittest.main()
