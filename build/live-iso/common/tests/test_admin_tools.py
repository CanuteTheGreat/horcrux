"""app-portage/gentoolkit (equery, eclean, revdep-rebuild, euse) is on every Gentoo install (owner 2026-11-12).

One place covers every image: common/packages.txt is emerged (and so lands in @world) by common/Dockerfile.rootfs, the base of every
Horcrux and Patronus variant; the installed system is checked for `equery` by qemu-test.py through headless_gates.admin_tools_gate.
Runs identically in the vendored copy of common/ in Patronus (the t2 check skips there)."""
import os
import re
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.abspath(os.path.join(HERE, ".."))
T2 = os.path.abspath(os.path.join(COMMON, "..", "..", "..", "gentoo", "t2", "live", "packages.txt"))
ALLOW = os.path.abspath(os.path.join(COMMON, "..", "..", "..", "gentoo", "app-misc", "horcrux-test-machine", "files", "world.allowlist"))


def atoms(path):
    return [l.split("#")[0].split()[-1] for l in open(path) if l.split("#")[0].strip()]


class GentoolkitEverywhere(unittest.TestCase):
    def test_common_base_list_has_gentoolkit(self):
        self.assertIn("app-portage/gentoolkit", atoms(os.path.join(COMMON, "packages.txt")))

    def test_not_arch_gated_or_late(self):
        # a '<arch>? ' prefix would drop it on the other architecture; the --noreplace base emerge puts it in @world
        line = [l for l in open(os.path.join(COMMON, "packages.txt")) if "app-portage/gentoolkit" in l and not l.lstrip().startswith("#")]
        self.assertEqual(len(line), 1)
        self.assertRegex(line[0].strip(), r"^app-portage/gentoolkit$")

    def test_rootfs_emerges_common_list_into_world(self):
        d = open(os.path.join(COMMON, "Dockerfile.rootfs")).read()
        self.assertRegex(d, r"emerge --noreplace \$\(live-pkglist /tmp/packages\.common\.txt")      # no --oneshot: gentoolkit is selected into world

    def test_vdb_prune_keeps_gentoolkit(self):
        s = open(os.path.join(COMMON, "vdb-prune.py")).read()
        self.assertNotIn("gentoolkit", s)       # nothing prunes it by name
        self.assertNotRegex(s, r"app-portage/\*")

    def test_qemu_gate_runs_on_every_installed_system(self):
        q = open(os.path.join(COMMON, "qemu-test.py")).read()
        self.assertIn("HG.admin_tools_gate", q)
        m = re.search(r"def ssh_checks\(self\):.*?(?=\n    def |\nclass |\ndef |\Z)", q, re.S)
        self.assertIn("HG.admin_tools_gate", m.group(0))   # ssh_checks runs for server, gnome-light, hogsmeade and Patronus alike

    @unittest.skipUnless(os.path.exists(T2), "Horcrux repo tree only")
    def test_hogshead_t2_live_has_gentoolkit(self):
        self.assertIn("app-portage/gentoolkit", atoms(T2))

    @unittest.skipUnless(os.path.exists(ALLOW), "Horcrux repo tree only")
    def test_test_machine_world_allowlist_accepts_gentoolkit(self):
        # horcrux-test-machine-check fails the lab for any world entry not on the allowlist: gentoolkit is in @world on every machine now
        import fnmatch
        pats = [l.strip() for l in open(ALLOW) if l.strip() and not l.startswith("#")]
        self.assertTrue(any(fnmatch.fnmatch("app-portage/gentoolkit", p) for p in pats))
        deny = os.path.join(os.path.dirname(ALLOW), "personal-apps.denylist")
        self.assertFalse(any(fnmatch.fnmatch("app-portage/gentoolkit", l.split("#")[0].strip()) for l in open(deny) if l.split("#")[0].strip()))


if __name__ == "__main__":
    unittest.main()
