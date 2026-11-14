"""liveinst.initsys: the installer's OpenRC/systemd abstraction (owner rule 2026-11-13: both init systems are first-class)."""
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.join(HERE, "..")
sys.path.insert(0, os.path.join(COMMON, "overlay/usr/lib/live-installer"))
from liveinst import initsys  # noqa: E402


def mkroot(init):
    d = tempfile.mkdtemp()
    os.makedirs(os.path.join(d, "usr/sbin"))
    os.makedirs(os.path.join(d, "usr/lib/systemd/system"))
    os.makedirs(os.path.join(d, "etc/init.d"))
    if init == "systemd":
        os.makedirs(os.path.join(d, "usr/lib/systemd"), exist_ok=True)
        open(os.path.join(d, "usr/lib/systemd/systemd"), "w").close()
        os.symlink("../lib/systemd/systemd", os.path.join(d, "usr/sbin/init"))
    else:
        open(os.path.join(d, "usr/sbin/init"), "w").close()
        open(os.path.join(d, "usr/sbin/openrc"), "w").close()
    return d


class Rec:
    def __init__(self, target):
        self.target = target
        self.cmds = []

    def run(self, cmd, chroot=False, check=True, **kw):
        self.cmds.append(cmd)


class TargetInit(unittest.TestCase):
    def test_detects_both(self):
        self.assertEqual(initsys.target_init(mkroot("systemd")), "systemd")
        self.assertEqual(initsys.target_init(mkroot("openrc")), "openrc")

    def test_enable_openrc_is_rc_update(self):
        i = Rec(mkroot("openrc"))
        initsys.enable(i, "zfs-import", "boot")
        initsys.disable(i, "sshd")
        self.assertEqual(i.cmds, [["rc-update", "add", "zfs-import", "boot"], ["rc-update", "del", "sshd", "default"]])

    def test_enable_systemd_maps_names(self):
        i = Rec(mkroot("systemd"))
        for s in ("zfs-import", "zfs-mount", "zfs-zed", "sshd", "horcrux-api", "swclock", "elogind", "zram-swap"):
            initsys.enable(i, s, "boot")
        self.assertEqual(i.cmds, [
            ["systemctl", "enable", "zfs-import-cache.service", "zfs-import.target"],
            ["systemctl", "enable", "zfs-mount.service", "zfs.target"],
            ["systemctl", "enable", "zfs-zed.service"],
            ["systemctl", "enable", "sshd.service"],
            ["systemctl", "enable", "horcrux-api.service"],
            ["systemctl", "enable", "zram-swap.service"],
        ])

    def test_display_manager_maps_to_the_installed_dm(self):
        r = mkroot("systemd")
        i = Rec(r)
        self.assertFalse(initsys.exists("display-manager", r))
        open(os.path.join(r, "usr/lib/systemd/system/gdm.service"), "w").close()
        self.assertTrue(initsys.exists("display-manager", r))
        initsys.enable(i, "display-manager")
        initsys.enable(i, "cupsd")
        self.assertEqual(i.cmds, [["systemctl", "enable", "gdm.service"], ["systemctl", "enable", "cups.service"]])

    def test_exists(self):
        r = mkroot("systemd")
        self.assertFalse(initsys.exists("horcrux-api", r))
        open(os.path.join(r, "usr/lib/systemd/system/horcrux-api.service"), "w").close()
        self.assertTrue(initsys.exists("horcrux-api", r))
        o = mkroot("openrc")
        open(os.path.join(o, "etc/init.d/horcrux-api"), "w").close()
        self.assertTrue(initsys.exists("horcrux-api", o))
        self.assertFalse(initsys.exists("elogind", r))          # systemd does that job itself

    def test_control(self):
        self.assertEqual(initsys.control_cmd("openrc", "restart", "dhcpcd"), ["rc-service", "dhcpcd", "restart"])
        self.assertEqual(initsys.control_cmd("systemd", "restart", "dhcpcd"), ["systemctl", "restart", "dhcpcd.service"])
        self.assertEqual(initsys.control_cmd("systemd", "status", "sshd"), ["systemctl", "is-active", "--quiet", "sshd.service"])
        with self.assertRaises(ValueError):
            initsys.control_cmd("runit", "start", "x")

    def test_tty1_systemd_dropin_and_inittab(self):
        r = mkroot("systemd")
        p = initsys.set_tty1(r, "--noclear -n -l /usr/libexec/console-menu-session")
        t = open(p).read()
        self.assertIn("ExecStart=\nExecStart=-/sbin/agetty --noclear -n -l /usr/libexec/console-menu-session %I $TERM", t)
        o = mkroot("openrc")
        os.makedirs(os.path.join(o, "etc"), exist_ok=True)
        open(os.path.join(o, "etc/inittab"), "w").write("c1:12345:respawn:/sbin/agetty 38400 tty1 linux\n")
        initsys.set_tty1(o, "--autologin bob --noclear")
        self.assertIn("c1:12345:respawn:/sbin/agetty --autologin bob --noclear 38400 tty1 linux", open(os.path.join(o, "etc/inittab")).read())

    def test_autologin_audit_sees_getty_dropins(self):
        from liveinst import autologin
        r = mkroot("systemd")
        initsys.set_tty1(r, "--autologin bob --noclear")
        self.assertTrue(autologin.audit(r))
        autologin.scrub(r)
        self.assertEqual(autologin.audit(r), [])


