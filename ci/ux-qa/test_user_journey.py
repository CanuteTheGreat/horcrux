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


if __name__ == "__main__":
    unittest.main()
