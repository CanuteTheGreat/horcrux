"""zfs-enc-test scrub_wait: the post-resume scrub is polled until it FINISHED (a slow nested-KVM scrub is no failure), fails with a stall report when it
makes no progress, and the verdict still comes from the final `zpool status` output (run 4083: one blocking `zpool scrub -w` hit its 600 s cap)."""
import importlib.util
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
spec = importlib.util.spec_from_file_location("zet2", os.path.join(HERE, "..", "zfs-enc-test.py"))
zet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(zet)

RUN = "  scan: scrub in progress since Thu Nov 19 16:02:30 2026\n\t2.00G / 9.00G scanned, 1.00G / 9.00G issued at 50M/s\n\t0B repaired, 11.11% done\nconfig:"
RUN2 = RUN.replace("1.00G / 9.00G issued", "3.00G / 9.00G issued")
DONE = "  scan: scrub repaired 0B in 00:20:11 with 0 errors on Thu Nov 19 16:22:41 2026\nconfig:"


def harness(results):
    cls = [v for v in vars(zet).values() if isinstance(v, type) and hasattr(v, "scrub_wait")][0]
    h = cls.__new__(cls)
    h.res, h.calls = {}, []
    q = list(results)

    def sh(cmd, timeout=60):
        h.calls.append(cmd)
        return q.pop(0) if q else (0, DONE)
    h.sh = sh
    h.wait_ssh = lambda t=0: True
    zet.time.sleep = lambda s: None
    return h


class ScrubWait(unittest.TestCase):
    def test_progress_polled_until_finished(self):
        h = harness([(0, RUN), (0, RUN), (0, RUN2), (0, DONE), (0, "  scan: scrub repaired 0B with 0 errors\nerrors: No known data errors\n state: ONLINE")])
        rc, o = h.scrub_wait(None, "b1", poll=0)
        self.assertEqual(rc, 0)
        self.assertIn("with 0 errors", o)
        self.assertIn("No known data errors", o)

    def test_no_progress_fails_with_stall_report(self):
        t = [1000.0]
        zet.time.time = lambda: t.__setitem__(0, t[0] + 200) or t[0]
        h = harness([(0, RUN)] * 20 + [(0, "load 0.1 stuck-D-task")])
        rc, o = h.scrub_wait(None, "b1", idle=600, poll=0)
        self.assertEqual(rc, 1)
        self.assertIn("scrub stalled", o)
        self.assertEqual(len(h.res["scrub_stalled"]), 1)

    def test_progress_token_parse(self):
        self.assertEqual(zet.zfs_scrub_progress(RUN), ("2.00G", "1.00G"))
        self.assertIsNone(zet.zfs_scrub_progress(DONE))


if __name__ == "__main__":
    unittest.main()
