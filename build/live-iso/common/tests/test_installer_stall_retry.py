"""Regression (horcrux live-iso run 11851, gnome-light qemu-zfs-enc-desktop-hibernate): the unattended install guest went silent at
41 % of the unsquashfs copy and the harness waited out its whole 1700 s budget (every other run: 110-270 s). A guest that stops
sending serial output is installed once more on a fresh disk; everything else (FAILED, a guest that keeps printing, a second stall)
still fails as before."""
import importlib.util
import os
import re
import threading
import time
import types
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.dirname(HERE)


def load_harness():
    spec = importlib.util.spec_from_file_location("hx_qemu_test_stall", os.path.join(COMMON, "qemu-test.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


class FakeVM:
    def __init__(self, raw=b"", silent_for=0):
        self.raw = raw
        self.lock = threading.Lock()
        self.p = types.SimpleNamespace(poll=lambda: None)
        self.last_rx = time.time() - silent_for

    def expect_raw(self, pat, timeout, consume=True):
        time.sleep(0.3)
        return re.search(pat.encode(), self.raw)


class StallRetry(unittest.TestCase):
    def setUp(self):
        self.q = load_harness()
        self.q.INSTALL_STALL_S = 1
        self.r = self.q.Runner.__new__(self.q.Runner)

    def test_silent_guest_raises_stall(self):
        with self.assertRaises(self.q.InstallStall):
            self.r.wait_installer(FakeVM(b"AUTOINSTALL-PROGRESS 40% x\n", silent_for=5), 60, True)

    def test_silent_guest_without_retry_waits_out_the_budget(self):
        t = time.time()
        self.assertIsNone(self.r.wait_installer(FakeVM(silent_for=5), 1, False))
        self.assertLess(time.time() - t, 10)

    def test_chatty_guest_is_not_a_stall(self):
        self.q.INSTALL_STALL_S = 100
        self.assertIsNone(self.r.wait_installer(FakeVM(silent_for=0), 1, True))

    def test_finished_and_failed_are_returned(self):
        self.assertIn(b"AUTOINSTALL-OK", self.r.wait_installer(FakeVM(b"x AUTOINSTALL-OK\n", silent_for=5), 60, True).group(0))
        self.assertIn(b"FAILED", self.r.wait_installer(FakeVM(b"AUTOINSTALL-FAILED no disk\n", silent_for=5), 60, True).group(0))

    def test_phase1_retries_once_then_fails_on_a_second_stall(self):
        calls = []
        a = types.SimpleNamespace(flow="auto", out=os.environ.get("TMPDIR", "/tmp"), fs="zfs", zfs_raid="single", disk_gb=1)
        self.r.a = a
        self.r.disk = self.r.disk2 = os.path.join(a.out, "stall-test-disk.qcow2")
        self.q.subprocess = types.SimpleNamespace(check_call=lambda *x, **k: calls.append("recreate"))

        def once(nvars, retry):
            calls.append(("once", retry))
            raise self.q.InstallStall("quiet")
        self.r._phase1_once = once
        with self.assertRaises(self.q.InstallStall):
            self.r.phase1(None)
        self.assertEqual(calls, [("once", True), "recreate", ("once", False)])


if __name__ == "__main__":
    unittest.main()
