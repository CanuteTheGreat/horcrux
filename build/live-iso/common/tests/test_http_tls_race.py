"""qemu-test.http(): a 308 'only speaks HTTPS' from the plain-http fallback (the https attempt timed out on a slow
aarch64 TCG guest) is transient, never a web-login result (daily run 20726, 2026-11-19)."""
import importlib.util
import os
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("qemu_test_mod", os.path.join(HERE, "..", "qemu-test.py"))
qt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qt)


class HttpRetry(unittest.TestCase):
    def run_http(self, answers):
        it = iter(answers)
        with mock.patch.object(qt, "http_once", side_effect=lambda *a, **k: next(it)), mock.patch.object(qt.time, "sleep"):
            return qt.http(8006, "/api/auth/login", "POST", {})

    def test_redirect_from_http_fallback_is_retried(self):
        r = self.run_http([("http", 308, b"This server only speaks HTTPS"), ("https", 200, b'{"csrf_token":"x"}')])
        self.assertEqual(r[:2], ("https", 200))

    def test_real_answers_are_not_retried(self):
        self.assertEqual(self.run_http([("https", 401, b"no")])[:2], ("https", 401))

    def test_gives_up_after_tries(self):
        r = self.run_http([("http", 308, b"x")] * 4)
        self.assertEqual(r[:2], ("http", 308))


if __name__ == "__main__":
    unittest.main()
