"""Console menu 'Reset to factory defaults': the web service must be listening before the reset is reported done, and a restart that
re-applies the whole config store (Patronus: `patronus apply`) must not be killed after 120 s on a loaded guest.
Run: python3 -m unittest discover -s build/live-iso/common/tests   (Python >= 3.11)
"""
import io
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import consolemenu as CM  # noqa: E402
from liveinst import system as S  # noqa: E402

PROF = {"name": "patronus", "title": "Patronus", "services": ["patronus-web"], "web_service": "patronus-web", "web_port": 8443,
        "db_path": "", "reapply_services": []}


class Fake:
    """scripted stand-in for S.sh: records every call, answers by command text"""
    def __init__(self, listening_after=0, restart_rc=(0,)):
        self.calls, self.polls, self.listening_after, self.restart_rc = [], 0, listening_after, list(restart_rc)

    def __call__(self, cmd, timeout=60):
        self.calls.append((cmd, timeout))
        if isinstance(cmd, str) and "ss -ltn" in cmd:
            self.polls += 1
            return (0 if self.polls > self.listening_after else 1), ""
        if isinstance(cmd, list) and cmd[-1] == "restart":
            return (self.restart_rc.pop(0) if self.restart_rc else 0), "boom"
        return 0, ""


class FactoryResetWeb(unittest.TestCase):
    def setUp(self):
        self.orig_sh, self.orig_sleep = S.sh, CM.time.sleep
        CM.time.sleep = lambda s: None
        self.c = CM.Console(PROF, inp=lambda p="": "", out=io.StringIO())

    def tearDown(self):
        S.sh, CM.time.sleep = self.orig_sh, self.orig_sleep

    def clock(self):
        t = [0.0]
        return (lambda: t[0]), (lambda s: t.__setitem__(0, t[0] + s))

    def test_wait_listening_polls_until_up(self):
        S.sh = Fake(listening_after=3)
        clock, nap = self.clock()
        self.assertTrue(self.c.wait_listening(60, clock=clock, nap=nap))
        self.assertEqual(S.sh.polls, 4)

    def test_wait_listening_gives_up_at_the_deadline(self):
        S.sh = Fake(listening_after=10 ** 6)
        clock, nap = self.clock()
        self.assertFalse(self.c.wait_listening(10, interval=2, clock=clock, nap=nap))
        self.assertLessEqual(S.sh.polls, 7)

    def test_start_web_starts_then_confirms_the_port(self):
        S.sh = Fake(listening_after=0)
        self.assertTrue(self.c.start_web(wait=5))
        self.assertEqual(S.sh.calls[0][0], ["rc-service", "patronus-web", "start"])
        self.assertFalse(any(isinstance(x[0], list) and x[0][-1] == "restart" for x in S.sh.calls))

    def test_start_web_restarts_once_when_the_port_stays_closed(self):
        S.sh = Fake(listening_after=10 ** 6)
        self.c.wait_listening = lambda timeout, **k: False
        self.assertFalse(self.c.start_web(wait=5))
        restarts = [x for x in S.sh.calls if isinstance(x[0], list) and x[0][-1] == "restart"]
        self.assertEqual(len(restarts), 1)
        self.assertIn("not listening", self.c.out.getvalue())

    def test_restart_timeout_is_not_the_old_120_seconds_and_retries_once(self):
        S.sh = Fake(restart_rc=(1, 0))
        rc, _ = self.c.rc_restart("patronus-firewall")
        self.assertEqual(rc, 0)
        calls = [x for x in S.sh.calls if x[0][-1] == "restart"]
        self.assertEqual(len(calls), 2)
        self.assertTrue(all(t >= 300 for _, t in calls), calls)

    def test_factory_reset_reports_a_dead_web_service(self):
        S.sh = Fake(listening_after=10 ** 6)
        self.c.wait_listening = lambda timeout, **k: False
        self.c.ask = lambda *a, **k: "RESET"
        self.c.make_backup = lambda tag: "/tmp/b.tar.gz"
        self.c.new_password = lambda: "Kx9!mQv2-tle-77Zp"
        self.c.write_firstboot = lambda pw: None
        self.c.remove_firstboot_later = lambda: None
        self.c.reset_files = lambda: []
        self.c.factory_reset()
        out = self.c.out.getvalue()
        self.assertIn("NOT listening", out)
        self.assertNotIn("Factory defaults restored; the web service was restarted.", out)


if __name__ == "__main__":
    unittest.main()
