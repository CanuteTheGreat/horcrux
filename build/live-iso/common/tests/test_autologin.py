"""No display-manager autologin by default (owner rule 2026-11-03) + per-machine hostname must be asked/confirmed.

Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import importlib.util
import os
import re
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.join(HERE, "..")
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
sys.path.insert(0, os.path.join(COMMON, "overlay", "usr", "lib", "live-installer"))
from liveinst import autologin as AL  # noqa: E402
from liveinst import hwprofile as HW  # noqa: E402
from liveinst import system as S  # noqa: E402

SURFACE_PKG = os.path.join(ROOT, "gentoo", "app-misc", "horcrux-surface-desktop")


def put(root, rel, text):
    p = root + rel
    os.makedirs(os.path.dirname(p), exist_ok=True)
    open(p, "w").write(text)
    return p


class FakeInst:
    """just enough of Installer for the hardware-profile desktop step"""

    def __init__(self, root, desktop):
        self.target, self.c, self.logs, self.runs = root, {"desktop": desktop}, [], []

    def write(self, rel, text, append=False):
        p = self.target + rel
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "a" if append else "w").write(text)

    def run(self, cmd, **kw):
        self.runs.append(cmd)

    def log(self, m):
        self.logs.append(m)


class Scanner(unittest.TestCase):
    def test_active_settings_are_found_everywhere(self):
        with tempfile.TemporaryDirectory() as r:
            put(r, "/etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf", "[Seat:*]\nautologin-user=u\nautologin-user-timeout=0\n")
            put(r, "/etc/gdm/custom.conf", "[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=u\n")
            put(r, "/etc/sddm.conf.d/a.conf", "[Autologin]\nUser=u\nSession=plasma\n")
            put(r, "/etc/inittab", "c1:12345:respawn:/sbin/agetty --autologin u --noclear 38400 tty1 linux\nc2:2345:respawn:/sbin/agetty 38400 tty2 linux\n")
            files = {a[0] for a in AL.audit(r)}
            self.assertEqual(files, {"/etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf", "/etc/gdm/custom.conf", "/etc/sddm.conf.d/a.conf", "/etc/inittab"})

    def test_comments_and_disabled_values_are_clean(self):
        with tempfile.TemporaryDirectory() as r:
            put(r, "/etc/lightdm/lightdm.conf", "[Seat:*]\n#autologin-user=u\n# autologin-user-timeout = 0\nautologin-guest=false\n")
            put(r, "/etc/gdm/custom.conf", "[daemon]\nWaylandEnable=true\nAutomaticLoginEnable=false\n#TimedLoginEnable=true\n")
            put(r, "/etc/sddm.conf", "[Autologin]\nUser=\n")
            self.assertEqual(AL.audit(r), [])

    def test_scrub_removes_every_setting_and_keeps_the_rest(self):
        with tempfile.TemporaryDirectory() as r:
            put(r, "/etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf", "[Seat:*]\nautologin-user=u\n")
            put(r, "/etc/lightdm/lightdm.conf.d/50-x.conf", "[Seat:*]\ngreeter-session=lightdm-gtk-greeter\nautologin-user=u\n")
            put(r, "/etc/gdm/custom.conf", "[daemon]\nWaylandEnable=true\nAutomaticLoginEnable=True\nAutomaticLogin=u\n")
            put(r, "/etc/inittab", "c1:12345:respawn:/sbin/agetty --autologin root --noclear 38400 tty1 linux\n")
            changed = AL.scrub(r)
            self.assertEqual(len(changed), 4)
            self.assertEqual(AL.audit(r), [])
            self.assertFalse(os.path.exists(r + "/etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf"))
            self.assertIn("greeter-session=lightdm-gtk-greeter", open(r + "/etc/lightdm/lightdm.conf.d/50-x.conf").read())
            self.assertIn("WaylandEnable=true", open(r + "/etc/gdm/custom.conf").read())


class InstallerDefaults(unittest.TestCase):
    def test_default_config_has_no_autologin(self):
        c = S.default_config({"name": "horcrux", "title": "H", "filesystems": ["xfs"], "swap_mb": 0, "network_mode": "management", "services": [],
                              "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"})
        self.assertFalse(c["desktop"]["autologin"])
        self.assertFalse(c["user_autologin"])

    def desktop_step(self, desktop):
        with tempfile.TemporaryDirectory() as r:
            put(r, "/usr/share/horcrux/desktop/display-manager", "lightdm\n")
            put(r, "/etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf", "[Seat:*]\nautologin-user=stale\n")      # a stale file shipped by an old image
            inst = FakeInst(r, desktop)
            HW.add_desktop_user(inst, desktop)
            return AL.audit(r), inst.logs

    def test_xfce_profile_without_choice_has_no_autologin_even_with_a_stale_file(self):
        hits, logs = self.desktop_step({"enable": True, "user": "owner", "password_hash": "x"})
        self.assertEqual(hits, [])
        self.assertTrue(any("removed the setting" in l for l in logs))

    def test_xfce_profile_opt_in_works_only_when_explicitly_true_and_warns(self):
        hits, logs = self.desktop_step({"enable": True, "user": "owner", "password_hash": "x", "autologin": True})
        self.assertEqual({h[0] for h in hits}, {"/etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf"})
        self.assertTrue(any(l.startswith("WARNING") for l in logs))
        for junk in ("true", "yes", 1, "false", None, False):          # only the boolean True counts (a stray string must not enable it)
            if junk is True:
                continue
            hits, _ = self.desktop_step({"enable": True, "user": "owner", "password_hash": "x", "autologin": junk})
            self.assertEqual(hits, [], junk)


class PackagesAndImages(unittest.TestCase):
    def test_no_package_or_overlay_file_configures_autologin(self):
        bad = []
        for base in (os.path.join(ROOT, "gentoo"), os.path.join(ROOT, "build", "live-iso", "project"), os.path.join(ROOT, "build", "live-iso", "common", "overlay")):
            for dp, _dn, fn in os.walk(base):
                for f in fn:
                    p = os.path.join(dp, f)
                    rel = p[len(ROOT):]
                    if not re.search(r"/(lightdm|gdm3?|sddm)([./]|$)|/etc/inittab$|/conf\.d/agetty", rel + "/"):
                        continue
                    try:
                        t = open(p, errors="replace").read()
                    except OSError:
                        continue
                    for i, line in AL._active_lines(p, t):
                        if "live-installer" in rel or "/t2/live/" in rel or "live-serial" in rel:
                            continue                  # the LIVE medium's root consoles (root locked, sshd off) are not an installed-system login
                        bad.append(f"{rel}:{i}")
        self.assertEqual(bad, [])

    def test_surface_answer_file_builder_never_turns_it_on(self):
        t = open(os.path.join(SURFACE_PKG, "files", "root", "usr", "bin", "horcrux-surface-auto")).read()
        self.assertIn("autologin = false", t)
        self.assertNotIn("autologin = true", t)

    def test_surface_package_ships_chrony_backup_and_services(self):
        eb = [f for f in os.listdir(SURFACE_PKG) if f.endswith(".ebuild")]
        self.assertEqual(len(eb), 1, eb)
        t = open(os.path.join(SURFACE_PKG, eb[0])).read()
        for atom in ("net-misc/chrony", "net-misc/horcrux-chrony-nm", "sys-apps/horcrux-backup"):
            self.assertIn(atom, t)
        self.assertIn("chronyd:default", open(os.path.join(SURFACE_PKG, "files", "root", "usr", "share", "live-installer", "hwprofiles", "surface.toml")).read())

    def test_every_gate_harness_checks_autologin(self):
        for rel in ("build/live-iso/common/zfs-enc-test.py", "gentoo/surface/tests/surface-xfce-test.py"):
            t = open(os.path.join(ROOT, rel)).read()
            self.assertIn("no_autologin_config_gate", t, rel)
            self.assertIn("greeter_no_session_gate", t, rel)


class GateSelfTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        sp = importlib.util.spec_from_file_location("qt", os.path.join(COMMON, "qemu-test.py"))
        cls.qt = importlib.util.module_from_spec(sp)
        sp.loader.exec_module(cls.qt)

    def run_gate(self, files):
        out = "".join(f"@@@FILE {p}\n{t}" for p, t in files.items()) + "@@@END\n"
        got = {}
        self.qt.no_autologin_config_gate(lambda cmd: (0, out), lambda n, ok, info="": got.update(ok=ok, info=info))
        return got

    def test_gate_fails_on_lightdm_autologin(self):
        g = self.run_gate({"/etc/lightdm/lightdm.conf.d/60-horcrux-autologin.conf": "[Seat:*]\nautologin-user=u\n"})
        self.assertFalse(g["ok"])
        self.assertIn("AUTOLOGIN CONFIGURED", g["info"])

    def test_gate_fails_on_gdm_autologin(self):
        self.assertFalse(self.run_gate({"/etc/gdm/custom.conf": "[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=u\n"})["ok"])

    def test_gate_passes_on_a_clean_system(self):
        g = self.run_gate({"/etc/lightdm/lightdm.conf": "[Seat:*]\n#autologin-user=\n", "/etc/gdm/custom.conf": "[daemon]\nWaylandEnable=true\n"})
        self.assertTrue(g["ok"], g)

    def test_gate_fails_when_nothing_could_be_read(self):
        got = {}
        self.qt.no_autologin_config_gate(lambda cmd: (255, ""), lambda n, ok, info="": got.update(ok=ok))
        self.assertFalse(got["ok"])

    def test_greeter_gate_fails_when_a_user_session_exists(self):
        calls = {"n": 0}

        def sh(cmd):
            if "@@S" in cmd:
                return 0, "@@S\n 1 1000 owner seat0 tty7 active no -\n@@P\n 1234 xfce4-session\n@@L\n1234\n1000\n"
            return 0, "DM\n  555 lightdm-gtk-greeter\n"
        got = {}
        self.qt.time.sleep = lambda s: None
        self.qt.greeter_no_session_gate(sh, lambda n, ok, info="": got.update(ok=ok, info=info), "owner", "lightdm", wait=1)
        self.assertFalse(got["ok"])

    def test_greeter_gate_passes_with_greeter_and_no_session(self):
        def sh(cmd):
            if "@@S" in cmd:
                return 0, "@@S\n c1 110 lightdm - - active no -\n@@P\n@@L\n1000\n"
            return 0, "DM\n  555 lightdm-gtk-greeter\n"
        got = {}
        self.qt.time.sleep = lambda s: None
        self.qt.greeter_no_session_gate(sh, lambda n, ok, info="": got.update(ok=ok, info=info), "owner", "lightdm", wait=1)
        self.assertTrue(got["ok"], got)


class HostnameRule(unittest.TestCase):
    def test_generic_image_default_is_refused(self):
        for h in ("surface-pro", "surface-pro.localdomain", "localhost", "ubuntu"):
            self.assertTrue(S.hostname_problem(h), h)
        for h in ("madammalkins", "gringotts", "hogshead.home.example"):
            self.assertIsNone(S.hostname_problem(h), h)

    def test_surface_auto_asks_and_confirms_the_hostname(self):
        t = open(os.path.join(SURFACE_PKG, "files", "root", "usr", "bin", "horcrux-surface-auto")).read()
        self.assertIn('hostname = "$HOST_NAME"', t)
        self.assertNotIn('hostname = "surface-pro"', t)
        self.assertIn("HOSTNAME-CONFIRMED", t)
        self.assertIn("HOSTNAME-REFUSED-GENERIC", t)
        self.assertIn("$PRIV/hostname", t)

    def test_no_machine_script_hardcodes_the_generic_name(self):
        for rel in ("etc/local.d/50-surface-tailscale.start", "usr/bin/surface-remote"):
            self.assertNotIn("surface-pro", open(os.path.join(SURFACE_PKG, "files", "root", rel)).read(), rel)


if __name__ == "__main__":
    unittest.main()
