"""The installer shows the web UI address without a port when it is the standard one (owner: stick to standard ports)."""
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import system as S  # noqa: E402


class WebUrl(unittest.TestCase):
    def test_standard_https_has_no_port(self):
        self.assertEqual(S.web_url({"web_scheme": "https", "web_port": 443}, "10.0.0.5"), "https://10.0.0.5/")

    def test_default_port_is_443(self):
        self.assertEqual(S.web_url({}, "h"), "https://h/")

    def test_explicit_non_standard_port_is_shown(self):
        self.assertEqual(S.web_url({"web_scheme": "https", "web_port": 8443}, "h"), "https://h:8443/")

    def test_http_standard_port(self):
        self.assertEqual(S.web_url({"web_scheme": "http", "web_port": 80}, "h"), "http://h/")

    def test_profile_default_is_standard(self):
        p = S.normalise_profile({"name": "x", "title": "X"}) if hasattr(S, "normalise_profile") else None
        if p is not None:
            self.assertEqual(p["web_port"], 443)


if __name__ == "__main__":
    unittest.main()
