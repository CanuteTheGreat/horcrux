"""horcrux-core ships the default-editor tool + conf; they must stay byte-identical to the image overlay copies (build/live-iso/common/overlay)."""
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.abspath(os.path.join(HERE, ".."))
ROOT = os.path.abspath(os.path.join(PKG, "..", "..", ".."))
OVL = os.path.join(ROOT, "build", "live-iso", "common", "overlay")


class Mirror(unittest.TestCase):
    def test_tool_and_conf_identical(self):
        for name, ov in (("default-editor", "usr/sbin/default-editor"), ("default-editor.conf", "etc/default-editor.conf")):
            self.assertEqual(open(os.path.join(PKG, "files", name)).read(), open(os.path.join(OVL, ov)).read(), name)

    def test_ebuild_depends_on_vim_and_installs_both(self):
        e = [f for f in os.listdir(PKG) if f.endswith(".ebuild")]
        self.assertEqual(len(e), 1, e)
        s = open(os.path.join(PKG, e[0])).read()
        self.assertRegex(s, r"(?m)^\tapp-editors/vim$")
        self.assertNotIn("app-editors/nano", s)
        self.assertIn('dosbin "${FILESDIR}"/default-editor', s)
        self.assertIn("default-editor apply --if-unset", s)


if __name__ == "__main__":
    unittest.main()
