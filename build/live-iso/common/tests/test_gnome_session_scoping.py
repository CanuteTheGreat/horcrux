"""GNOME on OpenRC: session-scoped user services (gnome-base/gnome-session-openrc-50.0-r2::horcrux) and ssh logins without OpenRC user services
(sys-auth/horcrux-ssh-pam, pulled by sys-apps/horcrux-core on every edition). Static checks of the repo tree; the behaviour itself is proven by
gnome-session-lifecycle-test.py on an installed GNOME VM. Skipped in the vendored copy of common/ in Patronus."""
import os
import re
import subprocess
import tempfile
import threading
import time
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
        p4 = (GSO / "files" / "0001-horcrux-session-scoped-user-services.patch").read_text()
        self.assertTrue((GSO / "files" / "0001-horcrux-session-scoped-user-services.patch").is_file())
        self.assertNotRegex(t, r"\brm\b[^\n]*etc/portage", "the ebuild must not modify the admin's /etc/portage")
        self.assertRegex((GSO / "Manifest").read_text(), r"(?m)^DIST gnome-session-openrc-50\.0\.tar\.gz ")
        for verb in ("teardown", "preclean", "guard"):
            self.assertIn(f"cmd_{verb}", p4)
        self.assertIn('run_helper ("teardown"', p4)
        self.assertIn('run_helper ("preclean"', p4)
        # gsd audio after PipeWire, gsd meta services unsupervised, shell on the user bus
        self.assertIn("want pipewire-pulse wireplumber", p4)
        self.assertIn("-supervisor=supervise-daemon", p4)
        self.assertIn('DBUS_SESSION_BUS_ADDRESS="unix:path=${XDG_RUNTIME_DIR}/bus"', p4)
        # upstream 50 is Wayland-only: gnome-shell and gnome-shell.gnome-login are the two guarded services
        self.assertEqual(len(re.findall(r'^\+command_args="guard /usr/bin/gnome-shell', p4, re.M)), 2)

    def test_greeter_shell_gets_gdm_session_mode(self):
        # GDM 50 starts the login screen as plain `gnome-session`; without GNOME_SHELL_SESSION_MODE=gdm gnome-shell is an empty
        # desktop of gdm-greeter and nobody can log in (the lab's greeter-login tests failed on exactly this)
        t = self.ebuild(GSO, "gnome-session-openrc-*.ebuild")
        p = (GSO / "files" / "0002-greeter-shell-session-mode.patch").read_text()
        self.assertIn("0002-greeter-shell-session-mode.patch", t)
        self.assertEqual(p.count("+\t\texport GNOME_SHELL_SESSION_MODE=gdm"), 2)
        self.assertIn('XDG_SESSION_CLASS}" = "greeter"', p)

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
        """owner-ws 2026-11-13: the user bus's activation environment had no XDG_CURRENT_DESKTOP, xdg-desktop-portal picked no GNOME
        backend and there was no ScreenCast (OBS / browser screen sharing broken). The leader and gnome-session-init-* push it."""
        p4 = (GSO / "files" / "0001-horcrux-session-scoped-user-services.patch").read_text()
        self.assertIn("cmd_activation_env", p4)
        self.assertIn('run_helper ("activation-env", NULL)', p4)
        # the leader pushes before it enters the gnome-session runlevel (before anything can activate the portal)
        self.assertLess(p4.index('run_helper ("activation-env", NULL)'), p4.index('"/sbin/openrc", "-U", "gnome-session"'))
        for svc in ("gnome-session-init", "gnome-shell"):
            self.assertRegex(p4, rf"diff --git a/data/openrc/{svc} [^\n]*\n(?:[^d][^\n]*\n)*?\+\t/usr/libexec/"
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

    def test_service_wrapper_loads_the_session_identity_before_gnome_session_service(self):
        p4 = (GSO / "files" / "0001-horcrux-session-scoped-user-services.patch").read_text()
        self.assertIn("cmd_session_env", p4)
        self.assertIn('eval "$(gnome-session-openrc-helper session-env', p4)
        # run the real subcommand: leader env in, export lines out
        lines, on = [], False
        for line in p4.splitlines():
            if line.startswith("+++ b/data/gnome-session-openrc-helper"):
                on = True
                continue
            if on and line.startswith("diff --git"):
                break
            if on and line.startswith("+") and not line.startswith("+#"):
                lines.append(line[1:])
        helper = "\n".join(l for l in lines if not l.startswith("PATH=")) + "\n"
        with tempfile.TemporaryDirectory() as d:
            Path(d, "helper").write_text(helper)
            sleeper = subprocess.Popen(["sleep", "20"], env={"XDG_CURRENT_DESKTOP": "GNOME", "DESKTOP_SESSION": "gnome", "XDG_SESSION_TYPE": "tty"})
            try:
                Path(d, "gnome-session-leader.pid").write_text(str(sleeper.pid))
                r = subprocess.run(["sh", str(Path(d) / "helper"), "session-env"], capture_output=True, text=True, timeout=30,
                                   env={"PATH": os.environ["PATH"], "XDG_RUNTIME_DIR": d})
            finally:
                sleeper.kill()
            self.assertEqual(r.returncode, 0, r.stderr)
            for kv in ("XDG_CURRENT_DESKTOP='GNOME'", "XDG_SESSION_DESKTOP='gnome'", "DESKTOP_SESSION='gnome'", "XDG_SESSION_TYPE='wayland'"):
                self.assertIn(f"export {kv}", r.stdout)

    def test_session_env_portal_restart_matches_the_real_process(self):
        # the kernel cuts comm to 15 characters ("xdg-desktop-por"): `pgrep -x xdg-desktop-portal` can never match
        for f in (G / "gnome-extra" / "horcrux-gnome-defaults" / "files" / "horcrux-session-env",):
            self.assertNotRegex(re.sub(r"(?m)^\s*#.*$", "", f.read_text()), r"pgrep[^\n]*-x xdg-desktop-portal")
        self.assertNotRegex((GSO / "files" / "0001-horcrux-session-scoped-user-services.patch").read_text(), r"pgrep[^\n]*-x xdg-desktop-portal")

    def _patched_tree(self, d):
        """data/ of the session package after 0001-0003, without the upstream tarball: the helper is a new file in 0001, 0003 patches it and
        adds the Xwayland hook (the gsd-xsettings hunk needs the upstream file and is checked as text)."""
        p1 = (GSO / "files" / "0001-horcrux-session-scoped-user-services.patch").read_text()
        p3 = (GSO / "files" / "0003-xwayland-gsd-xsettings.patch").read_text()
        lines, on = [], False
        for line in p1.splitlines():
            if line.startswith("+++ b/data/gnome-session-openrc-helper"):
                on = True
                continue
            if on and line.startswith("diff --git"):
                break
            if on and line.startswith("+"):
                lines.append(line[1:])
        Path(d, "data", "Xwayland-session.d").mkdir(parents=True)
        Path(d, "data", "gnome-session-openrc-helper").write_text("\n".join(lines) + "\n")
        keep = [s for s in re.split(r"(?m)^(?=diff --git )", p3) if s.startswith("diff --git a/data/gnome-session-openrc-helper ")
                or s.startswith("diff --git a/data/Xwayland-session.d/")]
        self.assertEqual(len(keep), 2, "0003 must patch the helper and add the Xwayland-session.d hook")
        r = subprocess.run(["patch", "-p1", "-s", "-d", d], input="".join(keep), capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        return Path(d, "data")

    def test_x11_apps_get_the_ui_scale_gsd_xsettings_starts_with_xwayland(self):
        """owner-ws 2026-11-23: Steam, the Nextcloud client and terminator were tiny at 133 %. mutter 50 always scales Xwayland natively and
        X11 apps learn the factor only from gsd-xsettings (Xft.dpi, Gdk/WindowScalingFactor), which nothing started on OpenRC."""
        t = self.ebuild(GSO, "gnome-session-openrc-*.ebuild")
        self.assertIn('"${FILESDIR}"/0003-xwayland-gsd-xsettings.patch', t)
        p3 = (GSO / "files" / "0003-xwayland-gsd-xsettings.patch").read_text()
        # the service: no hard-coded display, guarded like gnome-shell, display from the helper
        svc = re.search(r"(?s)diff --git a/data/openrc/gsd-xsettings .*?(?=^diff --git |\Z)", p3, re.M).group(0)
        self.assertIn("-DISPLAY=:2", svc)
        self.assertNotRegex(svc, r"(?m)^\+\s*DISPLAY=:")
        self.assertIn('+command_args="guard /usr/libexec/gsd-xsettings"', svc)
        self.assertIn("+\td=$(/usr/libexec/gnome-session-openrc-helper x-display)", svc)
        self.assertRegex(p3, r"install_dir: '/etc/xdg/Xwayland-session\.d'")
        # per-app scale variables are not the fix (fixed at app start, double-scale on top of Xft.dpi)
        for v in ("GDK_SCALE=", "QT_SCALE_FACTOR=", "STEAM_FORCE_DESKTOPUI_SCALING="):
            self.assertNotRegex(p3, rf"(?m)^\+[^#\n]*\b{v}")
        with tempfile.TemporaryDirectory() as d:
            data = self._patched_tree(d)
            hook = data / "Xwayland-session.d" / "20-gnome-session-openrc-xsettings"
            helper = data / "gnome-session-openrc-helper"
            self.assertIn("[ -d /run/systemd/system ] && exit 0", hook.read_text())
            text = helper.read_text().replace("/usr/libexec/gnome-session-openrc-helper", str(helper))
            helper.write_text("\n".join(l for l in text.splitlines() if not l.startswith("PATH=")) + "\n")
            rt, bin_ = Path(d, "rt"), Path(d, "bin")
            rt.mkdir(); bin_.mkdir()
            fakes = {
                "rc-service": f'#!/bin/sh\necho "$*" >> {d}/rc\n[ "$3" = status ] && exit 3\nexit 0\n',
                "gdbus": "#!/bin/sh\necho \"(<uint32 2>,)\"\n",
                "gsettings": "#!/bin/sh\necho 1.25\n",
                "xrdb": f'#!/bin/sh\necho "DISPLAY=$DISPLAY" > {d}/xrdb.env\nprintf "Xft.antialias:\\t1\\nXft.dpi:\\t240\\n"\n',
            }
            for n, body in fakes.items():
                (bin_ / n).write_text(body); (bin_ / n).chmod(0o755)
            env = {"PATH": f"{bin_}:{os.environ['PATH']}", "XDG_RUNTIME_DIR": str(rt), "XDG_SESSION_CLASS": "user"}
            # an X display lock of a live process of ours (what mutter writes for Xwayland's display)
            n = 7000 + os.getpid() % 2000
            lock = Path(f"/tmp/.X{n}-lock")
            if lock.exists():
                self.skipTest(f"{lock} exists")
            lock.write_text(f"{os.getpid():>10}\n")
            try:
                # mutter runs the hook with DISPLAY and XAUTHORITY set
                xa = rt / ".mutter-Xwaylandauth.TEST"
                xa.write_text("cookie")
                r = subprocess.run(["sh", str(helper), "xsettings-start", f":{n}"], capture_output=True, text=True,
                                   env=dict(env, XAUTHORITY=str(xa)), timeout=30)
                self.assertEqual(r.returncode, 0, r.stderr)
                # the helper detaches rc-service: wait on a monotonic deadline with a sleep nothing can patch away (another test
                # replacing the shared time.sleep once turned this into 50 instant polls and a lost race)
                deadline, tick = time.monotonic() + 15, threading.Event()
                while time.monotonic() < deadline:
                    if Path(d, "rc").exists() and "start" in Path(d, "rc").read_text().split():
                        break
                    tick.wait(0.1)
                self.assertEqual((rt / "gnome-session-openrc-xwayland-display").read_text().strip(), f":{n}")
                self.assertIn("-U gsd-xsettings start", Path(d, "rc").read_text())
                r = subprocess.run(["sh", str(helper), "x-display"], capture_output=True, text=True, env=env, timeout=30)
                self.assertEqual(r.stdout.strip(), f":{n}")
                # gsd-xsettings cannot open Xwayland's display without its cookie (it crashed in a respawn loop without it)
                r = subprocess.run(["sh", str(helper), "x-authority"], capture_output=True, text=True, env=env, timeout=30)
                self.assertEqual(r.stdout.strip(), str(xa))
                svc = (GSO / "files" / "0003-xwayland-gsd-xsettings.patch").read_text()
                self.assertIn("+\td=$(/usr/libexec/gnome-session-openrc-helper x-authority)", svc)
                self.assertIn('+\texport XAUTHORITY="$d"', svc)
                # the status check: Xft.dpi 240 = 96 x 2 x 1.25 passes, no gsd-xsettings process of ours fails
                r = subprocess.run(["sh", str(helper), "x11-scale-status"], capture_output=True, text=True, env=env, timeout=30)
                self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
                self.assertIn(f"OK Xft.dpi=240 on :{n}", r.stdout)
                self.assertIn(f"FAIL gsd-xsettings is not running for :{n}", r.stdout)
                self.assertEqual(Path(d, "xrdb.env").read_text().strip(), f"DISPLAY=:{n}")
                # not an X display / the login screen: nothing is started
                Path(d, "rc").unlink()
                for arg, cls in ((":0;rm -rf /", "user"), ("wayland-0", "user"), (f":{n}", "greeter")):
                    r = subprocess.run(["sh", str(helper), "xsettings-start", arg], capture_output=True, text=True,
                                       env=dict(env, XDG_SESSION_CLASS=cls), timeout=30)
                    self.assertEqual(r.returncode, 0)
                threading.Event().wait(0.5)
                self.assertFalse(Path(d, "rc").exists())
            finally:
                lock.unlink()


if __name__ == "__main__":
    unittest.main()
