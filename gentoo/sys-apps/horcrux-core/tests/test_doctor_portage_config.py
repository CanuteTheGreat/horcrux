"""horcrux-doctor portage-config: files and directories under /etc/portage that members of the portage group cannot reach.

The check models the kernel (path walked component by component, one permission class chosen per object), so these tests pin the
cases where a plain `mode & 0o044` test lies: 0604 root:portage (the group class wins: members CANNOT read), 0640 root:root (the
group bit is set, but the file's group is not portage), a 0700 directory above a 0644 file, restrictive umasks, symlinks, secrets.

Unprivileged: the tests create the files themselves, so "portage" is a gid chosen per case: the test user's own gid (files whose group
IS portage) or a gid nobody has (files whose group is not portage). The principal is a member that owns nothing.
"""
import importlib.machinery
import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
CORE = os.path.join(REPO, "gentoo", "sys-apps", "horcrux-core")
DOCTOR = os.path.join(CORE, "files", "horcrux-doctor")

_loader = importlib.machinery.SourceFileLoader("horcrux_doctor", DOCTOR)
_spec = importlib.util.spec_from_loader("horcrux_doctor", _loader)
hd = importlib.util.module_from_spec(_spec)
_loader.exec_module(hd)
hd.OWNER_UID = os.getuid()  # the files these tests create are "root's" for --fix purposes

MYGID = os.getgid()
OTHERGID = MYGID + 4242  # nobody's group: files are never in it


def mk(path, mode, content="x\n", d=False):
    """create with an explicit mode regardless of the umask"""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if d:
        os.makedirs(path, exist_ok=True)
    else:
        with open(path, "w") as f:
            f.write(content)
    os.chmod(path, mode)
    return path


@unittest.skipUnless(os.path.isfile(DOCTOR), "Horcrux repo tree only")
class Base(unittest.TestCase):
    portage_gid = OTHERGID

    def setUp(self):
        self.old_umask = os.umask(0o022)
        self.tmp = tempfile.mkdtemp()
        self.root = os.path.join(self.tmp, "root")
        self.cfg = os.path.join(self.root, "etc", "portage")
        os.makedirs(self.cfg)
        os.chmod(os.path.join(self.root, "etc"), 0o755)
        os.chmod(self.root, 0o755)
        os.chmod(self.cfg, 0o755)
        self.set_group(self.portage_gid)

    def tearDown(self):
        os.umask(self.old_umask)
        subprocess.run(["chmod", "-R", "u+rwx", self.tmp], check=False)
        subprocess.run(["rm", "-rf", self.tmp], check=False)

    def set_group(self, gid):
        self.portage_gid = gid
        with open(os.path.join(self.root, "etc", "group"), "w") as f:
            f.write("root:x:0:\nportage:x:%d:\n" % gid)

    def check(self, **kw):
        return hd.check_portage_config(self.root, kw.pop("gid", self.portage_gid), kw.pop("uid", None), **kw)

    def paths(self, findings):
        return sorted(os.path.relpath(f["path"], "/etc/portage") if f["path"].startswith("/etc/portage") else f["path"]
                      for f in findings)

    def mode(self, rel):
        return stat.S_IMODE(os.lstat(os.path.join(self.cfg, rel)).st_mode)

    def cli(self, *args, umask=None, env_extra=None):
        env = dict(os.environ, HX_DOCTOR_OWNER_UID=str(os.getuid()), HX_DOCTOR_STATE_DIR=os.path.join(self.tmp, "state"))
        env.update(env_extra or {})
        pre = (lambda: os.umask(umask)) if umask is not None else None
        return subprocess.run([sys.executable, DOCTOR, "--root", self.root, "--gid", str(self.portage_gid), "--force-nonroot", *args],
                              capture_output=True, text=True, env=env, preexec_fn=pre)


