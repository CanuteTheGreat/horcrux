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


GRUBD = os.path.join(CORE, "files", "grub-unattended")
UUID = "0f1e2d3c-4b5a-4968-8776-a5b4c3d2e1f0"


def linux_entries(kvers, submenu=False):
    """what Gentoo's /etc/grub.d/10_linux prints (structure only)"""
    def entry(kv, kind, ind=""):
        title = f"Horcrux GNU/Linux, with Linux {kv}" + (" (recovery mode)" if kind == "recovery" else "")
        return (f"{ind}menuentry '{title}' --class horcrux $menuentry_id_option 'gnulinux-{kv}-{kind}-{UUID}' {{\n"
                f"{ind}\tload_video\n{ind}\tif [ x$grub_platform = xefi ]; then\n{ind}\t\tinsmod gzio\n{ind}\tfi\n"
                f"{ind}\tlinux\t/vmlinuz-{kv} root=UUID={UUID} ro\n{ind}}}\n")
    out = ""
    if submenu:
        out += f"menuentry 'Horcrux GNU/Linux' --class horcrux $menuentry_id_option 'gnulinux-simple-{UUID}' {{\n\tlinux\t/vmlinuz-{kvers[0]}\n}}\n"
        out += f"submenu 'Advanced options for Horcrux GNU/Linux' $menuentry_id_option 'gnulinux-advanced-{UUID}' {{\n"
        for kv in kvers:
            out += entry(kv, "advanced", "\t") + entry(kv, "recovery", "\t")
        return out + "}\n"
    for kv in kvers:
        out += entry(kv, "advanced") + entry(kv, "recovery")
    return out


