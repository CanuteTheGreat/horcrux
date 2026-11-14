"""ci-run.sh zfs_enc_run: an externally SIGKILLed zfs-enc-test run (rc 137, no [FAIL]) is run once more; assertion failures (rc 1) and
deadline hits (rc 124) are never retried (gnome-light run 2726 lost a healthy hibernate test to a kill from outside the harness)."""
import os
import re
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = open(os.path.join(HERE, "..", "ci-run.sh")).read()


def helper():
    m = re.search(r"^  zfs_enc_run\(\) \{\n.*?^  \}\n", SRC, re.S | re.M)
    assert m, "zfs_enc_run missing from ci-run.sh"
    return m.group(0)


def run(rcs):
    """run zfs_enc_run with a with_deadline stub that exits with each rc in turn; returns (final rc, attempts)"""
    with tempfile.TemporaryDirectory() as d:
        counter = os.path.join(d, "n")
        script = (
            "set -u\nHERE=/x; ISO=/y; EVI=%s; DL_ZFS=1\n"
            "with_deadline() { n=$(cat %s 2>/dev/null || echo 0); echo $((n+1)) > %s; rcs=(%s); echo out; return ${rcs[$n]:-0}; }\n"
            "pkill() { :; }\nsleep() { :; }\n%s\nzfs_enc_run t1 --desktop; echo FINAL=$rc\n"
            % (d, counter, counter, " ".join(map(str, rcs)), helper())
        )
        out = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=60).stdout
        return int(re.search(r"FINAL=(\d+)", out).group(1)), int(open(counter).read().strip())


class ZfsEncRetry(unittest.TestCase):
    def test_sigkill_is_retried_once_and_can_pass(self):
        self.assertEqual(run([137, 0]), (0, 2))

    def test_sigkill_twice_still_fails(self):
        self.assertEqual(run([137, 137]), (137, 2))

    def test_assertion_failure_not_retried(self):
        self.assertEqual(run([1]), (1, 1))

    def test_deadline_not_retried(self):
        self.assertEqual(run([124]), (124, 1))

    def test_pass_runs_once(self):
        self.assertEqual(run([0]), (0, 1))


if __name__ == "__main__":
    unittest.main()
