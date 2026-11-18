"""sys-apps/horcrux-core host boot guards (-r7).

1. rc_sys="" lives in a package-owned /etc/rc.conf.d file: with rc_sys unset, OpenRC read a leftover /.dockerenv as "docker" and skipped
   devfs/udev/udev-trigger/root/localmount (read-only root, no NIC driver, only lo). The image-written rc_sys="" in /etc/rc.conf was lost when
   an OpenRC update's ._cfg0000_rc.conf was accepted.
2. Every non-boot-environment install refreshes grub.cfg after a kernel install/removal (installkernel is -grub).
3. The lab's network/boot-health watchdog runs even when NetworkManager did not start.
"""
import glob
import os
import re
import stat
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
CORE = os.path.join(REPO, "gentoo", "sys-apps", "horcrux-core")
TM = os.path.join(REPO, "gentoo", "app-misc", "horcrux-test-machine")
HOOK = os.path.join(CORE, "files", "horcrux-boot-entry")


def latest(d, pn):
    eb = sorted(glob.glob(os.path.join(d, pn + "-1-r*.ebuild")), key=lambda p: int(re.search(r"-r(\d+)\.ebuild$", p).group(1)))
    return eb[-1], open(eb[-1]).read()


@unittest.skipUnless(os.path.isdir(CORE), "Horcrux repo tree only")
class HostRcSys(unittest.TestCase):
    def test_rc_sys_is_set_uncommented(self):
        lines = [l.strip() for l in open(os.path.join(CORE, "files", "host-rc.conf")) if l.strip() and not l.lstrip().startswith("#")]
        self.assertEqual(lines, ['rc_sys=""'])

    def test_core_installs_it_in_rc_conf_d(self):
        path, e = latest(CORE, "horcrux-core")
        self.assertGreaterEqual(int(re.search(r"-r(\d+)\.ebuild$", path).group(1)), 7)
        i = e.index('newins "${FILESDIR}"/host-rc.conf horcrux-host.conf')
        self.assertLess(e.rindex("insinto /etc/rc.conf.d", 0, i), i)

    def test_postinst_removes_marker_only_on_a_booted_host(self):
        _, e = latest(CORE, "horcrux-core")
        post = e[e.index("pkg_postinst()"):]
        self.assertIn("/.dockerenv", post)
        self.assertIn("readlink /proc/1/exe", post)
        self.assertIn("init|openrc-init|systemd)", post)

    def test_images_still_strip_the_marker(self):
        self.assertIn('rm -f "$ROOT/.dockerenv"', open(os.path.join(REPO, "build", "live-iso", "common", "assemble-iso.sh")).read())


@unittest.skipUnless(os.path.isdir(CORE), "Horcrux repo tree only")
class KernelBootEntryHook(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.bin = os.path.join(self.d, "bin")
        os.makedirs(self.bin)
        self.cfg = os.path.join(self.d, "grub.cfg")
        open(self.cfg, "w").write("old\n")
        self.calls = os.path.join(self.d, "calls")
        self.fake("grub-mkconfig", 'echo "$@" >> %s; [ -n "$FAIL" ] && exit 1; o=$2; echo new > "$o"' % self.calls)
        self.conf = os.path.join(self.d, "conf")

    def fake(self, name, body):
        p = os.path.join(self.bin, name)
        open(p, "w").write("#!/bin/sh\n" + body + "\n")
        os.chmod(p, 0o755)

    def run_hook(self, extra="", env=None, arg="6.18.63-gentoo-dist"):
        open(self.conf, "w").write("GRUB_CFG=%s\nBE_CONF=%s/be.conf\nBE_HOOK=%s/90-horcrux-be\n%s" % (self.cfg, self.d, self.d, extra))
        e = dict(os.environ, PATH=self.bin + ":/usr/bin:/bin", HORCRUX_BOOT_ENTRY_CONF=self.conf)
        e.update(env or {})
        r = subprocess.run(["sh", HOOK, arg], env=e, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)     # never fails the kernel install
        return open(self.cfg).read()

    def test_refreshes_grub_cfg_without_boot_environments(self):
        self.assertEqual(self.run_hook(), "new\n")
        self.assertIn("-o %s.new" % self.cfg, open(self.calls).read())

    def test_leaves_boot_environment_installs_to_horcrux_be(self):
        open(os.path.join(self.d, "90-horcrux-be"), "w").close()
        self.assertEqual(self.run_hook(), "old\n")
        self.assertFalse(os.path.exists(self.calls))

    def test_failed_mkconfig_keeps_the_old_menu(self):
        self.assertEqual(self.run_hook(env={"FAIL": "1"}), "old\n")
        self.assertFalse(os.path.exists(self.cfg + ".new"))

    def test_off(self):
        self.assertEqual(self.run_hook("BOOT_ENTRY=off\n"), "old\n")

    def test_installed_for_install_and_removal(self):
        _, e = latest(CORE, "horcrux-core")
        for d in ("postinst.d", "postrm.d"):
            i = e.index("exeinto /etc/kernel/" + d)
            self.assertIn('newexe "${FILESDIR}"/horcrux-boot-entry 91-horcrux-boot-entry', e[i:i + 120])
        self.assertIn('newins "${FILESDIR}"/horcrux-boot-entry.confd horcrux-boot-entry', e)


@unittest.skipUnless(os.path.isdir(TM), "Horcrux repo tree only")
class NetwatchFallback(unittest.TestCase):
    def test_runs_without_networkmanager(self):
        initd = open(os.path.join(TM, "files", "horcrux-test-netwatch.initd")).read()
        code = "\n".join(l for l in initd.splitlines() if not l.lstrip().startswith("#"))
        self.assertNotRegex(code, r"need\s+NetworkManager")
        unit = open(os.path.join(TM, "files", "horcrux-test-netwatch.service")).read()
        self.assertNotIn("Requires=NetworkManager", unit)

    def test_coldplug_and_health(self):
        s = open(os.path.join(TM, "files", "horcrux-test-netwatch.sh")).read()
        subprocess.run(["bash", "-n", os.path.join(TM, "files", "horcrux-test-netwatch.sh")], check=True)
        for needle in ("udevadm trigger --type=devices --action=add", "rc-service udev-trigger start", "rc-service NetworkManager restart",
                       "systemctl restart NetworkManager.service", "openrc --sys", "HEALTH_FILE", ". /etc/conf.d/horcrux-test-netwatch"):
            self.assertIn(needle, s)
        _, e = latest(TM, "horcrux-test-machine")
        self.assertIn('newins "${FILESDIR}"/horcrux-test-netwatch.conf horcrux-test-netwatch', e)
        self.assertIn(">=sys-apps/horcrux-core-1-r7", e)
        self.assertNotIn("horcrux-test-kernel-hook", e)


if __name__ == "__main__":
    unittest.main()
