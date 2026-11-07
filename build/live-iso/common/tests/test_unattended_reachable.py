"""Unattended reinstall must always come back reachable (gringotts, 2026-11-07: a bare-metal autoinstall left the laptop answering ping with every port
closed for hours; nobody could read why).

* the live system of an `autoinstall.after=<restart>` install runs a key-only sshd from the answer file's keys (state only in /run), so a stuck install is inspectable;
* a hard time limit restarts the machine into whatever is on the disk and copies the partial log out;
* a lab test machine gets the watchdog + never-suspend files from the installer itself (first boot), not only from a package merged later.
"""
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "overlay", "usr", "lib", "live-installer"))
from liveinst import livessh as L  # noqa: E402
from liveinst import testmachine as TM  # noqa: E402

INIT = os.path.join(ROOT, "overlay/etc/init.d/live-installer")
KEY = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SzG6UOEjAJ5Z3c6o8XQJvhT4UQ2cUJvK lab@test"
ANSWERS = f'[global]\nhostname = "gringotts"\nssh_keys = ["{KEY}"]\nssh_password_auth = false\n\n[disk]\nfilesystem = "zfs"\n'


class Keys(unittest.TestCase):
    def test_keys_from_the_answer_file(self):
        self.assertEqual(L.parse_keys(ANSWERS), [KEY])

    def test_single_key_string_and_junk_filtered(self):
        self.assertEqual(L.parse_keys(f'[global]\nssh_key = "{KEY}"\n'), [KEY])
        self.assertEqual(L.parse_keys('[global]\nssh_keys = ["not a key", "rm -rf /"]\n'), [])
        self.assertEqual(L.parse_keys("this is [not toml"), [])

    def test_no_key_no_sshd(self):
        started = []
        ok, msg = L.start('[global]\nhostname = "x"\n', run=tempfile.mkdtemp(), runner=lambda *a, **k: started.append(a))
        self.assertFalse(ok)
        self.assertEqual(started, [])
        self.assertIn("closed", msg)

    def test_config_is_key_only_and_lives_in_run(self):
        c = L.sshd_config("/run/live-autoinstall-ssh", 22)
        for want in ("PasswordAuthentication no", "PermitRootLogin prohibit-password", "AuthenticationMethods publickey",
                     "AuthorizedKeysFile /run/live-autoinstall-ssh/authorized_keys", "HostKey /run/live-autoinstall-ssh/ssh_host_ed25519_key"):
            self.assertIn(want, c)
        self.assertNotIn("PasswordAuthentication yes", c)

    def test_nothing_outside_run(self):
        src = open(os.path.join(ROOT, "overlay/usr/lib/live-installer/liveinst/livessh.py")).read()
        self.assertNotIn("/etc/ssh", src)
        self.assertNotIn("rc-update", src)
        self.assertNotIn("rc-service", src)


@unittest.skipUnless(shutil.which("sshd") or os.path.exists("/usr/sbin/sshd"), "no sshd on this host")
class RealSshd(unittest.TestCase):
    """start the real sshd from the generated config on a free port and log in with the key (and fail without it)"""

    def test_login_with_the_key_only(self):
        sshd = shutil.which("sshd") or "/usr/sbin/sshd"
        d = tempfile.mkdtemp()
        try:
            kp = os.path.join(d, "k")
            subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "t", "-f", kp])
            pub = open(kp + ".pub").read().strip()
            s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
            run = os.path.join(d, "run")
            ok, msg = L.start(f'[global]\nssh_keys = ["{pub}"]\n', run=run, port=port, sshd=sshd, use_pam=False)
            self.assertTrue(ok, msg)
            try:
                if os.geteuid() != 0:
                    cfg = open(f"{run}/sshd_config").read().replace("PermitRootLogin prohibit-password", "")
                    open(f"{run}/sshd_config", "w").write(cfg)
                # root only when we are root; as a normal user the same config accepts that user's key
                base = ["ssh", "-p", str(port), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "BatchMode=yes",
                        "-o", "ConnectTimeout=8", "-o", "IdentitiesOnly=yes"]
                user = "root" if os.geteuid() == 0 else os.environ.get("USER", "nobody")
                end = time.time() + 10
                r = None
                while time.time() < end:
                    r = subprocess.run(base + ["-i", kp, f"{user}@127.0.0.1", "echo LIVE-OK"], capture_output=True, text=True)
                    if "LIVE-OK" in r.stdout:
                        break
                    time.sleep(0.5)
                if os.geteuid() != 0 and "LIVE-OK" not in (r.stdout if r else ""):
                    self.skipTest("unprivileged sshd cannot serve this account here: " + (r.stderr if r else "")[-200:])
                self.assertIn("LIVE-OK", r.stdout, r.stderr)
                bad = subprocess.run(base + ["-o", "PreferredAuthentications=password", f"{user}@127.0.0.1", "true"], capture_output=True, text=True)
                self.assertNotEqual(bad.returncode, 0)
            finally:
                try:
                    pid = int(open(f"{run}/sshd.pid").read())
                    os.kill(pid, 15)
                except (OSError, ValueError):
                    pass
        finally:
            shutil.rmtree(d, ignore_errors=True)