class ModeIsNotAccess(Base):
    def test_other_readable_file_is_fine_when_group_is_not_portage(self):
        mk(os.path.join(self.cfg, "package.use", "a"), 0o644)
        f, _ = self.check()
        self.assertEqual(f, [])

    def test_group_bit_does_not_help_when_group_is_not_portage(self):
        # 0640 root:root-style: the group bit is set but members of portage are "other" for this file
        mk(os.path.join(self.cfg, "package.use", "a"), 0o640)
        f, _ = self.check()
        self.assertEqual(self.paths(f), ["package.use/a"])
        self.assertEqual(f[0]["problem"], "unreadable-file")
        self.assertIn("judged by the other bits", f[0]["detail"])

    def test_owner_only_file_flagged(self):
        mk(os.path.join(self.cfg, "package.use", "horcrux-x"), 0o600)
        f, _ = self.check()
        self.assertEqual(self.paths(f), ["package.use/horcrux-x"])

    def test_other_bit_does_not_help_when_group_is_portage(self):
        self.set_group(MYGID)
        mk(os.path.join(self.cfg, "a"), 0o604)  # misleading: world-readable but the group class (---) wins
        f, _ = self.check()
        self.assertEqual(self.paths(f), ["a"])
        self.assertIn("judged by the group bits", f[0]["detail"])

    def test_group_readable_with_portage_group_is_fine(self):
        self.set_group(MYGID)
        mk(os.path.join(self.cfg, "a"), 0o640)
        mk(os.path.join(self.cfg, "b"), 0o644)
        f, _ = self.check()
        self.assertEqual(f, [])

    def test_write_only_or_exec_bits_are_not_read(self):
        mk(os.path.join(self.cfg, "a"), 0o733)
        f, _ = self.check()
        self.assertEqual(self.paths(f), ["a"])

    def test_owner_class_applies_to_a_member_who_owns_the_file(self):
        mk(os.path.join(self.cfg, "a"), 0o600)
        f, _ = self.check(uid=os.getuid())
        self.assertEqual(f, [])  # owner bits decide for the owner

    def test_missing_group_is_reported_not_crashed(self):
        os.remove(os.path.join(self.root, "etc", "group"))
        f, _ = hd.check_portage_config(self.root, None, None)
        self.assertEqual(f[0]["problem"], "no-portage-group")


class Traversal(Base):
    def test_unsearchable_directory_hides_readable_file(self):
        mk(os.path.join(self.cfg, "repos.conf"), 0o700, d=True)
        mk(os.path.join(self.cfg, "repos.conf", "gentoo.conf"), 0o644)
        f, _ = self.check()
        self.assertEqual(self.paths(f), ["repos.conf"])  # reported once, as the blocker, not per file
        self.assertEqual(f[0]["problem"], "unreachable-directory")
        self.assertTrue(f[0]["fixable"])

    def test_directory_without_read_bit_cannot_be_listed(self):
        mk(os.path.join(self.cfg, "package.use"), 0o711, d=True)
        mk(os.path.join(self.cfg, "package.use", "a"), 0o644)
        f, _ = self.check()
        self.assertEqual(self.paths(f), ["package.use"])
        self.assertIn("cannot be listed", f[0]["detail"])

    def test_ancestor_above_config_dir_is_reported_but_never_fixable(self):
        mk(os.path.join(self.cfg, "a"), 0o644)
        os.chmod(os.path.join(self.root, "etc"), 0o700)
        f, _ = self.check()
        self.assertTrue(f)
        self.assertTrue(all(not x["fixable"] for x in f))
        self.assertEqual(f[0]["target"], "/etc")
        self.assertEqual(f[0]["problem"], "unreachable-directory")

    def test_group_class_applies_to_directories_too(self):
        self.set_group(MYGID)
        mk(os.path.join(self.cfg, "d"), 0o705, d=True)  # world r-x but group ---: members are shut out
        mk(os.path.join(self.cfg, "d", "f"), 0o644)
        f, _ = self.check()
        self.assertEqual(self.paths(f), ["d"])

    def test_config_dir_itself(self):
        os.chmod(self.cfg, 0o700)
        mk(os.path.join(self.cfg, "a"), 0o644)
        f, _ = self.check()
        self.assertEqual(f[0]["target"], "/etc/portage")
        self.assertTrue(f[0]["fixable"])


