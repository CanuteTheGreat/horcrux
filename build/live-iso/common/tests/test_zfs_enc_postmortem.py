"""zfs-enc-test resume_postmortem: after a guest vanished post-resume, the disk is booted once more and the guest's own log is printed;
diagnostics only (an exception inside never escapes and never changes a verdict)."""
import importlib.util
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
spec = importlib.util.spec_from_file_location("zet", os.path.join(HERE, "..", "zfs-enc-test.py"))
zet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(zet)
CLS = [v for v in vars(zet).values() if isinstance(v, type) and hasattr(v, "resume_postmortem")][0]


class PostMortem(unittest.TestCase):
    def test_command_reads_guest_logs_of_the_previous_boot(self):
        c = CLS.POSTMORTEM_CMD
        for need in ("/var/log/messages", "journalctl -b -1", "pstore", "last -x", "zpool history"):
            self.assertIn(need, c)

    def test_failure_inside_is_swallowed(self):
        h = CLS.__new__(CLS)
        h.res = {}
        h.z = type("Z", (), {"out": "/nonexistent"})()
        zet.time.sleep = lambda *_: None
        zet.qt.pflash_vars = lambda *_: (_ for _ in ()).throw(RuntimeError("boom"))
        h.resume_postmortem("vars")          # must not raise
        self.assertNotIn("resume_postmortem", h.res)


if __name__ == "__main__":
    unittest.main()
