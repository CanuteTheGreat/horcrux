"""grub_serial_keys (qemu-test.py): GRUB hotkeys over the UART are re-sent until GRUB shows it took them.
Regression for run 2319 (uefi-usb-auto-openconsole): the single 's' + '4' were lost, the countdown ran out and the default
Graphical install booted, so the automated installer never started."""
import importlib.util
import os
import re
import sys
import threading
import unittest

COMMON = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, COMMON)

COUNT = b"The highlighted entry will be executed automatically in %ds.\r\n"


class FakeVM:
    """Scripted GRUB: `script` maps a key to a list of byte strings, one popped per press (b'' = the key was lost)."""

    def __init__(self, script, pending=b""):
        self.lock = threading.Lock()
        self.raw = pending
        self.script = {k: list(v) for k, v in script.items()}
        self.sent = []

    def send(self, k):
        self.sent.append(k)
        q = self.script.get(k) or [b""]
        out = q.pop(0) if len(q) > 1 else q[0]
        with self.lock:
            self.raw += out

    def expect_raw(self, pat, timeout, consume=True):
        rx = re.compile(pat)
        with self.lock:
            m = rx.search(self.raw)
            if m and consume:
                self.raw = self.raw[m.end():]
            return m


class GrubSerialKeys(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        sp = importlib.util.spec_from_file_location("qt_grubkeys", os.path.join(COMMON, "qemu-test.py"))
        cls.qt = importlib.util.module_from_spec(sp)
        sp.loader.exec_module(cls.qt)

    def run_keys(self, vm, want=r"Automated install"):
        return self.qt.grub_serial_keys(vm, ["s", "4"], want, tries=4, settle=0, quiet=0)

    def test_keys_taken_first_time(self):
        vm = FakeVM({"s": [b"\x1b[2J submenu"], "4": [b"\x1b[2J"]}, pending=COUNT % 5)   # countdown before the key is no evidence
        ok, why = self.run_keys(vm)
        self.assertTrue(ok, why)
        self.assertEqual(vm.sent, ["s", "4"])

    def test_lost_menu_key_is_sent_again(self):
        vm = FakeVM({"s": [COUNT % 4, COUNT % 3, b"\x1b[2J submenu"], "4": [b"\x1b[2J"]})
        ok, why = self.run_keys(vm)
        self.assertTrue(ok, why)
        self.assertEqual(vm.sent, ["s", "s", "s", "4"])

    def test_countdown_expiry_is_a_failure_naming_the_entry(self):
        vm = FakeVM({"s": [COUNT % 1, COUNT % 0 + b"Booting `Patronus - Graphical install'\r\n"]})
        ok, why = self.run_keys(vm)
        self.assertFalse(ok)
        self.assertIn("Graphical install", why)
        self.assertNotIn("4", vm.sent)

    def test_lost_entry_key_is_sent_again_after_silence(self):
        vm = FakeVM({"s": [b"submenu"], "4": [b"", b"\x1b[2J"]})
        ok, why = self.run_keys(vm)
        self.assertTrue(ok, why)
        self.assertEqual(vm.sent, ["s", "4", "4"])
        self.assertIn("re-send", why)

    def test_menu_key_never_taken(self):
        vm = FakeVM({"s": [COUNT % 5]})
        ok, why = self.run_keys(vm)
        self.assertFalse(ok)
        self.assertEqual(vm.sent, ["s"] * 4)

    def test_entry_key_never_taken(self):
        vm = FakeVM({"s": [b"submenu"], "4": [b""]})
        ok, why = self.run_keys(vm)
        self.assertFalse(ok)
        self.assertIn("silent", why)


if __name__ == "__main__":
    unittest.main()
