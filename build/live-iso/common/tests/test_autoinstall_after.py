"""<proj>.autoinstall.after=reboot (lab / unattended fleets): the unattended installer must restart the machine instead of powering it off, even when
the install failed or refused, never via /sbin/reboot (the live medium may be the disk that was just erased), and must keep the default (poweroff)."""
import os, re, subprocess, unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INIT = os.path.join(ROOT, "overlay/etc/init.d/live-installer")


class AutoinstallAfter(unittest.TestCase):
    def setUp(self):
        self.s = open(INIT).read()

    def test_reads_the_kernel_arg_and_suppresses_poweroff_only_for_reboot(self):
        self.assertIn('after=$(cmdarg "$p" autoinstall.after)', self.s)
        self.assertRegex(self.s, r'\[ "\$after" = reboot \] && nohalt=--no-halt')
        self.assertIn("live-installer --answers auto $nohalt", self.s)

    def test_restart_is_sysrq_not_a_binary_from_the_erased_medium(self):
        blk = self.s.split('if [ "$after" = reboot ]; then', 1)[1].split("\n\t\tfi\n", 1)[0]
        self.assertIn("echo b > /proc/sysrq-trigger", blk)
        self.assertNotRegex(blk, r"(^|\s)(/sbin/)?reboot(\s|$)")
        self.assertIn("sync", blk)

    def test_reboot_happens_before_the_interactive_fallback(self):
        self.assertLess(self.s.index('if [ "$after" = reboot ]'), self.s.index("INSTALLER-FALLBACK-INTERACTIVE"))

    def test_default_behaviour_unchanged(self):
        self.assertIn("grep -q 'ANSWER-FILE\\|No answer file' /var/log/autoinstall.log", self.s)

    def test_log_copy_only_to_a_vfat_uuid_that_still_exists(self):
        self.assertIn('mount -t vfat "/dev/disk/by-uuid/$lg"', self.s)

    def test_shell_syntax(self):
        r = subprocess.run(["sh", "-n", INIT], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)


if __name__ == "__main__":
    unittest.main()
