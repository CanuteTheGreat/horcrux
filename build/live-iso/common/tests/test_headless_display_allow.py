"""The headless-server gate (zfs-enc-test.py headless_checks) rejects display-stack packages but must tolerate header-only
build dependencies: the default world update runs --with-bdeps=y (horcrux-autoupdate -r4), which installs x11-base/xorg-proto
on a server; a run failed server-has-no-display-stack-installed on exactly that (2026-11-24, aarch64/x86 daily)."""
import ast
import os
import re
import unittest

SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "zfs-enc-test.py")


def allow():
    for l in open(SRC):
        m = re.match(r"\s*DISPLAY_ALLOW = (\(.*?\))", l)
        if m:
            return ast.literal_eval(m.group(1))
    raise AssertionError("DISPLAY_ALLOW not found")


class DisplayAllow(unittest.TestCase):
    def test_header_only_build_deps_allowed(self):
        self.assertIn("x11-base/xorg-proto", allow())

    def test_real_display_packages_still_rejected(self):
        for bad in ("x11-base/xorg-server", "x11-base/xwayland", "x11-libs/libX11", "gnome-base/gdm"):
            self.assertNotIn(bad, allow())


if __name__ == "__main__":
    unittest.main()
