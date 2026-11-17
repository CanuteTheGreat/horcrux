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


class SerialProbe(unittest.TestCase):
    def fake(self, menu=True):
        import threading
        vm = types.SimpleNamespace(lock=threading.Lock(), raw=b"", sent=[])
        def send(s):
            vm.sent.append(s)
            if s == "\r" and menu:
                vm.raw += b"Enter an option: "
            if s.startswith("echo DIAG-S"):
                vm.raw += b"DIAG-S-42\nlo UNKNOWN 127.0.0.1/8\nenp0s2 DOWN 10.77.0.1/24\nchain input { type filter hook input priority 0; policy drop;\nLOCALWEB-8443-200\nPING-GW-0\nCTRULES-1\nLISTEN 0 128 0.0.0.0:22\nDIAG-E-42\n"
        vm.send = send
        def expect_raw(pat, timeout, consume=True):
            import re
            m = re.search(pat.encode() if isinstance(pat, str) else pat, vm.raw)
            return m
        vm.expect_raw = expect_raw
        return vm

    def test_alive_guest_dumps_network(self):
        vm = self.fake()
        d = load().serial_probe(vm, 1)
        self.assertIn("answers", d)
        self.assertIn("enp0s2 DOWN", d)
        self.assertIn("policy drop", d)
        self.assertIn("LOCALWEB-8443-200", d)
        self.assertIn("PING-GW-0", d)
        self.assertIn("CTRULES-1", d)
        self.assertTrue(any("ping -c2" in x and "10.0.2.2" in x for x in vm.sent))
        self.assertIn("8\r", vm.sent)
        self.assertTrue(any("nft list ruleset" in x and "LOCALWEB-" in x for x in vm.sent))

    def test_silent_guest(self):
        d = load().serial_probe(self.fake(menu=False), 1)
        self.assertIn("SILENT", d)

    def test_never_raises(self):
        self.assertIn("serial probe failed", load().serial_probe(object(), 1))


if __name__ == "__main__":
    unittest.main()
