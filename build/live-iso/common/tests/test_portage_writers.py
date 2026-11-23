"""Every writer of /etc/portage config sets explicit modes, whatever umask it runs under (the tests all run with umask 077).

Background: an administrator ran `emerge -p` as himself and got "Permission denied: /etc/portage/package.use/<file>" because a file
there had been written 0600 root. Rule: /etc/portage (except gnupg/) is world-readable, directories traversable, in every writer:
  - the installer's Installer.write (liveinst/portageperms.py)
  - the machine-profile engine (horcrux_profile.py Sys.write; its own copy of the rule)
  - the image build gate normalize-portage-perms.sh (assemble-iso.sh runs it over every variant, private ones included)
  - packaging (ebuilds/Dockerfiles): gentoo/ci/check-portage-perms.py
Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import importlib.util
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.join(HERE, "..")
LI = os.path.join(COMMON, "overlay", "usr", "lib", "live-installer")
sys.path.insert(0, LI)
from liveinst import portageperms as PP  # noqa: E402
from liveinst import system  # noqa: E402

_spec = importlib.util.spec_from_file_location("horcrux_profile", os.path.join(LI, "horcrux_profile.py"))
E = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(E)

REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
CHECKER = os.path.join(REPO, "gentoo", "ci", "check-portage-perms.py")
NORMALIZE = os.path.join(COMMON, "normalize-portage-perms.sh")


def mode(p):
    return stat.S_IMODE(os.lstat(p).st_mode)


class Umask077(unittest.TestCase):
    def setUp(self):
        self.old = os.umask(0o077)
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.addCleanup(os.umask, self.old)


class InstallerWrite(Umask077):
    class Fake:
        write = system.Installer.write

    def inst(self):
        f = self.Fake()
        f.target = os.path.join(self.tmp, "target")
        os.makedirs(f.target, mode=0o700)
        return f

    def test_portage_file_and_dirs_are_public(self):
        i = self.inst()
        i.write("/etc/portage/package.use/00-live-installer", "x\n")
        for rel in ("etc", "etc/portage", "etc/portage/package.use"):
            self.assertEqual(mode(f"{i.target}/{rel}"), 0o755, rel)
        self.assertEqual(mode(f"{i.target}/etc/portage/package.use/00-live-installer"), 0o644)

    def test_append_keeps_public(self):
        i = self.inst()
        i.write("/etc/portage/package.use/a", "x\n")
        i.write("/etc/portage/package.use/a", "y\n", append=True)
        self.assertEqual(mode(f"{i.target}/etc/portage/package.use/a"), 0o644)

    def test_restrictive_request_for_portage_config_is_raised(self):
        i = self.inst()
        i.write("/etc/portage/repos.conf/x.conf", "x\n", 0o600)
        self.assertEqual(mode(f"{i.target}/etc/portage/repos.conf/x.conf"), 0o644)
        i.write("/etc/portage/bashrc", "x\n", 0o775)           # never group/world-writable
        self.assertEqual(mode(f"{i.target}/etc/portage/bashrc"), 0o755)

    def test_secrets_stay_secret(self):
        i = self.inst()
        i.write("/etc/portage/gnupg/key", "k\n", 0o600)
        i.write("/etc/horcrux/secret", "s\n", 0o600)
        self.assertEqual(mode(f"{i.target}/etc/portage/gnupg/key"), 0o600)
        self.assertEqual(mode(f"{i.target}/etc/horcrux/secret"), 0o600)
        self.assertEqual(mode(f"{i.target}/etc/portage/gnupg"), 0o700)       # not a config dir: untouched by the rule

    def test_existing_private_dirs_are_not_widened_silently_outside_portage(self):
        i = self.inst()
        os.makedirs(f"{i.target}/var/lib/x", mode=0o700)
        i.write("/var/lib/x/f", "x\n", 0o600)
        self.assertEqual(mode(f"{i.target}/var/lib/x"), 0o700)


class EngineWrite(Umask077):
    def setUp(self):
        super().setUp()
        self.base = os.path.join(self.tmp, "etc", "portage")
        os.makedirs(os.path.dirname(self.base), mode=0o700)
        old = E.PORTAGE_DIR
        E.PORTAGE_DIR = self.base
        self.addCleanup(setattr, E, "PORTAGE_DIR", old)
        self.sy = E.Sys()

    def test_files_and_dirs(self):
        p = f"{self.base}/package.use/horcrux-profile"
        self.sy.write(p, "a b\n")
        self.assertEqual(mode(p), 0o644)
        self.assertEqual(mode(f"{self.base}/package.use"), 0o755)
        self.assertEqual(mode(self.base), 0o755)

    def test_profile_file_entry_with_restrictive_mode_is_raised_inside_portage_only(self):
        a, b = f"{self.base}/make.conf.d/x", os.path.join(self.tmp, "etc", "other", "x")
        self.sy.write(a, "x\n", 0o600)
        self.sy.write(b, "x\n", 0o600)
        self.assertEqual(mode(a), 0o644)
        self.assertEqual(mode(b), 0o600)

    def test_gnupg_is_left_alone(self):
        p = f"{self.base}/gnupg/trustdb"
        self.sy.write(p, "x\n", 0o600)
        self.assertEqual(mode(p), 0o600)
        self.assertEqual(mode(f"{self.base}/gnupg"), 0o700)

    def test_public_dirs_for_repo_snapshots(self):
        d = os.path.join(self.tmp, "var", "db", "repos")
        E.ensure_dirs(d, public=True)
        self.assertEqual(mode(d), 0o755)
        self.assertEqual(mode(os.path.join(self.tmp, "var", "db")), 0o755)

    def test_both_copies_of_the_rule_agree(self):
        E.PORTAGE_DIR = "/etc/portage"
        for rel, m in (("package.use/x", 0o600), ("gnupg/k", 0o600), ("gnupg", 0o700), ("bashrc", 0o700), ("x", 0o666), ("../shadow", 0o600)):
            self.assertEqual(E.portage_config_mode(f"/etc/portage/{rel}", m), PP.config_mode(f"/etc/portage/{rel}", m), rel)
        self.assertEqual(E.portage_config_path("/etc/portage-other/x"), PP.is_config("/etc/portage-other/x"))
        self.assertFalse(PP.is_config("/etc/portage-other/x"))


class NormalizeScript(Umask077):
    def build(self):
        r = os.path.join(self.tmp, "root")
        os.makedirs(r + "/etc/portage/package.use", mode=0o700)
        os.makedirs(r + "/etc/portage/gnupg", mode=0o700)
        os.makedirs(r + "/etc/portage/postsync.d", mode=0o700)
        os.chmod(r + "/etc", 0o700)
        for rel, m in (("package.use/horcrux-gnome", 0o600), ("make.conf", 0o640), ("gnupg/secring", 0o600), ("postsync.d/60-x", 0o700),
                       ("bashrc", 0o777)):
            with open(f"{r}/etc/portage/{rel}", "w") as f:
                f.write("x\n")
            os.chmod(f"{r}/etc/portage/{rel}", m)
        outside = os.path.join(self.tmp, "outside")
        with open(outside, "w") as f:
            f.write("x")
        os.chmod(outside, 0o600)
        os.symlink(outside, r + "/etc/portage/make.profile")
        return r, outside

    def run_it(self, r):
        return subprocess.run(["sh", NORMALIZE, r], capture_output=True, text=True)

    def test_fixes_everything_except_gnupg_and_symlink_targets(self):
        r, outside = self.build()
        p = self.run_it(r)
        self.assertEqual(p.returncode, 0, p.stderr)
        e = r + "/etc/portage"
        self.assertEqual(mode(r + "/etc"), 0o755)
        for d in ("", "/package.use", "/postsync.d"):
            self.assertEqual(mode(e + d), 0o755, d)
        self.assertEqual(mode(e + "/package.use/horcrux-gnome"), 0o644)
        self.assertEqual(mode(e + "/make.conf"), 0o644)
        self.assertEqual(mode(e + "/postsync.d/60-x"), 0o755)          # executables stay executable
        self.assertEqual(mode(e + "/bashrc"), 0o755)                   # x bit kept, group/world write removed
        self.assertEqual(mode(e + "/gnupg"), 0o700)
        self.assertEqual(mode(e + "/gnupg/secring"), 0o600)
        self.assertEqual(mode(outside), 0o600)                          # a symlink must not widen its target
        self.assertIn("fixed", p.stdout)

    def test_idempotent_and_missing_dir_ok(self):
        r, _ = self.build()
        self.run_it(r)
        again = self.run_it(r)
        self.assertEqual(again.returncode, 0)
        self.assertIn("fixed 0 entries", again.stdout)
        empty = os.path.join(self.tmp, "empty")
        os.makedirs(empty)
        self.assertEqual(self.run_it(empty).returncode, 0)

    def test_wired_into_image_assembly(self):
        asm = open(os.path.join(COMMON, "assemble-iso.sh")).read()
        self.assertIn("normalize-portage-perms.sh", asm)
        self.assertLess(asm.index("normalize-portage-perms.sh"), asm.index("mksquashfs"))
        tools = open(os.path.join(COMMON, "Dockerfile.tools")).read()
        self.assertIn("common/normalize-portage-perms.sh", tools)


@unittest.skipUnless(os.path.isfile(CHECKER), "this tree has no gentoo/ci (Patronus)")
class StaticChecker(Umask077):
    def load(self):
        spec = importlib.util.spec_from_file_location("cpp", CHECKER)
        m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(m)
        return m

    def test_flags_each_way_to_break_portage_config(self):
        m = self.load()
        bad = {
            "a.ebuild": "src_install() {\n\tinsopts -m0600\n\tinsinto /etc/portage/package.use\n\tdoins x\n}\n",
            "b.ebuild": "src_install() {\n\tinsinto /etc/portage/package.use\n\tdoins x\n\tfperms 0600 /etc/portage/package.use/x\n}\n",
            "c.sh": "chmod 0640 /etc/portage/make.conf\n",
            "Dockerfile": "RUN install -D -m600 x /etc/portage/package.use/x\n",
            "d.sh": "umask 077\necho x > /etc/portage/package.use/x\n",
            "e.sh": "chmod go-r /etc/portage/package.use/x\n",
        }
        for name, text in bad.items():
            self.assertTrue(m.check_text(name, text), name)

    def test_accepts_good_and_exempt_forms(self):
        m = self.load()
        good = {
            "a.ebuild": "src_install() {\n\tinsopts -m0600\n\tinsinto /etc/ctg\n\tdoins k\n\tinsopts -m0644\n\tinsinto /etc/portage/package.use\n\tdoins x\n}\n",
            "b.ebuild": "src_install() {\n\tinsopts -m0600\n}\nother() {\n\tinsinto /etc/portage/package.use\n\tdoins x\n}\n",
            "c.sh": "chmod 0600 /etc/portage/gnupg/secring\n( umask 077; head -c8 /dev/urandom > /etc/ctg/key )\n",
            "d.sh": "chmod 0644 /etc/portage/make.conf\nchmod 0600 /etc/foo  # /etc/portage\n",
            "e.sh": "chmod 0600 /etc/portage/x  # portage-perms: allow test fixture\n",
        }
        for name, text in good.items():
            self.assertEqual(m.check_text(name, text), [], name)

    def test_repo_is_clean(self):
        p = subprocess.run([sys.executable, CHECKER, os.path.join(REPO, "gentoo"), os.path.join(REPO, "build")], capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)


if __name__ == "__main__":
    unittest.main()
