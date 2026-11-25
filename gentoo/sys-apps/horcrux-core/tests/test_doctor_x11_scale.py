"""horcrux-doctor x11-scale: runs gnome-session-openrc-helper x11-scale-status per logged-in GNOME user (read-only).

A fake helper (shell script) and a fake /proc stand in for the real thing; nothing needs GNOME, root or a display.
"""
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
DOCTOR = os.path.abspath(os.path.join(HERE, "..", "files", "horcrux-doctor"))
ME = os.getuid()


@unittest.skipUnless(os.path.isfile(DOCTOR), "Horcrux repo tree only")
class X11Scale(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.proc = os.path.join(self.tmp, "proc")
        self.run_dir = os.path.join(self.tmp, "run")
        os.makedirs(self.proc)
        os.makedirs(self.run_dir)
        self.helper = os.path.join(self.tmp, "helper")
        self.log = os.path.join(self.tmp, "calls")

    def tearDown(self):
        subprocess.run(["rm", "-rf", self.tmp], check=False)

    def fake_helper(self, body):
        with open(self.helper, "w") as f:
            f.write('#!/bin/sh\necho "$1 uid=$(id -u) xdg=$XDG_RUNTIME_DIR" >> %s\n%s\n' % (self.log, body))
        os.chmod(self.helper, 0o755)

    def proc_entry(self, pid, comm, uid):
        d = os.path.join(self.proc, str(pid))
        os.makedirs(d)
        open(os.path.join(d, "comm"), "w").write(comm + "\n")
        open(os.path.join(d, "status"), "w").write("Name:\t%s\nUid:\t%d\t%d\t%d\t%d\n" % (comm, uid, uid, uid, uid))

    def doctor(self, *args, helper=None):
        env = dict(os.environ, HX_X11_HELPER=helper or self.helper, HX_PROC_DIR=self.proc, HX_RUN_DIR=self.run_dir,
                   HX_X11_TIMEOUT="5")
        return subprocess.run([sys.executable, DOCTOR, "x11-scale", *args], capture_output=True, text=True, env=env)

    def calls(self):
        if not os.path.exists(self.log):
            return []
        with open(self.log) as f:
            return f.read().splitlines()

    def test_ok_prints_helper_lines_and_sets_runtime_dir(self):
        self.fake_helper('echo "OK gsd-xsettings (42) serves :2"; echo "OK Xft.dpi=192 on :2"')
        self.proc_entry(100, "gnome-shell", ME)
        self.proc_entry(101, "bash", ME)
        r = self.doctor()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("OK gsd-xsettings (42) serves :2", r.stdout)
        self.assertIn("OK Xft.dpi=192 on :2", r.stdout)
        self.assertEqual(self.calls(), ["x11-scale-status uid=%d xdg=/run/user/%d" % (ME, ME)])

    def test_fail_line_is_nonzero(self):
        self.fake_helper('echo "OK gsd-xsettings (42) serves :2"; echo "FAIL Xft.dpi=96 on :2, expected 192"; exit 1')
        self.proc_entry(100, "gnome-shell", ME)
        r = self.doctor()
        self.assertEqual(r.returncode, 1)
        self.assertIn("FAIL Xft.dpi=96 on :2, expected 192", r.stdout)

    def test_helper_failing_silently_is_a_failure(self):
        self.fake_helper("exit 3")
        self.proc_entry(100, "gnome-shell", ME)
        r = self.doctor()
        self.assertEqual(r.returncode, 1)
        self.assertIn("FAIL helper exited 3", r.stdout)

    def test_fail_line_with_exit_zero_is_still_failure(self):
        self.fake_helper('echo "FAIL something"')
        self.proc_entry(100, "gnome-shell", ME)
        self.assertEqual(self.doctor().returncode, 1)

    def test_helper_timeout(self):
        self.fake_helper("sleep 30")
        self.proc_entry(100, "gnome-shell", ME)
        env = dict(os.environ, HX_X11_HELPER=self.helper, HX_PROC_DIR=self.proc, HX_RUN_DIR=self.run_dir, HX_X11_TIMEOUT="1")
        r = subprocess.run([sys.executable, DOCTOR, "x11-scale"], capture_output=True, text=True, env=env)
        self.assertEqual(r.returncode, 1)
        self.assertIn("timed out", r.stdout)

    def test_skips_on_systemd(self):
        self.fake_helper("echo FAIL should-not-run; exit 1")
        os.makedirs(os.path.join(self.run_dir, "systemd", "system"))
        self.proc_entry(100, "gnome-shell", ME)
        r = self.doctor()
        self.assertEqual(r.returncode, 0)
        self.assertIn("skipped: systemd", r.stdout)
        self.assertEqual(self.calls(), [])

    def test_skips_without_helper(self):
        self.proc_entry(100, "gnome-shell", ME)
        r = self.doctor(helper=os.path.join(self.tmp, "nope"))
        self.assertEqual(r.returncode, 0)
        self.assertIn("skipped:", r.stdout)

    def test_skips_without_gnome_user(self):
        self.fake_helper("exit 1")
        self.proc_entry(101, "bash", ME)
        r = self.doctor()
        self.assertEqual(r.returncode, 0)
        self.assertIn("no GNOME user is logged in", r.stdout)
        self.assertEqual(self.calls(), [])

    def test_quiet_skip_prints_nothing(self):
        r = self.doctor("--quiet", helper=os.path.join(self.tmp, "nope"))
        self.assertEqual((r.returncode, r.stdout), (0, ""))

    def test_user_option_checks_that_user_without_gnome_shell_lookup(self):
        import pwd
        self.fake_helper('echo "OK fine"')
        r = self.doctor("--user", pwd.getpwuid(ME).pw_name)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(len(self.calls()), 1)

    def test_unknown_user_is_usage_error(self):
        self.fake_helper("exit 0")
        r = self.doctor("--user", "no-such-user-xyz")
        self.assertEqual(r.returncode, 2)

    @unittest.skipIf(ME == 0, "root can run as anyone")
    def test_other_user_needs_root(self):
        self.fake_helper("exit 0")
        self.proc_entry(100, "gnome-shell", 0)
        r = self.doctor()
        self.assertEqual(r.returncode, 0)
        self.assertIn("SKIP needs root", r.stdout)
        self.assertEqual(self.calls(), [])
        r = self.doctor("--user", "root")
        self.assertEqual(r.returncode, 2)

    def test_json(self):
        self.fake_helper('echo "OK a"')
        self.proc_entry(100, "gnome-shell", ME)
        r = self.doctor("--json")
        d = json.loads(r.stdout)
        self.assertTrue(d["ok"])
        self.assertEqual(d["users"][0]["lines"], ["OK a"])

    def test_same_user_twice_is_checked_once(self):
        self.fake_helper('echo "OK a"')
        self.proc_entry(100, "gnome-shell", ME)
        self.proc_entry(101, "gnome-shell", ME)  # same user twice: once
        self.doctor()
        self.assertEqual(len(self.calls()), 1)


if __name__ == "__main__":
    unittest.main()
