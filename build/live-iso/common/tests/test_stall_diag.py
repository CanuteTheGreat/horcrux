"""Factory-reset 'timeout' failures (lab j01472/j04438, daily 12986) carried no cause: web and ssh both silent. stall_diag() must
report QEMU liveness, host load and the serial tail, and never raise."""
import importlib.util
import os
import tempfile
import types
import unittest

COMMON = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load():
    sp = importlib.util.spec_from_file_location("hx_qt_stalldiag", os.path.join(COMMON, "qemu-test.py"))
    m = importlib.util.module_from_spec(sp)
    sp.loader.exec_module(m)
    return m


class StallDiag(unittest.TestCase):
    def vm(self, rc=None, text=b"login: \nzfs: importing pool\n"):
        f = tempfile.NamedTemporaryFile(delete=False)
        f.write(text)
        f.flush()
        self.addCleanup(os.unlink, f.name)
        return types.SimpleNamespace(p=types.SimpleNamespace(poll=lambda: rc), log=f)

    def test_ssh_dead_includes_serial_tail_and_load(self):
        d = load().stall_diag(self.vm(), 255, "timeout")
        self.assertIn("qemu alive", d)
        self.assertIn("host load", d)
        self.assertIn("zfs: importing pool", d)

    def test_ssh_ok_skips_serial(self):
        d = load().stall_diag(self.vm(), 0, "ok")
        self.assertNotIn("last serial", d)

    def test_exited_qemu(self):
        self.assertIn("exited rc=1", load().stall_diag(self.vm(rc=1), 255, ""))

    def test_never_raises(self):
        self.assertIn("guest unreachable", load().stall_diag(object(), 255, ""))


if __name__ == "__main__":
    unittest.main()
