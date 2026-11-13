"""GNOME on OpenRC: session-scoped user services (gnome-base/gnome-session-openrc-49.2-r2::horcrux) and ssh logins without OpenRC user services
(sys-auth/horcrux-ssh-pam, pulled by sys-apps/horcrux-core on every edition). Static checks of the repo tree; the behaviour itself is proven by
gnome-session-lifecycle-test.py on an installed GNOME VM. Skipped in the vendored copy of common/ in Patronus."""
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
G = REPO / "gentoo"
LIVE = REPO / "build" / "live-iso"
GSO = G / "gnome-base" / "gnome-session-openrc"
PAM = G / "sys-auth" / "horcrux-ssh-pam"

SYSTEM_LOGIN = """auth\t\trequired\tpam_nologin.so
auth\t\tinclude\t\tsystem-auth
account\t\trequired\tpam_access.so
account\t\tinclude\t\tsystem-auth
password\tinclude\t\tsystem-auth
session\t\toptional\tpam_loginuid.so
session\t\trequired\tpam_env.so envfile=/etc/profile.env
session\t\tinclude\t\tsystem-auth
session\t\toptional\tpam_motd.so motd=/etc/motd
session\t\toptional\tpam_lastlog.so never showfailed
session\t\toptional\tpam_mail.so
-session\toptional\tpam_elogind.so
-session\toptional\tpam_openrc.so
"""


