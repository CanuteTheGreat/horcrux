"""Pure-python checks (no QEMU): init renderers, serial framing, hub switching, scenario init-call gate."""
import os
import re
import socket
import struct
import sys
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
from vmtest import guest  # noqa: E402
from vmtest.netsim import Hub  # noqa: E402


class Init(unittest.TestCase):
    def test_every_op_rendered_for_both(self):
        for name, i in guest.INITS.items():
            for op in ("is_active", "is_failed", "start", "stop", "restart", "enable", "disable"):
                c = getattr(i, op)("horcrux-api")
                self.assertIn("horcrux-api", c, (name, op))
            for op in ("boot_complete", "status_all", "diag"):
                self.assertTrue(getattr(i, op)())
        self.assertIn("systemctl", guest.INITS["systemd"].start("x"))
        self.assertIn("rc-service", guest.INITS["openrc"].start("x"))

    def test_quoting(self):
        self.assertIn("'a b; rm -rf /'", guest.INITS["openrc"].start("a b; rm -rf /"))
        self.assertIn("'a b; rm -rf /'", guest.INITS["systemd"].start("a b; rm -rf /"))

    def test_serial_frame_marker_not_in_echo(self):
        line, rid = guest.wrap_for_serial("echo hi")
        self.assertNotIn(f"@@E{rid}@@", line)      # the tty echo of the typed line must not look like the end marker
        self.assertNotIn(f"@@S{rid}@@", line)
        self.assertNotIn("\n", line)


class HubTest(unittest.TestCase):
    def _port(self, hub):
        s = socket.create_connection(("127.0.0.1", hub.port))
        s.settimeout(0.5)
        return s

    def _frame(self, dst, src):
        return bytes.fromhex(dst.replace(":", "")) + bytes.fromhex(src.replace(":", "")) + b"\x08\x00" + b"x" * 50

    def _send(self, s, f):
        s.sendall(struct.pack(">I", len(f)) + f)

    def _recv(self, s):
        try:
            h = s.recv(4)
            return s.recv(struct.unpack(">I", h)[0]) if h else None
        except socket.timeout:
            return None

    def test_partition_and_heal(self):
        import tempfile
        h = Hub("t", tempfile.mkdtemp(), pcap=True)
        A, B = "52:54:00:01:00:00", "52:54:00:02:00:00"
        h.register(A, "a"); h.register(B, "b")
        a, b = self._port(h), self._port(h)
        time.sleep(0.3)                                          # let the hub accept both before the first frame
        self._send(a, self._frame("ff:ff:ff:ff:ff:ff", A)); time.sleep(0.2)
        self.assertIsNotNone(self._recv(b))                      # broadcast flooded
        self._send(b, self._frame(A, B)); time.sleep(0.2)
        self.assertIsNotNone(self._recv(a))                      # unicast learned
        h.partition(["a"], ["b"])
        self._send(a, self._frame(B, A)); time.sleep(0.2)
        self.assertIsNone(self._recv(b))
        self.assertGreaterEqual(h.stats["dropped"], 1)
        h.heal()
        self._send(a, self._frame(B, A)); time.sleep(0.2)
        self.assertIsNotNone(self._recv(b))
        h.close()


class NoInitCallsInScenarios(unittest.TestCase):
    """OWNER RULE: scenarios never name an init system; the guest layer does."""
    PAT = re.compile(r"\b(systemctl|rc-service|rc-update|journalctl|rc-status)\b")

    def test_scenarios_are_init_agnostic(self):
        d = os.path.join(HERE, "..", "scenarios")
        for f in sorted(os.listdir(d)):
            if not f.endswith(".py"):
                continue
            for n, line in enumerate(open(os.path.join(d, f)), 1):
                if line.lstrip().startswith("#") or "vmtest:init-ok" in line:
                    continue
                self.assertFalse(self.PAT.search(line), f"{f}:{n} names an init system: {line.strip()}")


if __name__ == "__main__":
    unittest.main()
