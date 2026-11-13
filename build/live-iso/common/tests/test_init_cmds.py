"""qemu-test.py is init-aware: the same health checks are produced for OpenRC and systemd guests (parity rule 2026-11-13)."""
import os, re, sys, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import init_cmds as ic  # noqa: E402


class InitCmds(unittest.TestCase):
    def test_openrc_commands(self):
        self.assertEqual(ic.service_status_cmd("openrc", "horcrux-api"), "rc-service horcrux-api status 2>&1")
        h = ic.services_health_sh("openrc", ["horcrux-api", "sshd"])
        self.assertIn("rc-service $s status", h[0])
        self.assertIn("rc-status --crashed", h[1])
        self.assertIn("rc-status -a", ic.boot_diag_cmd("openrc"))

    def test_systemd_commands(self):
        self.assertEqual(ic.service_status_cmd("systemd", "horcrux-api"), "systemctl is-active horcrux-api 2>&1")
        h = ic.services_health_sh("systemd", ["horcrux-api", "sshd"])
        self.assertIn("systemctl is-active $s", h[0])
        self.assertIn("systemctl --failed", h[1])
        self.assertNotIn("rc-", " ".join(h) + ic.boot_diag_cmd("systemd"))

    def test_both_inits_check_every_service_and_report_in_the_same_words(self):
        for init in ic.INITS:
            h = ic.services_health_sh(init, ["a", "b"])
            self.assertIn("for s in a b;", h[0])
            self.assertIn("BAD service", h[0])
            self.assertIn("BAD crashed services", h[1])

    def test_running_patterns(self):
        self.assertTrue(re.search(ic.service_running_re("openrc"), " * status: started"))
        self.assertTrue(re.search(ic.service_running_re("systemd"), "active", re.M))
        self.assertFalse(re.search(ic.service_running_re("systemd"), "inactive", re.M))

    def test_boot_banner_matches_either_init(self):
        for banner in ("Linux version 6.12", "OpenRC 0.54 is starting up", "systemd 257 running in system mode"):
            self.assertTrue(re.search(ic.BOOT_RE, banner), banner)
        self.assertTrue(ic.boots_with("systemd", "Welcome to Gentoo! systemd 257"))
        self.assertFalse(ic.boots_with("systemd", "OpenRC 0.54"))

    def test_unknown_init_is_an_error(self):
        with self.assertRaises(ValueError):
            ic.service_status_cmd("runit", "x")

    def test_qemu_test_uses_the_helpers_not_hardcoded_openrc(self):
        src = open(os.path.join(HERE, "..", "qemu-test.py")).read()
        self.assertIn("--init", src)
        for bad in ("rc-service {self.P", "rc-status --crashed", "rc-status -a"):
            self.assertNotIn(bad, src, f"qemu-test.py still hard-codes {bad!r}")


if __name__ == "__main__":
    unittest.main()
