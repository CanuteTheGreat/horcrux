"""Self-tests for user_journey.py that need no browser/server: the report logic and the shipped baseline."""
import json
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import user_journey as uj  # noqa: E402


class Journey(unittest.TestCase):
    def test_expect_records_failures_only(self):
        uj.findings.clear()
        self.assertTrue(uj.expect("j9", "ok", True, "fine"))
        self.assertFalse(uj.expect("j9", "bad", False, "broken"))
        self.assertEqual([f["id"] for f in uj.findings], ["journey:j9:bad"])
        uj.findings.clear()

    def test_baseline_ids_are_real_checks(self):
        src = open(os.path.join(HERE, "user_journey.py")).read()
        base = json.load(open(os.path.join(HERE, "user-journey-baseline.json")))
        for k in base:
            if k.startswith("_"):
                continue
            _, step, check = k.split(":")
            self.assertIn(f'"{step}"', src, k)
            self.assertIn(f'"{check}"', src, k)

    def test_every_baseline_entry_says_who_fixes_it(self):
        base = json.load(open(os.path.join(HERE, "user-journey-baseline.json")))
        for k, v in base.items():
            if not k.startswith("_"):
                self.assertTrue(len(v) > 20, f"{k}: say what is wrong and where it is tracked")

    def test_vm_lookup_between_steps_survives_api_errors(self):
        # a failed login between steps used to end the run with a traceback and no report (the review lane saw nothing)
        real = uj.api

        def boom(*a, **k):
            raise OSError("HTTP Error 401: Unauthorized")
        uj.api = boom
        try:
            self.assertIsNone(uj.vm_by_name("x"))
        finally:
            uj.api = real

    def test_page_checks_are_js_functions(self):
        for js in (uj.REDUNDANT_EMPTY_JS, uj.H_OVERFLOW_JS):
            self.assertTrue(js.lstrip().startswith("() =>"), js[:40])
        self.assertIn(r"/\babove\b/i", uj.REDUNDANT_EMPTY_JS)


if __name__ == "__main__":
    unittest.main()
