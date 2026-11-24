import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_ascii_icons as c  # noqa: E402


class AsciiIcons(unittest.TestCase):
    def test_catches_and_passes(self):
        with tempfile.TemporaryDirectory() as d:
            open(os.path.join(d, "a.rs"), "w").write('x { "[OK] Yes" }\n')
            self.assertEqual(len(c.scan(d)), 1)
            open(os.path.join(d, "a.rs"), "w").write('x { "✓ Yes" }\n')
            self.assertEqual(c.scan(d), [])

    def test_ui_tree_is_clean(self):
        root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "horcrux-api", "horcrux-ui", "src")
        self.assertEqual(c.scan(root), [])


if __name__ == "__main__":
    unittest.main()