class InitScript(unittest.TestCase):
    def setUp(self):
        self.s = open(INIT).read()

    def test_guard_runs_only_for_the_restart_flow_and_before_the_installer(self):
        self.assertIn('[ "$after" = reboot ] && unattended_guard "$p"', self.s)
        self.assertLess(self.s.index('unattended_guard "$p"'), self.s.index("live-installer --answers auto"))

    def test_hard_time_limit_with_a_default_and_a_kernel_arg(self):
        self.assertIn('cmdarg "$p" autoinstall.timeout', self.s)
        self.assertIn("to=3600", self.s)
        blk = self.s.split("unattended_guard() {", 1)[1].split("\nstart()", 1)[0]
        self.assertIn("AUTOINSTALL-TIMEOUT", blk)
        self.assertIn("echo b > /proc/sysrq-trigger", blk)
        self.assertIn("copy_install_log", blk)
        self.assertIn("live-autoinstall-ssh", blk)

    def test_timer_is_stopped_when_the_installer_returns(self):
        self.assertLess(self.s.index("live-installer --answers auto"), self.s.index("autoinstall-timer.pid)\" 2>/dev/null; pkill"))

    def test_log_is_appended_so_the_guard_output_survives(self):
        self.assertIn("tee -a /var/log/autoinstall.log", self.s)
        self.assertNotIn("tee /var/log/autoinstall.log", self.s)

    def test_default_flow_keeps_the_live_system_closed(self):
        # the helper is only reachable through unattended_guard, which only the restart flow calls
        self.assertEqual(self.s.count("unattended_guard"), 2)        # the definition + the one call under [ "$after" = reboot ]

    def test_shell_syntax(self):
        r = subprocess.run(["sh", "-n", INIT], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_helper_is_executable_and_syntax_ok(self):
        p = os.path.join(ROOT, "overlay/usr/libexec/live-autoinstall-ssh")
        self.assertTrue(os.access(p, os.X_OK))
        self.assertEqual(subprocess.run(["sh", "-n", p]).returncode, 0)


class FakeInst:
    def __init__(self):
        self.files, self.cmds, self.logs = {}, [], []

    def write(self, rel, text, mode=0o644, append=False):
        self.files[rel] = (text, mode)

    def run(self, cmd, chroot=False, check=True, **k):
        self.cmds.append((cmd, chroot))

    def log(self, m):
        self.logs.append(m)


class TestMachinePayload(unittest.TestCase):
    PKG = os.path.join(ROOT, "..", "..", "..", "gentoo", "app-misc", "horcrux-test-machine", "files")
    SRC = os.path.join(ROOT, "overlay", "usr", "share", "live-installer", "test-machine")

    def test_payload_writes_watchdog_never_suspend_and_panic_files_and_enables_the_watchdog(self):
        inst = FakeInst()
        out = TM.install_payload(inst, self.SRC)
        self.assertIn("/usr/libexec/horcrux-test-watchdog", out)
        self.assertEqual(inst.files["/usr/libexec/horcrux-test-watchdog"][1], 0o755)
        self.assertIn("AllowSuspend=no", inst.files["/etc/elogind/sleep.conf.d/50-horcrux-test-machine.conf"][0])
        self.assertIn("kernel.panic = 20", inst.files["/etc/sysctl.d/50-horcrux-test-machine.conf"][0])
        self.assertIn("softdog", inst.files["/usr/libexec/horcrux-test-watchdog"][0])
        self.assertIn((["rc-update", "add", "horcrux-test-watchdog", "default"], True), inst.cmds)

    def test_missing_payload_file_is_an_error_not_a_silent_skip(self):
        with self.assertRaises(OSError):
            TM.install_payload(FakeInst(), tempfile.mkdtemp())

    def test_copies_are_byte_identical_to_the_package_files(self):
        if not os.path.isdir(self.PKG):
            self.skipTest("no gentoo/ overlay next to common/ (vendored copy)")
        for name, _dst, _mode in TM.PAYLOAD:
            a, b = os.path.join(self.SRC, name), os.path.join(self.PKG, name)
            self.assertEqual(open(a, "rb").read(), open(b, "rb").read(), f"{name} differs from gentoo/app-misc/horcrux-test-machine/files/{name}: copy it")

    def test_the_installer_stage_installs_it_for_a_test_machine_only(self):
        src = open(os.path.join(ROOT, "overlay/usr/lib/live-installer/liveinst/system.py")).read()
        blk = src.split("def stage_hardening(self):", 1)[1].split("def stage_use_flags", 1)[0]
        self.assertIn("if testmachine.requested(self.c):", blk)
        self.assertIn("testmachine.install_payload(self)", blk)


if __name__ == "__main__":
    unittest.main()
