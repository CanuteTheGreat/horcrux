"""horcrux-ha-fence (the manager-independent HA fence agent, j01743/j02106) must reach real installs: the manager tarball, the horcrux-bin package
(which installs it itself, so a tarball built before it existed still gets it), the source ebuilds, the installer's service list and every
image variant that clears the loose horcrux files before Portage owns them."""
import os
import re
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
HAVE = os.path.isdir(os.path.join(ROOT, "horcrux-api"))
rd = lambda *p: open(os.path.join(ROOT, *p)).read()
BIN = ("gentoo", "app-emulation", "horcrux-bin")


def bin_ebuilds():
    d = os.path.join(ROOT, *BIN)
    return sorted(f for f in os.listdir(d) if f.endswith(".ebuild"))


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class HaFencePackaging(unittest.TestCase):
    def test_files_in_the_package_are_the_deploy_units(self):
        self.assertEqual(rd(*BIN, "files", "horcrux-ha-fence.initd"), rd("deploy", "openrc", "horcrux-ha-fence"))
        self.assertEqual(rd(*BIN, "files", "horcrux-ha-fence.service"), rd("deploy", "systemd", "horcrux-ha-fence.service"))
        init = rd("deploy", "openrc", "horcrux-ha-fence")
        self.assertIn("ha-fence-agent", init)
        self.assertRegex(init, r"before horcrux-api")       # started before the manager, and without it (no `need horcrux-api`)
        self.assertNotRegex(init, r"need .*horcrux-api")

    def test_every_horcrux_bin_ebuild_installs_and_enables_it(self):
        names = bin_ebuilds()
        self.assertTrue(names)
        for n in names:
            e = rd(*BIN, n)
            self.assertIn('newinitd "${FILESDIR}"/horcrux-ha-fence.initd horcrux-ha-fence', e, n)
            self.assertIn("systemd_dounit", e, n)
            self.assertRegex(e, r"inherit pam systemd", n)
            self.assertIn("/etc/runlevels/default", e, n)
            self.assertRegex(e, r"-L \$\{rl\}/horcrux-api", n)     # only where the manager itself is enabled (server / cluster member)

    def test_stamped_ebuild_revision_was_bumped_and_the_bump_script_handles_it(self):
        stamped = [n for n in bin_ebuilds() if re.search(r"_p\d{12}", n)]
        self.assertTrue(any(re.search(r"_p\d{12}(-r\d+)?\.ebuild$", n) for n in stamped), stamped)
        sh = rd(*BIN, "bump-horcrux-bin.sh")
        self.assertIn("(-r[0-9]+)?", sh)

    def test_source_ebuilds_install_it(self):
        for v in ("0.1.0", "9999"):
            e = rd("gentoo", "app-emulation", "horcrux", f"horcrux-{v}.ebuild")
            self.assertIn('newinitd "${S}/deploy/openrc/horcrux-ha-fence" horcrux-ha-fence', e, v)
            self.assertIn('systemd_dounit "${S}/deploy/systemd/horcrux-ha-fence.service"', e, v)

    def test_manager_tarball_carries_it(self):
        app = rd("build", "live-iso", "project", "Dockerfile.app")
        self.assertIn("COPY deploy/openrc/horcrux-ha-fence /out/etc/init.d/horcrux-ha-fence", app)
        tar = [l for l in app.splitlines() if "tar --zstd -cf" in l or "etc/init.d/horcrux-guests etc/" in l]
        self.assertTrue(any("etc/init.d/horcrux-ha-fence" in l for l in app.splitlines() if "usr/bin/horcrux usr/bin/horcrux-api" in l), tar)
        self.assertRegex(app, r"chmod 0755 [^;]*etc/init\.d/horcrux-ha-fence")

    def test_installer_enables_it_with_the_manager(self):
        t = rd("build", "live-iso", "project", "overlay", "etc", "live-installer", "profile.toml")
        svcs = re.findall(r'"([^"]+)"', re.search(r"(?m)^services = \[(.*?)\]", t).group(1))
        self.assertIn("horcrux-ha-fence", svcs)
        self.assertLess(svcs.index("horcrux-ha-fence"), svcs.index("libvirtd"))

    def test_variants_that_clear_loose_manager_files_clear_it_too(self):
        for v in ("gringotts/Dockerfile", "hogshead/Dockerfile.part2", "surface/Dockerfile"):
            self.assertIn("/etc/init.d/horcrux-ha-fence", rd("build", "live-iso", "project", "variants", v), v)
        for v in ("gringotts/Dockerfile", "hogshead/Dockerfile.part2", "hogsmeade/Dockerfile.part2"):
            self.assertIn("/etc/runlevels/*/horcrux-ha-fence", rd("build", "live-iso", "project", "variants", v), v)
        self.assertIn("etc/init.d/horcrux-ha-fence", rd("build", "live-iso", "project", "variants", "gringotts", "image-files.exclude"))

    def test_role_checks_assert_the_installed_image_has_it(self):
        self.assertIn("horcrux-ha-fence", rd("gentoo", "ci", "portage-role-checks.sh"))


if __name__ == "__main__":
    unittest.main()
