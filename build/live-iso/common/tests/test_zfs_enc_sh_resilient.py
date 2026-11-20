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


class DeadVm(unittest.TestCase):
    """QEMU gone after a transport drop (run 3150: 'closed by remote host' then 'Connection refused'): no re-attach attempt (wait_ssh would burn 180 s
    on a dead forward), the failure detail carries how QEMU ended, and the assertion still fails."""
    def test_dead_vm_is_reported_not_retried(self):
        h = harness([(255, "Connection to 127.0.0.1 closed by remote host.\n")])
        waited = []
        h.wait_ssh = lambda t=0: waited.append(t) or True

        class P:
            def poll(self):
                return -9

            def wait(self, timeout=None):
                return -9

        class V:
            p, name, out = P(), "5-boot4-resume", "/nonexistent"

            def alive(self):
                return False
        h.vm_death_report = lambda vm, label: "QEMU exited during the command (exit status -9)"
        rc, o = h.sh_resilient("scrub", vm=V())
        self.assertEqual(rc, 255)
        self.assertIn("exit status -9", o)
        self.assertEqual(waited, [])
        self.assertEqual(len(h.calls), 1)

    def test_report_names_exit_status_and_survives_missing_files(self):
        cls = [v for v in vars(zet).values() if isinstance(v, type) and hasattr(v, "vm_death_report")][0]
        h = cls.__new__(cls)
        h.res = {}

        class P:
            def poll(self):
                return 0

        class V:
            p, name, out = P(), "5-boot4-resume", "/nonexistent"
            log = type("L", (), {"name": "/nonexistent/x"})()
        msg = h.vm_death_report(V(), "scrub-after-resume")
        self.assertIn("exit status 0", msg)
        self.assertEqual(len(h.res["vm_died_during_command"]), 1)

    def test_report_includes_qmp_shutdown_reason(self):
        cls = [v for v in vars(zet).values() if isinstance(v, type) and hasattr(v, "vm_death_report")][0]
        h = cls.__new__(cls)
        h.res = {}

        class P:
            def poll(self):
                return -9

        class V:
            p, name, out = P(), "5-boot4-resume", "/nonexistent"
            log = type("L", (), {"name": "/nonexistent/x"})()
            events = ['03:16:09 SHUTDOWN {"guest": true, "reason": "guest-shutdown"}']
        msg = h.vm_death_report(V(), "scrub-after-resume")
        self.assertIn("qmp events", msg)
        self.assertIn("guest-shutdown", msg)

    def test_event_reader_records_events_from_a_second_monitor(self):
        import os, socket, tempfile, threading, json, time
        qt = zet.qt
        d = tempfile.mkdtemp()
        path = os.path.join(d, "x.ev")
        srv = socket.socket(socket.AF_UNIX)
        srv.bind(path)
        srv.listen(1)

        def server():
            c, _ = srv.accept()
            f = c.makefile("rw")
            f.write(json.dumps({"QMP": {}}) + "\n")
            f.flush()
            f.readline()
            f.write(json.dumps({"return": {}}) + "\n")
            f.write(json.dumps({"event": "SHUTDOWN", "data": {"guest": True, "reason": "guest-reset"}}) + "\n")
            f.flush()
            time.sleep(0.5)
            c.close()
        threading.Thread(target=server, daemon=True).start()

        class P:
            def poll(self):
                return None
        v = qt.VM.__new__(qt.VM)
        v.ev_path, v.events, v.p = path, [], P()
        v._ev_rd()
        self.assertEqual(len(v.events), 1)
        self.assertIn("guest-reset", v.events[0])


if __name__ == "__main__":
    unittest.main()