@unittest.skipUnless(os.path.isdir(CORE), "Horcrux repo tree only")
class GrubUnattended(unittest.TestCase):
    """/etc/grub.d/11_horcrux_unattended: a grub-mkconfig menu never waits for a key when a kernel fails to load. GRUB unsets `timeout`
    once an automatic boot comes back to the menu; `fallback` takes entry NUMBERS only. The QEMU proof (the image's own grub-mkconfig,
    booted with no key pressed) is build/live-iso/common/grub-autoboot-test.py --mode mkconfig."""

    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.gd = os.path.join(self.d, "grub.d")
        os.makedirs(self.gd)
        self.conf = os.path.join(self.d, "conf")
        open(self.conf, "w").write("")
        self.script("00_header", "cat <<'EOF'\nset default=\"0\"\nfunction load_video {\n  insmod all_video\n}\nEOF\n")
        self.marker = os.path.join(self.d, "later-ran")
        self.script("30_later", f"touch {self.marker}\necho \"menuentry 'later' {{\"\necho '}}'\n")
        import shutil
        shutil.copy(GRUBD, os.path.join(self.gd, "11_horcrux_unattended"))
        os.chmod(os.path.join(self.gd, "11_horcrux_unattended"), 0o755)

    def script(self, name, body):
        p = os.path.join(self.gd, name)
        open(p, "w").write("#!/bin/sh\n" + body)
        os.chmod(p, 0o755)

    def linux(self, kvers, submenu=False):
        out = os.path.join(self.d, "linux.out")
        open(out, "w").write(linux_entries(kvers, submenu))
        self.script("10_linux", f"cat {out}\necho 'Found linux image' >&2\n")

    def run_it(self, conf=""):
        open(self.conf, "w").write(conf)
        r = subprocess.run(["sh", os.path.join(self.gd, "11_horcrux_unattended")], capture_output=True, text=True,
                           env=dict(os.environ, HORCRUX_BOOT_ENTRY_CONF=self.conf))
        self.assertEqual(r.returncode, 0, r.stderr)    # never fails grub-mkconfig
        return r.stdout, r.stderr

    def test_flat_menu_falls_back_to_other_kernels_by_number_then_retry(self):
        self.linux(["6.18.70", "6.18.64", "6.12.50"])
        out, _ = self.run_it()
        # entries: 0 6.18.70, 1 its recovery, 2 6.18.64, 3 recovery, 4 6.12.50, 5 recovery, 6 = the retry entry
        adds = re.findall(r"(?m)^hx_fallback_add (\d+) '([^']*)' '([^']*)'$", out)
        self.assertEqual([(n, i) for n, i, _t in adds], [("0", f"gnulinux-6.18.70-advanced-{UUID}"), ("2", f"gnulinux-6.18.64-advanced-{UUID}"),
                                                          ("4", f"gnulinux-6.12.50-advanced-{UUID}")])
        self.assertEqual(adds[1][2], "Horcrux GNU/Linux, with Linux 6.18.64")
        self.assertIn('set fallback="${hx_fallback}6"', out)
        self.assertNotIn("recovery", "".join(i for _n, i, _t in adds), "a recovery entry stops at a single-user prompt: never a fallback")
        self.assertFalse(os.path.exists(self.marker), "scripts after this one must not be run (their entries come after ours)")

    def test_default_is_left_out_at_boot_time(self):
        self.linux(["6.18.70"])
        out, _ = self.run_it()
        # GRUB_DEFAULT may be a number, an id, a title, `saved` or next_entry: compared in GRUB, not at grub-mkconfig time
        fn = out[out.index("function hx_fallback_add"):out.index("set hx_default")]
        for arg in ("$1", "$2", "$3"):
            self.assertIn(f'if [ "x${{hx_default}}" = "x{arg}" ] ; then return ; fi', fn)
        self.assertIn('set hx_default="${default}"', out)
        self.assertIn('if [ -z "${hx_default}" ] ; then set hx_default=0 ; fi', out)

    def test_retry_entry_reloads_then_restarts(self):
        self.linux(["6.18.70", "6.18.64"])
        out, _ = self.run_it()
        body = out[out.index("menuentry 'Start the boot menu again"):]
        self.assertIn("$menuentry_id_option 'horcrux-retry'", body)
        self.assertIn('if [ "${hx_tries}" = "xxx" ] ; then', body)
        self.assertIn("reboot", body)
        self.assertIn("export hx_tries", body)
        self.assertIn('configfile "${prefix}/grub.cfg"', body)  # not ${config_file}: empty inside an entry
        out, _ = self.run_it("RETRY_ROUNDS=5\nRETRY_CONFIG='(hd0,gpt2)/grub/grub.cfg'\n")
        self.assertIn('= "xxxxx" ]', out)
        self.assertIn('configfile "(hd0,gpt2)/grub/grub.cfg"', out)
        out, _ = self.run_it("RETRY_ROUNDS=0\n")
        self.assertIn('= "xxx" ]', out, "an invalid round count falls back to 3")

    def test_submenu_keeps_retry_and_warns(self):
        self.linux(["6.18.70", "6.18.64"], submenu=True)
        out, err = self.run_it()
        adds = re.findall(r"(?m)^hx_fallback_add (\d+) ", out)
        self.assertEqual(adds, ["0"])                  # only the top-level simple entry; never the submenu (GRUB would stop in it)
        self.assertIn('set fallback="${hx_fallback}2"', out)
        self.assertIn("GRUB_DISABLE_SUBMENU=y", err)

    def test_off_and_no_kernels(self):
        self.linux(["6.18.70"])
        out, _ = self.run_it("UNATTENDED=off\n")
        self.assertEqual(out, "")
        self.linux([])
        out, _ = self.run_it()
        self.assertEqual(out, "")

    def test_grub_script_check_accepts_it(self):
        import shutil
        if not shutil.which("grub-script-check"):
            self.skipTest("grub-script-check not installed")
        self.linux(["6.18.70", "6.18.64"])
        out, _ = self.run_it()
        f = os.path.join(self.d, "grub.cfg")
        open(f, "w").write(linux_entries(["6.18.70", "6.18.64"]) + out)
        r = subprocess.run(["grub-script-check", f], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_packaged_and_existing_installs_switch_to_a_flat_menu(self):
        path, e = latest(CORE, "horcrux-core")
        self.assertGreaterEqual(int(re.search(r"-r(\d+)\.ebuild$", path).group(1)), 9)
        i = e.index("exeinto /etc/grub.d")
        self.assertIn('newexe "${FILESDIR}"/grub-unattended 11_horcrux_unattended', e[i:i + 100])
        post = e[e.index("pkg_postinst()"):]
        blk = post[post.index("GRUB_DISABLE_SUBMENU"):]
        # added only when /etc/default/grub does not set it either way; never on boot-environment installs or image builds
        self.assertIn("grep -qE '^[[:space:]]*(export[[:space:]]+)?GRUB_DISABLE_SUBMENU='", post)
        self.assertIn("-z ${ROOT} && ! -e /etc/horcrux/be.conf && ! -e /etc/kernel/postinst.d/90-horcrux-be", post)
        self.assertIn("91-horcrux-boot-entry refresh", blk)
        subprocess.run(["sh", "-n", GRUBD], check=True)
        installer = open(os.path.join(REPO, "build", "live-iso", "common", "overlay", "usr", "lib", "live-installer", "liveinst", "system.py")).read()
        self.assertIn("'GRUB_DISABLE_SUBMENU=y'", installer)

    def test_hook_refresh_mode(self):
        d = tempfile.mkdtemp()
        b = os.path.join(d, "bin")
        os.makedirs(b)
        open(os.path.join(b, "grub-mkconfig"), "w").write("#!/bin/sh\necho new > \"$2\"\n")
        os.chmod(os.path.join(b, "grub-mkconfig"), 0o755)
        cfg = os.path.join(d, "grub.cfg")
        open(cfg, "w").write("old\n")
        conf = os.path.join(d, "conf")
        open(conf, "w").write(f"GRUB_CFG={cfg}\nBE_CONF={d}/be.conf\nBE_HOOK={d}/90-horcrux-be\n")
        r = subprocess.run(["sh", HOOK, "refresh"], env=dict(os.environ, PATH=b + ":/usr/bin:/bin", HORCRUX_BOOT_ENTRY_CONF=conf),
                           capture_output=True, text=True)
        self.assertEqual(open(cfg).read(), "new\n")
        self.assertIn("boot menu refreshed (", r.stderr)


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
