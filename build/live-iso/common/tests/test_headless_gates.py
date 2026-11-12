"""headless_gates.py: pure-function tests (no VM) + the guest-side scripts run against a fake root."""
import datetime
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
spec = importlib.util.spec_from_file_location("headless_gates", os.path.join(ROOT, "headless_gates.py"))
HG = importlib.util.module_from_spec(spec)
spec.loader.exec_module(HG)


def run_script(script, **subst):
    for k, v in subst.items():
        script = script.replace(k, v)
    return subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=60).stdout


class MonthsOff(unittest.TestCase):
    def test_base_is_months_in_the_past(self):
        now = datetime.datetime(2026, 11, 3, 12, 0, 5, 999)
        self.assertEqual(HG.months_off_base(now), "2026-07-06T12:00:05")
        self.assertGreaterEqual(HG.MONTHS_OFF, 90)


class GuiScan(unittest.TestCase):
    def scan(self, pkgs):
        with tempfile.TemporaryDirectory() as d:
            for p in pkgs:
                os.makedirs(os.path.join(d, "var/db/pkg", p))
            return run_script(HG.GUI_CHECK, **{"/var/db/pkg": os.path.join(d, "var/db/pkg")})

    def test_headless_set_passes(self):
        o = self.scan(["net-firewall/nftables-1.1.3", "net-dns/unbound-1.22.0-r1", "sys-apps/dbus-1.14.10", "dev-lang/python-3.12.8"])
        self.assertIn("GUI-PACKAGES 0", o)
        self.assertIn("GUI-DONE", o)

    def test_gui_packages_are_found_by_name_not_version(self):
        o = self.scan(["gui-wm/cage-0.2.1", "gui-libs/gtk-4.16.5", "x11-base/xorg-server-21.1.15-r2", "media-libs/mesa-24.2.8", "net-misc/curl-8.11.1"])
        self.assertIn("GUI-PACKAGES 4", o)
        self.assertIn("gui-wm/cage", o)
        self.assertIn("x11-base/xorg-server", o)

    def test_gate_decision(self):
        out = {"v": ""}
        steps = []
        HG.gui_check(lambda cmd: (0, out["v"]), lambda n, ok, i="": steps.append((n, ok)))
        self.assertEqual(steps[-1][1], False)       # empty answer = the script did not finish = FAIL, never a silent pass
        out["v"] = "PKG-COUNT 300\nGUI-PACKAGES 0 \nGUI-PROCESSES 0 \nGUI-SERVICES 0 \nGUI-BINARIES 0 \nGUI-DONE\n"
        HG.gui_check(lambda cmd: (0, out["v"]), lambda n, ok, i="": steps.append((n, ok)))
        self.assertEqual(steps[-1][1], True)
        out["v"] = out["v"].replace("GUI-PROCESSES 0 ", "GUI-PROCESSES 1 Xorg")
        HG.gui_check(lambda cmd: (0, out["v"]), lambda n, ok, i="": steps.append((n, ok)))
        self.assertEqual(steps[-1][1], False)


class AdminTools(unittest.TestCase):
    GOOD = "TOOL-OK equery\nTOOL-OK eclean\nTOOL-OK revdep-rebuild\nTOOL-OK euse\nequery 0.8.1\napp-portage/gentoolkit-0.8.1\nADMIN-DONE\n"

    def run_gate(self, out):
        steps = []
        HG.admin_tools_gate(lambda cmd: (0, out), lambda n, ok, i="": steps.append((n, ok, i)))
        return steps[-1]

    def test_good(self):
        self.assertTrue(self.run_gate(self.GOOD)[1])

    def test_missing_equery_fails(self):
        out = self.GOOD.replace("TOOL-OK equery", "TOOL-MISSING equery")
        self.assertFalse(self.run_gate(out)[1])

    def test_no_package_or_empty_answer_fails(self):
        self.assertFalse(self.run_gate(self.GOOD.replace("app-portage/gentoolkit-0.8.1\n", ""))[1])
        self.assertFalse(self.run_gate("")[1])           # the script did not finish = FAIL, never a silent pass

    def test_script_checks_equery(self):
        seen = []
        HG.admin_tools_gate(lambda cmd: (seen.append(cmd) or (0, "")), lambda *a, **k: None)
        self.assertIn("equery", seen[0])
        self.assertIn("revdep-rebuild", seen[0])


class Accounts(unittest.TestCase):
    def check(self, out, **kw):
        steps = []
        HG.account_check(lambda cmd: (0, out), lambda n, ok, i="": steps.append((n, ok, i)), **kw)
        return steps

    GOOD = "EMPTY-PASSWORDS 0 \nROOT-HASH !\nAGETTY 2 AUTOLOGIN-OR-NOLOGIN 0 \nINITTAB-AUTOLOGIN 0\nLOGIN-USERS-OTHER-THAN-ROOT 0 \nACCOUNT-DONE\n"

    def test_good(self):
        s = self.check(self.GOOD)
        self.assertTrue(all(ok for _, ok, _ in s), s)

    def test_empty_password_fails(self):
        s = self.check(self.GOOD.replace("EMPTY-PASSWORDS 0 ", "EMPTY-PASSWORDS 1 hxtmp"))
        self.assertFalse(s[0][1])

    def test_autologin_fails_unless_console_menu_was_requested(self):
        bad = self.GOOD.replace("AUTOLOGIN-OR-NOLOGIN 0 ", "AUTOLOGIN-OR-NOLOGIN 1 /sbin/agetty -a root ttyS0")
        self.assertFalse(self.check(bad)[0][1])
        self.assertTrue(self.check(bad, console_open=True)[0][1])

    @unittest.skipUnless(subprocess.run(["which", "openssl"], capture_output=True).returncode == 0, "openssl missing")
    def test_root_hash_reuse_is_caught(self):
        h = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "abcdefgh", "-stdin"], input="Web-Admin-Pw-1\n", text=True).strip()
        out = self.GOOD.replace("ROOT-HASH !", "ROOT-HASH " + h)
        self.assertFalse(self.check(out, web_secret="Web-Admin-Pw-1")[1][1])
        self.assertTrue(self.check(out, web_secret="Another-Pw-2")[1][1])
        self.assertTrue(HG.verifies("Web-Admin-Pw-1", h))
        self.assertFalse(HG.verifies("nope", h))
        self.assertIsNone(HG.verifies("x", "!"))


class WiredIntoTheHarness(unittest.TestCase):
    def test_qemu_test_uses_every_gate(self):
        src = open(os.path.join(ROOT, "qemu-test.py")).read()
        for needle in ("--headless-gates", "-rtc", "HG.gui_check", "HG.admin_tools_gate", "HG.account_check", "HG.clock_gate", "HG.link(", "installed-boots-with-the-network-links-down", "-S"):
            self.assertIn(needle, src, needle)


if __name__ == "__main__":
    unittest.main()