class Umask(Base):
    def test_restrictive_umask_install_is_detected_and_fixed_to_644_755(self):
        os.umask(0o077)
        mk(os.path.join(self.cfg, "package.use", "horcrux-gnome"), 0o600)
        d = os.path.join(self.cfg, "package.accept_keywords")
        os.mkdir(d)  # umask 077 -> 0700
        open(os.path.join(d, "k"), "w").write("x\n")  # 0600
        self.assertEqual(self.mode("package.accept_keywords"), 0o700)
        r = self.cli("--json")
        self.assertEqual(r.returncode, 1, r.stderr)
        res = json.loads(r.stdout)
        got = sorted(os.path.relpath(f["path"], "/etc/portage") for f in res["findings"])
        self.assertEqual(got, ["package.accept_keywords", "package.use"])  # the 0700 directories are the blockers
        r = self.cli("--fix", "--json")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue(json.loads(r.stdout)["ok"])
        self.assertEqual(self.mode("package.use/horcrux-gnome"), 0o644)
        self.assertEqual(self.mode("package.accept_keywords"), 0o755)
        self.assertEqual(self.mode("package.accept_keywords/k"), 0o644)
        self.assertEqual(self.check()[0], [])

    def test_the_tool_itself_runs_under_umask_077(self):
        mk(os.path.join(self.cfg, "a"), 0o600)
        r = self.cli("--fix", "--record", "--quiet", umask=0o077)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        st = os.path.join(self.tmp, "state", "portage-config.json")
        self.assertEqual(stat.S_IMODE(os.stat(st).st_mode), 0o644)  # the status must not inherit the umask
        self.assertEqual(stat.S_IMODE(os.stat(os.path.dirname(st)).st_mode), 0o755)
        self.assertEqual(self.mode("a"), 0o644)

    def test_fix_only_adds_bits_never_removes_or_widens_beyond_need(self):
        self.set_group(MYGID)
        mk(os.path.join(self.cfg, "g"), 0o600)  # group is portage: only the group read bit is needed
        mk(os.path.join(self.cfg, "x"), 0o700)  # executable stays executable
        os.chmod(os.path.join(self.cfg, "x"), 0o700)
        self.check()
        r = self.cli("--fix")
        self.assertIn(r.returncode, (0, 1))
        self.assertEqual(self.mode("g"), 0o640)
        self.assertEqual(self.mode("x"), 0o740)
        mk(os.path.join(self.cfg, "fine"), 0o664)
        r = self.cli("--fix")
        self.assertEqual(self.mode("fine"), 0o664)

    def test_fix_keeps_owner_and_group(self):
        p = mk(os.path.join(self.cfg, "a"), 0o600)
        before = os.lstat(p)
        self.cli("--fix")
        after = os.lstat(p)
        self.assertEqual((before.st_uid, before.st_gid), (after.st_uid, after.st_gid))

    def test_fix_refuses_files_not_owned_by_root(self):
        p = mk(os.path.join(self.cfg, "a"), 0o600)
        r = self.cli("--fix", env_extra={"HX_DOCTOR_OWNER_UID": str(os.getuid() + 1)})
        self.assertEqual(r.returncode, 1)
        self.assertEqual(self.mode("a"), 0o600)

    def test_report_without_fix_changes_nothing(self):
        mk(os.path.join(self.cfg, "a"), 0o600)
        mk(os.path.join(self.cfg, "d"), 0o700, d=True)
        r = self.cli()
        self.assertEqual(r.returncode, 1)
        self.assertIn("horcrux-doctor portage-config --fix", r.stdout)
        self.assertEqual((self.mode("a"), self.mode("d")), (0o600, 0o700))

    def test_fix_needs_root_without_the_test_switch(self):
        if os.geteuid() == 0:
            self.skipTest("root")
        r = subprocess.run([sys.executable, DOCTOR, "--root", self.root, "--fix"], capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)

    def test_nested_blockers_converge(self):
        mk(os.path.join(self.cfg, "a", "b"), 0o700, d=True)
        mk(os.path.join(self.cfg, "a", "b", "c"), 0o600)
        os.chmod(os.path.join(self.cfg, "a"), 0o700)
        self.cli("--fix")
        self.assertEqual((self.mode("a"), self.mode("a/b"), self.mode("a/b/c")), (0o755, 0o755, 0o644))


