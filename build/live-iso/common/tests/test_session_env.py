"""horcrux-session-env (gnome-extra/horcrux-gnome-defaults): the GNOME session environment must reach the user bus's ACTIVATION environment on OpenRC,
else D-Bus-activated apps (GNOME Settings: "only supported under GNOME and Unity") get a bare environment.

Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus. The real-dbus-daemon test needs dbus-daemon and skips without it.
"""
import os
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
PKG = os.path.join(REPO, "gentoo", "gnome-extra", "horcrux-gnome-defaults")
SCRIPT = os.path.join(PKG, "files", "horcrux-session-env")
HAVE = os.path.isfile(SCRIPT)

SHELL_ENV = {"XDG_CURRENT_DESKTOP": "GNOME", "XDG_SESSION_DESKTOP": "gnome", "XDG_SESSION_TYPE": "wayland", "DESKTOP_SESSION": "gnome",
             "XDG_DATA_DIRS": "/usr/local/share:/usr/share:/usr/share/gdm", "SECRET_TOKEN": "must-not-leak"}


def fake_proc(root, pid, env):
    os.makedirs(os.path.join(root, str(pid)))
    with open(os.path.join(root, str(pid), "environ"), "wb") as f:
        f.write(b"".join(f"{k}={v}".encode() + b"\0" for k, v in env.items()))


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class SessionEnv(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp(prefix="hxse")
        self.addCleanup(shutil.rmtree, self.d, True)
        self.rt = os.path.join(self.d, "run")
        os.makedirs(self.rt)
        self.proc = os.path.join(self.d, "proc")
        fake_proc(self.proc, 4242, SHELL_ENV)
        with open(os.path.join(self.rt, "gnome-shell-wayland.pid"), "w") as f:
            f.write("4242\n")
        open(os.path.join(self.rt, "wayland-0"), "w").close()

    def run_script(self, *args, extra=None, update=None):
        env = {"PATH": os.environ["PATH"], "XDG_RUNTIME_DIR": self.rt, "HX_PROC": self.proc, "HX_NO_PORTAL": "1", "HX_UID": str(os.getuid())}
        if update:
            env["HX_UPDATE_CMD"] = update
        env.update(extra or {})
        return subprocess.run(["bash", SCRIPT, *args], env=env, capture_output=True, text=True, timeout=30)

    def test_takes_values_from_the_running_shell_when_the_caller_has_none(self):
        r = self.run_script("--print")
        self.assertEqual(r.returncode, 0, r.stderr)
        got = dict(l.split("=", 1) for l in r.stdout.splitlines())
        self.assertEqual(got["XDG_CURRENT_DESKTOP"], "GNOME")
        self.assertEqual(got["XDG_SESSION_TYPE"], "wayland")
        self.assertEqual(got["XDG_DATA_DIRS"], SHELL_ENV["XDG_DATA_DIRS"])
        self.assertEqual(got["WAYLAND_DISPLAY"], "wayland-0", "falls back to the live socket")

    def test_only_the_listed_variables_are_exported(self):
        r = self.run_script("--print", extra={"SECRET_TOKEN2": "x"})
        self.assertNotIn("SECRET_TOKEN", r.stdout)

    def test_callers_environment_wins(self):
        r = self.run_script("--print", extra={"XDG_CURRENT_DESKTOP": "ubuntu:GNOME"})
        self.assertIn("XDG_CURRENT_DESKTOP=ubuntu:GNOME", r.stdout.splitlines())

    def test_defaults_to_gnome_without_any_source(self):
        shutil.rmtree(self.proc)
        os.makedirs(self.proc)
        os.remove(os.path.join(self.rt, "gnome-shell-wayland.pid"))
        r = self.run_script("--print")
        self.assertIn("XDG_CURRENT_DESKTOP=GNOME", r.stdout.splitlines())

    def test_calls_dbus_update_activation_environment_on_the_user_bus(self):
        open(os.path.join(self.d, "fake.log"), "w").close()
        fake = os.path.join(self.d, "fake-update")
        with open(fake, "w") as f:
            f.write(f'#!/bin/sh\necho "BUS=$DBUS_SESSION_BUS_ADDRESS" > {self.d}/fake.log\nfor a in "$@"; do echo "$a" >> {self.d}/fake.log; done\n')
        os.chmod(fake, 0o755)
        sock = os.path.join(self.rt, "bus")
        import socket
        s = socket.socket(socket.AF_UNIX)
        s.bind(sock)
        self.addCleanup(s.close)
        r = self.run_script(update=fake, extra={"DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/dbus-gdm-private"})
        self.assertEqual(r.returncode, 0, r.stderr)
        log = Path(self.d, "fake.log").read_text()
        self.assertIn(f"BUS=unix:path={sock}", log, "must use the OpenRC user bus, not gdm's private one")
        self.assertIn("XDG_CURRENT_DESKTOP=GNOME", log)

    def test_fails_loudly_when_the_update_fails(self):
        r = self.run_script(update="false")
        self.assertEqual(r.returncode, 1)

    @unittest.skipUnless(shutil.which("dbus-daemon") and shutil.which("dbus-update-activation-environment") and shutil.which("dbus-send"), "needs dbus")
    def test_real_dbus_activated_service_sees_the_session_environment(self):
        """a service activated by a real dbus-daemon started WITHOUT the desktop variables (as the OpenRC `dbus` user service) gets them only after the script ran"""
        svc = os.path.join(self.d, "services")
        os.makedirs(svc)
        out = os.path.join(self.d, "activated.env")
        probe = os.path.join(self.d, "probe.sh")
        with open(probe, "w") as f:
            f.write(f"#!/bin/sh\nenv > {out}\n")
        os.chmod(probe, 0o755)
        for n in ("One", "Two"):      # the name is never claimed, so a second activation of the same name would join the still-pending first one
            with open(os.path.join(svc, f"org.horcrux.EnvProbe{n}.service"), "w") as f:
                f.write(f"[D-BUS Service]\nName=org.horcrux.EnvProbe{n}\nExec={probe}\n")
        conf = os.path.join(self.d, "bus.conf")
        with open(conf, "w") as f:
            f.write(f"""<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path={self.rt}/bus</listen><servicedir>{svc}</servicedir>
<policy context="default"><allow send_destination="*" eavesdrop="true"/><allow receive_sender="*" eavesdrop="true"/><allow receive_requested_reply="true"/><allow own="*"/></policy></busconfig>""")
        clean = {"PATH": os.environ["PATH"], "HOME": self.d}
        bus = subprocess.Popen(["dbus-daemon", "--config-file", conf, "--nofork"], env=clean, stderr=subprocess.DEVNULL)
        self.addCleanup(bus.wait)
        self.addCleanup(bus.terminate)
        addr = f"unix:path={self.rt}/bus"
        for _ in range(200):
            if os.path.exists(os.path.join(self.rt, "bus")) or bus.poll() is not None:
                break
            time.sleep(0.1)
        self.assertIsNone(bus.poll(), "dbus-daemon exited at once")

        def activate(name):
            if os.path.exists(out):
                os.remove(out)
            for _ in range(50):      # the listening socket can appear a moment after its file
                r = subprocess.run(["dbus-send", "--session", "--print-reply", "--reply-timeout=3000", "--dest=org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.StartServiceByName",
                                f"string:org.horcrux.EnvProbe{name}", "uint32:0"], env={**clean, "DBUS_SESSION_BUS_ADDRESS": addr}, capture_output=True, timeout=30)
                if b"Failed to connect" not in r.stderr:
                    break
                time.sleep(0.2)
            for _ in range(100):
                if os.path.exists(out) and os.path.getsize(out):
                    break
                time.sleep(0.1)
            self.last_send = (r.stdout + r.stderr).decode(errors="replace")
            return Path(out).read_text() if os.path.exists(out) else ""

        before = activate("One")
        self.assertTrue(before, "the probe service was never activated: %s (bus alive %s, run dir %s)" % (self.last_send, bus.poll() is None, os.listdir(self.rt)))
        self.assertNotIn("XDG_CURRENT_DESKTOP=", before, "precondition: the bare bus lacks the session environment")
        r = self.run_script(extra={"DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent", "PATH": os.environ["PATH"]})
        self.assertEqual(r.returncode, 0, r.stderr)
        after = activate("Two")
        self.assertIn("XDG_CURRENT_DESKTOP=GNOME", after.splitlines())
        self.assertIn("XDG_SESSION_TYPE=wayland", after.splitlines())
        self.assertNotIn("SECRET_TOKEN", after)


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class Packaging(unittest.TestCase):
    def test_ebuild_installs_the_script_and_autostart_entry(self):
        ebs = sorted(f for f in os.listdir(PKG) if f.endswith(".ebuild"))
        self.assertEqual(len(ebs), 1, ebs)
        t = Path(PKG, ebs[0]).read_text()
        self.assertIn('doexe "${FILESDIR}"/horcrux-session-env', t)
        self.assertIn('doins "${FILESDIR}"/horcrux-session-env.desktop', t)
        # dobin/doins above switch exeinto/insinto: the install dirs must be set again right before
        i = t.index('doexe "${FILESDIR}"/horcrux-session-env')
        self.assertIn("exeinto /usr/libexec", t[i - 40:i])
        j = t.index('doins "${FILESDIR}"/horcrux-session-env.desktop')
        self.assertIn("insinto /etc/xdg/autostart", t[j - 40:j])

    def test_autostart_entry_targets_the_installed_script(self):
        d = Path(PKG, "files", "horcrux-session-env.desktop").read_text()
        self.assertIn("Exec=/usr/libexec/horcrux-session-env", d)
        self.assertNotIn("Autostart-Phase=Initialization", d, "Initialization runs BEFORE the shell: too early for the environment")


if __name__ == "__main__":
    unittest.main()