@unittest.skipUnless((REPO / "build" / "live-iso" / "common").is_dir(), "Horcrux repo tree only")
class SessionScoping(unittest.TestCase):
    def ebuild(self, d, glob):
        e = sorted(d.glob(glob))
        self.assertTrue(e, f"{d}/{glob}")
        return e[-1].read_text()

    def test_gnome_session_openrc_ships_all_fixes_and_never_edits_etc_portage(self):
        t = self.ebuild(GSO, "gnome-session-openrc-*.ebuild")
        for p in ("0001-gsd-meta-service-no-supervisor", "0002-gsd-audio-after-pipewire", "0003-shell-keep-user-bus",
                  "0004-session-scoped-user-services"):
            self.assertIn(p, t)
            self.assertTrue((GSO / "files" / f"{p}.patch").is_file(), p)
        self.assertNotRegex(t, r"\brm\b[^\n]*etc/portage", "the ebuild must not modify the admin's /etc/portage")
        self.assertIn("eapply_user", t)
        self.assertRegex((GSO / "Manifest").read_text(), r"(?m)^DIST gnome-session-openrc-49\.2\.tar\.gz ")
        p4 = (GSO / "files" / "0004-session-scoped-user-services.patch").read_text()
        for verb in ("teardown", "preclean", "guard"):
            self.assertIn(f"cmd_{verb}", p4)
        self.assertIn('run_helper ("teardown"', p4)
        self.assertIn('run_helper ("preclean"', p4)
        self.assertEqual(len(re.findall(r'^\+command_args="guard /usr/bin/gnome-shell', p4, re.M)), 4)

    def test_images_no_longer_ship_the_user_patches(self):
        self.assertFalse(list((LIVE / "project" / "variants").glob("*/portage/patches/gnome-base/gnome-session-openrc/*")))
        for df in (LIVE / "project" / "variants").glob("*/Dockerfile*"):
            self.assertNotRegex(df.read_text(), r"COPY \S*portage/patches/gnome-base", str(df))

    def test_gnome_defaults_requires_and_pins_the_overlay_package(self):
        t = self.ebuild(G / "gnome-extra" / "horcrux-gnome-defaults", "horcrux-gnome-defaults-*.ebuild")
        self.assertRegex(t, r"!systemd\? \( >=gnome-base/gnome-session-openrc-49\.2-r2 \)")
        self.assertRegex(t, r"(?m)^\t\tgnome-base/gnome-session-openrc::gentoo$")

    def test_core_pulls_ssh_pam_on_every_edition(self):
        self.assertRegex(self.ebuild(G / "sys-apps" / "horcrux-core", "horcrux-core-*.ebuild"), r"(?m)^\tsys-auth/horcrux-ssh-pam$")

    def test_sshd_stack_has_no_pam_openrc_and_matches_pambase(self):
        pam = (PAM / "files" / "sshd-horcrux.pam").read_text()
        code = re.sub(r"(?m)#.*$", "", pam)
        self.assertNotIn("pam_openrc", code)
        for line in ("auth       include   system-remote-login", "account    include   system-remote-login", "session    include   system-auth"):
            self.assertIn(line, pam)
        self.assertIn("PAMServiceName sshd-horcrux", (PAM / "files" / "10-horcrux-ssh-pam.conf").read_text())
        # the drop-in must sort before Gentoo's 9999999gentoo-pam.conf (sshd: first value wins)
        self.assertLess("10-horcrux-ssh-pam.conf", "9999999gentoo-pam.conf")
        chk = PAM / "files" / "horcrux-ssh-pam-check"
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "etc" / "pam.d").mkdir(parents=True)
            (Path(d) / "etc" / "pam.d" / "system-login").write_text(SYSTEM_LOGIN)
            (Path(d) / "etc" / "pam.d" / "sshd-horcrux").write_text(pam)
            r = subprocess.run(["sh", str(chk), d], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            # a session module pambase adds later is reported as drift
            (Path(d) / "etc" / "pam.d" / "system-login").write_text(SYSTEM_LOGIN + "session\toptional\tpam_new.so\n")
            r = subprocess.run(["sh", str(chk), d], capture_output=True, text=True)
            self.assertEqual(r.returncode, 1)
            self.assertIn("pam_new.so", r.stdout)

    def test_session_env_reaches_dbus_activation_environment(self):
        """HOGSMEADE 2026-11-13: the user bus's activation environment had no XDG_CURRENT_DESKTOP, xdg-desktop-portal picked no GNOME
        backend and there was no ScreenCast (OBS / browser screen sharing broken). The leader and gnome-session-init-* push it."""
        p4 = (GSO / "files" / "0004-session-scoped-user-services.patch").read_text()
        self.assertIn("cmd_activation_env", p4)
        self.assertIn('run_helper ("activation-env", NULL)', p4)
        # the leader pushes before it enters the gnome-session runlevel (before anything can activate the portal)
        self.assertLess(p4.index('run_helper ("activation-env", NULL)'), p4.index('"/sbin/openrc", "-U", "gnome-session"'))
        for kind in ("wayland", "x11"):
            self.assertRegex(p4, rf"diff --git a/data/openrc/gnome-session-init-{kind} [^\n]*\n(?:[^d][^\n]*\n)*?\+\t/usr/libexec/"
                                 r"gnome-session-openrc-helper activation-env")
        # run the helper's activation-env with a fake dbus-update-activation-environment
        lines, on = [], False
        for line in p4.splitlines():
            if line.startswith("+++ b/data/gnome-session-openrc-helper"):
                on = True
                continue
            if on and line.startswith("diff --git"):
                break
            if on and line.startswith("+"):
                lines.append(line[1:])
        helper = "\n".join(l for l in lines if not l.startswith("PATH=")) + "\n"
        with tempfile.TemporaryDirectory() as d:
            import socket
            rt, bin_ = Path(d, "rt"), Path(d, "bin")
            rt.mkdir(); bin_.mkdir()
            socks = []
            for n in ("bus", "wayland-0"):
                s = socket.socket(socket.AF_UNIX); s.bind(str(rt / n)); socks.append(s)
            (rt / "wayland-0.lock").write_text("")
            (bin_ / "dbus-update-activation-environment").write_text(f'#!/bin/sh\necho "$DBUS_SESSION_BUS_ADDRESS $*" > {d}/args\n')
            (bin_ / "dbus-update-activation-environment").chmod(0o755)
            (Path(d) / "helper").write_text(helper)
            env = {"PATH": f"{bin_}:{os.environ['PATH']}", "XDG_RUNTIME_DIR": str(rt), "XDG_SESSION_TYPE": "tty", "DESKTOP_SESSION": "gnome"}
            r = subprocess.run(["sh", str(Path(d) / "helper"), "activation-env"], capture_output=True, text=True, env=env, timeout=30)
            for s in socks:
                s.close()
            self.assertEqual(r.returncode, 0, r.stderr)
            args = Path(d, "args").read_text().split()
            self.assertEqual(args[0], f"unix:path={rt}/bus")
            for kv in ("XDG_CURRENT_DESKTOP=GNOME", "XDG_SESSION_TYPE=wayland", "XDG_SESSION_DESKTOP=gnome", "DESKTOP_SESSION=gnome",
                       "WAYLAND_DISPLAY=wayland-0"):
                self.assertIn(kv, args)

    def test_session_env_portal_restart_matches_the_real_process(self):
        # the kernel cuts comm to 15 characters ("xdg-desktop-por"): `pgrep -x xdg-desktop-portal` can never match
        for f in (G / "gnome-extra" / "horcrux-gnome-defaults" / "files" / "horcrux-session-env",):
            self.assertNotRegex(re.sub(r"(?m)^\s*#.*$", "", f.read_text()), r"pgrep[^\n]*-x xdg-desktop-portal")
        self.assertNotRegex((GSO / "files" / "0004-session-scoped-user-services.patch").read_text(), r"pgrep[^\n]*-x xdg-desktop-portal")


if __name__ == "__main__":
    unittest.main()