class Symlinks(Base):
    def test_link_to_readable_target_is_fine(self):
        mk(os.path.join(self.cfg, "real"), 0o644)
        os.symlink("real", os.path.join(self.cfg, "link"))
        self.assertEqual(self.check()[0], [])

    def test_link_target_outside_config_is_reported_not_fixed(self):
        outside = mk(os.path.join(self.root, "srv", "x", "conf"), 0o600)
        os.symlink("/srv/x/conf", os.path.join(self.cfg, "link"))
        f, _ = self.check()
        self.assertEqual(len(f), 1)
        self.assertFalse(f[0]["fixable"])
        self.assertIn("link", f[0]["path"])
        self.cli("--fix")
        self.assertEqual(stat.S_IMODE(os.stat(outside).st_mode), 0o600)  # never chmod through a symlink

    def test_link_to_dir_in_unsearchable_parent(self):
        mk(os.path.join(self.root, "home", "u"), 0o700, d=True)
        mk(os.path.join(self.root, "home", "u", "conf"), 0o644)
        os.symlink("/home/u/conf", os.path.join(self.cfg, "make.conf"))
        f, _ = self.check()
        self.assertEqual(len(f), 1)
        self.assertEqual(f[0]["target"], "/home/u")
        self.assertFalse(f[0]["fixable"])

    def test_link_chain_and_relative_dotdot(self):
        mk(os.path.join(self.cfg, "profile", "base"), 0o600)
        os.symlink("profile/base", os.path.join(self.cfg, "make.profile-link"))
        os.makedirs(os.path.join(self.cfg, "x"))
        os.symlink("../make.profile-link", os.path.join(self.cfg, "x", "y"))
        f, _ = self.check()
        self.assertEqual(sorted({x["target"] for x in f}), ["/etc/portage/profile/base"])

    def test_dangling_and_loop(self):
        os.symlink("nowhere", os.path.join(self.cfg, "dangling"))
        os.symlink("loop2", os.path.join(self.cfg, "loop1"))
        os.symlink("loop1", os.path.join(self.cfg, "loop2"))
        f, _ = self.check()
        kinds = sorted(x["problem"] for x in f)
        self.assertIn("dangling-symlink", kinds)
        self.assertIn("symlink-loop", kinds)
        self.assertTrue(all(not x["fixable"] for x in f))

    def test_directory_symlink_is_not_walked_twice_or_escaped(self):
        mk(os.path.join(self.root, "elsewhere"), 0o700, d=True)
        mk(os.path.join(self.root, "elsewhere", "f"), 0o600)
        os.symlink("/elsewhere", os.path.join(self.cfg, "dirlink"))
        f, _ = self.check()
        self.assertEqual(len(f), 1)  # the link's own target directory is what is judged, not a tree walk beyond the config dir
        self.assertFalse(f[0]["fixable"])

    def test_symlink_mode_is_irrelevant(self):
        mk(os.path.join(self.cfg, "real"), 0o644)
        os.symlink("real", os.path.join(self.cfg, "link"))
        try:
            getattr(os, "lchmod")(os.path.join(self.cfg, "link"), 0o000)
        except (AttributeError, NotImplementedError, OSError):
            pass
        self.assertEqual(self.check()[0], [])


