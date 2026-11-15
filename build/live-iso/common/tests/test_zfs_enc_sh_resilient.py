"""zfs-enc-test sh_resilient: a transport drop (rc 255 'closed by remote host') mid long command after the hibernate resume is re-attached and the
command run again (the scrub assertion on its output is untouched); a real command failure (rc 1) or a non-transport 255 is never retried."""
import importlib.util
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
spec = importlib.util.spec_from_file_location("zet", os.path.join(HERE, "..", "zfs-enc-test.py"))
zet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(zet)


def harness(results):
    cls = [v for v in vars(zet).values() if isinstance(v, type) and hasattr(v, "sh_resilient")][0]
    h = cls.__new__(cls)
    h.res = {}
    h.calls = []
    q = list(results)

    def sh(cmd, timeout=60):
        h.calls.append(cmd)
        return q.pop(0) if q else (0, "BOOT=b1")
    h.sh = sh
    h.wait_ssh = lambda t=0: True
    return h


class ShResilient(unittest.TestCase):
    def test_drop_then_success_retries_and_records_diag(self):
        h = harness([(255, "Connection to 127.0.0.1 closed by remote host.\n"), (0, "BOOT=b1\nup"), (0, "with 0 errors")])
        self.assertEqual(h.sh_resilient("scrub", boot_id="b1"), (0, "with 0 errors"))
        self.assertEqual(len(h.res["ssh_transport_loss"]), 1)
        self.assertIn("same boot id=True", h.res["ssh_transport_loss"][0])

    def test_command_failure_not_retried(self):
        h = harness([(1, "cannot scrub")])
        self.assertEqual(h.sh_resilient("scrub"), (1, "cannot scrub"))
        self.assertEqual(len(h.calls), 1)

    def test_timeout_255_not_retried(self):
        h = harness([(255, "timeout")])
        self.assertEqual(h.sh_resilient("scrub"), (255, "timeout"))
        self.assertEqual(len(h.calls), 1)

    def test_gives_up_after_three_drops(self):
        d = (255, "closed by remote host")
        h = harness([d, (0, "BOOT=x"), d, (0, "BOOT=x"), d])
        self.assertEqual(h.sh_resilient("scrub")[0], 255)
        self.assertEqual(len(h.res["ssh_transport_loss"]), 2)


if __name__ == "__main__":
    unittest.main()
