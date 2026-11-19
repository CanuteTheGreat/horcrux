"""VM built with HOLD_ON_RESET keeps QEMU paused after a guest reset; alive() reads the registers (HMP) once and ends QEMU, so a KVM triple fault /
CF9 reset / clean reboot (all 'QEMU exit 0') can be told apart by RIP (zfs-enc hibernate resume, runs 3963/3964/4033)."""
import importlib.util
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
spec = importlib.util.spec_from_file_location("qt_hold", os.path.join(HERE, "..", "qemu-test.py"))
qt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qt)


class FakeProc:
    def __init__(self):
        self.rc = None

    def poll(self):
        return self.rc

    def kill(self):
        self.rc = -9

    def wait(self, timeout=None):
        return self.rc


def vm(status, tmp):
    v = qt.VM.__new__(qt.VM)
    v.p, v.hold, v.reset_state, v.out, v.name = FakeProc(), True, "", tmp, "t"

    def qmp_ret(cmd, **kw):
        if cmd == "query-status":
            return {"return": {"status": status}}
        return {"return": {"info registers -a": "CPU#0\nRIP=ffffffff81234567 RFL=00000002\nXMM00=0 0\nCR0=80050033", "x/8i $rip": "0xffffffff81234567: hlt", "info cpus": "* CPU #0"}[kw["command-line"]]}
    v.qmp_ret = qmp_ret
    return v


class Hold(unittest.TestCase):
    def test_running_guest_is_alive(self):
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            v = vm("running", d)
            self.assertTrue(v.alive())
            self.assertEqual(v.reset_state, "")

    def test_reset_guest_is_captured_and_ended(self):
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            v = vm("shutdown", d)
            self.assertFalse(v.alive())
            self.assertIn("RIP=ffffffff81234567", v.reset_state)
            self.assertNotIn("XMM00", v.reset_state)
            self.assertEqual(v.p.rc, -9)
            self.assertTrue(os.path.exists(f"{d}/t-reset-state.txt"))


if __name__ == "__main__":
    unittest.main()