class Secrets(Base):
    def setUp(self):
        super().setUp()
        self.s = [
            mk(os.path.join(self.cfg, "gnupg"), 0o700, d=True),
            mk(os.path.join(self.cfg, "gnupg", "private-keys-v1.d", "k.key"), 0o600, "TOPSECRET\n"),
            mk(os.path.join(self.cfg, "gnupg", "pubring.kbx"), 0o600),
            mk(os.path.join(self.cfg, "auth.conf"), 0o600, "pass=TOPSECRET\n"),
            mk(os.path.join(self.cfg, "repos.conf", "deploy-token"), 0o600, "TOPSECRET\n"),
        ]
        os.chmod(os.path.join(self.cfg, "gnupg", "private-keys-v1.d"), 0o700)

    def test_secrets_neither_reported_nor_fixed(self):
        f, skipped = self.check()
        self.assertEqual(f, [])
        joined = ["/".join(s) for s in skipped]
        self.assertIn("gnupg", joined)
        self.assertIn("auth.conf", joined)
        self.assertIn("repos.conf/deploy-token", joined)
        before = {p: stat.S_IMODE(os.lstat(p).st_mode) for p in self.s}
        r = self.cli("--fix")
        self.assertEqual(r.returncode, 0, r.stdout)
        for p, m in before.items():
            self.assertEqual(stat.S_IMODE(os.lstat(p).st_mode), m, p)
        self.assertNotIn("TOPSECRET", r.stdout + r.stderr)

    def test_secret_next_to_a_real_problem(self):
        mk(os.path.join(self.cfg, "package.use", "a"), 0o600)
        r = self.cli("--fix")
        self.assertEqual(r.returncode, 0)
        self.assertEqual(self.mode("package.use/a"), 0o644)
        self.assertEqual(self.mode("auth.conf"), 0o600)
        self.assertEqual(self.mode("gnupg"), 0o700)

    def test_symlink_to_secret_is_skipped_and_never_chmodded(self):
        os.symlink("auth.conf", os.path.join(self.cfg, "alias"))
        os.symlink("gnupg/pubring.kbx", os.path.join(self.cfg, "alias2"))
        f, _ = self.check()
        self.assertEqual(f, [])
        self.cli("--fix")
        self.assertEqual(self.mode("auth.conf"), 0o600)

    def test_a_secret_never_becomes_part_of_json_output(self):
        r = self.cli("--json")
        self.assertNotIn("TOPSECRET", r.stdout)


@unittest.skipUnless(os.path.isfile(DOCTOR) and os.geteuid() == 0, "real-credential probe needs root")
class Probe(Base):
    def test_probe_agrees_with_model(self):
        # a real portage group member opens the file: 0604 root:portage must fail, 0640 root:portage succeed
        gid = 60000
        self.set_group(gid)
        os.chmod(self.tmp, 0o755)  # mkdtemp is 0700: the unprivileged probe user must be able to reach the tree
        a = mk(os.path.join(self.cfg, "bad"), 0o604)
        b = mk(os.path.join(self.cfg, "good"), 0o640)
        os.chown(a, 0, gid)
        os.chown(b, 0, gid)
        os.chmod(self.root, 0o755)
        if hd.probe_open(b, gid, 64000, False) is not True:
            self.skipTest("the probe user cannot reach the temp dir here (parent directory modes)")
        self.assertFalse(hd.probe_open(a, gid, 64000, False))
        self.assertTrue(hd.probe_open(b, gid, 64000, False))
        f, _ = self.check(probe=True)
        self.assertEqual(self.paths(f), ["bad"])


@unittest.skipUnless(os.path.isdir(CORE), "Horcrux repo tree only")
class Packaging(unittest.TestCase):
    def ebuild(self):
        import glob
        import re
        eb = sorted(glob.glob(os.path.join(CORE, "horcrux-core-1-r*.ebuild")), key=lambda p: int(re.search(r"-r(\d+)\.ebuild$", p).group(1)))[-1]
        return open(eb).read()

    def test_doctor_is_installed_by_core(self):
        e = self.ebuild()
        self.assertIn('dosbin "${FILESDIR}"/horcrux-doctor', e)
        self.assertIn("60-horcrux-doctor", e)
        self.assertTrue(os.access(DOCTOR, os.X_OK))

    def test_runs_after_every_tree_sync_and_never_fails_it(self):
        hook = open(os.path.join(CORE, "files", "postsync-doctor")).read()
        self.assertIn("horcrux-doctor portage-config --record --quiet", hook)
        self.assertTrue(hook.rstrip().endswith("exit 0"))

    def test_init_neutral(self):
        s = open(DOCTOR).read()
        for word in ("rc-service", "rc-update", "systemctl", "/run/systemd"):
            self.assertNotIn(word, s)

    def test_no_hard_coded_mode_check_shortcuts(self):
        s = open(DOCTOR).read()
        for bad in ("& 0o044", "& 0o004", "& 0o040", "S_IROTH", "S_IRGRP"):
            self.assertNotIn(bad, s)


if __name__ == "__main__":
    unittest.main()