class LiveUnits(unittest.TestCase):
    """every live OpenRC service has a systemd twin that runs the SAME script through live-openrc-compat"""
    UNITS = os.path.join(COMMON, "overlay/usr/lib/systemd/system")

    def test_every_live_script_has_a_unit(self):
        for s in ("live-forensic", "live-installer", "live-persist", "live-product", "live-serial", "zram-swap"):
            u = open(os.path.join(self.UNITS, s + ".service")).read()
            self.assertIn(f"/usr/libexec/live-openrc-compat /etc/init.d/{s} ", u, s)
            self.assertIn("[Install]", u, s)

    def test_compat_shim_runs_start_and_exec(self):
        shim = os.path.join(COMMON, "overlay/usr/libexec/live-openrc-compat")
        d = tempfile.mkdtemp()
        sc = os.path.join(d, "demo")
        open(sc, "w").write('#!/sbin/openrc-run\ndepend() { need net; }\nstart() { einfo "hello $X"; return 3; }\n'
                            'command=/bin/echo\nstart_pre() { command_args="exec-ok $X"; }\n')
        r = subprocess.run(["sh", shim, sc, "start"], capture_output=True, text=True, env={"X": "world", "PATH": os.environ["PATH"]})
        self.assertEqual(r.returncode, 3)
        self.assertIn("hello world", r.stdout)
        r = subprocess.run(["sh", shim, sc, "exec"], capture_output=True, text=True, env={"X": "w", "PATH": os.environ["PATH"]})
        self.assertEqual(r.stdout.strip(), "exec-ok w")

    def test_live_tty1_dropin(self):
        t = open(os.path.join(COMMON, "overlay/etc/systemd/system/getty@tty1.service.d/00-live.conf")).read()
        self.assertIn("ExecStart=-/usr/libexec/live-getty-tty1", t)
        self.assertIn("After=live-installer.service", t)

    def test_setup_rootfs_masks_the_stock_serial_getty(self):
        s = open(os.path.join(COMMON, "setup-rootfs.sh")).read()
        self.assertIn("systemctl mask serial-getty@.service", s)
        self.assertIn('systemd|systemd-*|dracut-systemd) echo "$n"', s)       # every systemd* dracut module omitted
        self.assertIn('omit_dracutmodules+=" %s "', s)
        self.assertIn('add_dracutmodules+=" horcrux-scripted-init "', s)      # no systemctl in the script initrd (zfs-load-key.sh)
        self.assertIn('rm -f "$ROOT/.dockerenv"', open(os.path.join(COMMON, "assemble-iso.sh")).read())


class DhcpcdDns(unittest.TestCase):
    def test_enter_hook_bypasses_resolvectl_without_resolved(self):
        h = os.path.join(COMMON, "overlay/etc/dhcpcd.enter-hook")
        t = open(h).read()
        self.assertIn("/run/systemd/resolve/io.systemd.Resolve", t)
        self.assertIn("/usr/bin/resolvectl", t)
        # an OpenRC image (no resolvectl): the variable stays unset, dhcpcd's own default applies
        r = subprocess.run(["sh", "-c", f". {h}; echo \"[${{resolvconf:-}}]\""], capture_output=True, text=True)
        self.assertEqual(r.stdout.strip(), "[]")


class PkgList(unittest.TestCase):
    def test_conditions(self):
        d = tempfile.mkdtemp()
        f = os.path.join(d, "l.txt")
        open(f, "w").write("a/b\n# c\nopenrc? h/i\nsystemd? j/k\nnosucharch? x/y\n")
        sh = os.path.join(COMMON, "pkglist.sh")
        env = dict(os.environ)
        env["INIT"] = "systemd"
        self.assertEqual(subprocess.run(["sh", sh, f], capture_output=True, text=True, env=env).stdout.split(), ["a/b", "j/k"])
        env["INIT"] = "openrc"
        self.assertEqual(subprocess.run(["sh", sh, f], capture_output=True, text=True, env=env).stdout.split(), ["a/b", "h/i"])


class EarlyBootUnits(unittest.TestCase):
    def test_units_ordered_before_early_targets_drop_default_dependencies(self):
        # Before=swap.target/sysinit.target/local-fs.target with the default dependencies (After=basic.target) is an ordering cycle: systemd
        # breaks it by dropping a random job every boot (zram-swap.service dropped tmp.mount; every PrivateTmp= service then failed)
        d = os.path.join(COMMON, "overlay/usr/lib/systemd/system")
        early = {"swap.target", "sysinit.target", "local-fs.target", "local-fs-pre.target"}
        for f in sorted(os.listdir(d)):
            if not f.endswith(".service"):
                continue
            t = open(os.path.join(d, f)).read()
            before = {w for line in t.splitlines() if line.startswith("Before=") for w in line[7:].split()}
            if before & early:
                self.assertIn("DefaultDependencies=no", t, f"{f} is ordered before {sorted(before & early)} but keeps the default dependencies")
            after = {w for line in t.splitlines() if line.startswith("After=") for w in line[6:].split()}
            if "swap.target" in before:
                # tmp.mount (tmpfs) is After=swap.target and local-fs.target After=tmp.mount: a swap provider after local-fs.target is a cycle
                self.assertFalse(after & {"local-fs.target", "tmp.mount"}, f"{f} is Before=swap.target but After={sorted(after & {'local-fs.target', 'tmp.mount'})}")


class ConsoleStatus(unittest.TestCase):
    def test_systemd_status_lines_do_not_overwrite_the_console_menu(self):
        # parity with OpenRC: after the getty is up, systemd prints only failures on the console (the menu got scrolled away otherwise)
        f = os.path.join(COMMON, "overlay/usr/lib/systemd/system.conf.d/50-console-menu.conf")
        t = open(f).read()
        self.assertIn("[Manager]", t)
        self.assertIn("ShowStatus=error", [l.strip() for l in t.splitlines()])


if __name__ == "__main__":
    unittest.main()
