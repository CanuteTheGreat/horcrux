"""zfs-enc-test sh_until: a status query that answers empty/late under TCG is re-asked (bounded) until the caller's predicate holds; the caller's
assertion runs on the last answer, so a really broken guest still fails after `tries` asks."""
import importlib.util
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
spec = importlib.util.spec_from_file_location("zet2", os.path.join(HERE, "..", "zfs-enc-test.py"))
zet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(zet)
zet.qt.SLOW = 0.0          # no real sleeping


def harness(results):
    cls = [v for v in vars(zet).values() if isinstance(v, type) and hasattr(v, "sh_until")][0]
    h = cls.__new__(cls)
    h.calls = []
    q = list(results)

    def sh(cmd, timeout=None):
        h.calls.append(cmd)
        return q.pop(0) if q else (0, "")
    h.sh = sh
    return h


class ShUntil(unittest.TestCase):
    def test_empty_then_good_is_reasked(self):
        h = harness([(255, "timeout"), (0, ""), (0, "chronyd enabled")])
        self.assertEqual(h.sh_until("q", lambda o: "chronyd" in o, "t"), (0, "chronyd enabled"))
        self.assertEqual(len(h.calls), 3)

    def test_good_first_time_asks_once(self):
        h = harness([(0, "chronyd enabled")])
        h.sh_until("q", lambda o: "chronyd" in o, "t")
        self.assertEqual(len(h.calls), 1)

    def test_never_good_is_bounded_and_returns_last_answer(self):
        h = harness([])
        rc, o = h.sh_until("q", lambda o: "chronyd" in o, "t", tries=3)
        self.assertEqual(o, "")
        self.assertEqual(len(h.calls), 4)      # 3 asks + one diagnostic


if __name__ == "__main__":
    unittest.main()
