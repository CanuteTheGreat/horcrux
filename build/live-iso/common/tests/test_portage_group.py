"""Every administrator account of a Gentoo-based install is in the portage group, so equery and eix work for it without sudo
(owner 2026-11-17: "make sure my user is always in the portage group on gentoo-based installs so that commands like equery and eix work").

Covered here: the installer's user creation (server PAM admin, desktop user, hardware-profile desktop user; Horcrux and Patronus share it),
the eix cache the installer builds, the legacy/server installer scripts, and the package-managed safety net for existing installs
(sys-apps/horcrux-core: horcrux-admin-groups from pkg_postinst and after every tree sync). The installed system is checked by the QEMU
gate headless_gates.portage_group_gate (qemu-test.py ssh_checks, zfs-enc-test.py server/desktop checks).
Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import glob
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.join(HERE, "..")
sys.path.insert(0, os.path.join(COMMON, "overlay", "usr", "lib", "live-installer"))
from liveinst import desktop, hwprofile  # noqa: E402

import importlib.util  # noqa: E402
_spec = importlib.util.spec_from_file_location("headless_gates", os.path.join(COMMON, "headless_gates.py"))
HG = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(HG)

REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
_HORCRUX_TREE = os.path.isdir(os.path.join(REPO, "build", "live-iso")) and os.path.isdir(os.path.join(REPO, "gentoo", "sys-apps", "horcrux-core"))
CORE = os.path.join(REPO, "gentoo", "sys-apps", "horcrux-core")


class FakeInst:
    def __init__(self, root, c=None, p=None):
        self.target, self.c, self.p, self.runs, self.logs = root, c or {}, p or {}, [], []

    def run(self, cmd, **kw):
        self.runs.append(cmd)

    def write(self, rel, text, append=False, *a):
        path = self.target + rel
        os.makedirs(os.path.dirname(path), exist_ok=True)
        open(path, "a" if append else "w").write(text)

    def log(self, m):
        self.logs.append(m)


def target(group_lines):
    r = tempfile.mkdtemp()
    os.makedirs(r + "/etc")
    open(r + "/etc/group", "w").write("".join(l + "\n" for l in group_lines))
    return r


def useradd_groups(runs):
    ua = [c for c in runs if c and c[0] == "useradd"]
    assert len(ua) == 1, runs
    return ua[0][ua[0].index("-G") + 1].split(",")


class InstallerUserCreation(unittest.TestCase):
    def test_portage_is_an_admin_group(self):
        self.assertIn("portage", desktop.GROUPS)
        self.assertEqual(desktop.ADMIN_GROUPS, ("wheel", "portage"))

    def check_create_user(self, prof, admin_group):
        # an image whose /etc/group has no portage line: the installer creates it (-r) and still puts the user in it
        r = target(["root:x:0:", "wheel:x:10:", "audio:x:18:", admin_group + ":x:990:"])
        try:
            inst = FakeInst(r, {"user_name": "ron", "user_hash": "$6$x$y"}, prof)
            desktop.create_user(inst)
            self.assertIn(["groupadd", "-f", "-r", "portage"], inst.runs)
            ga = inst.runs.index(["groupadd", "-f", "-r", "portage"])
            ua = next(i for i, c in enumerate(inst.runs) if c[0] == "useradd")
            self.assertLess(ga, ua)                                       # group first, then the user
            g = useradd_groups(inst.runs)
            self.assertIn("portage", g)
            self.assertIn("wheel", g)
            return g
        finally:
            shutil.rmtree(r)

    def test_horcrux_pam_admin_gets_portage(self):
        g = self.check_create_user({"web_auth": "pam"}, "horcrux-admin")
        self.assertIn("horcrux-admin", g)

    def test_patronus_pam_admin_gets_portage(self):
        g = self.check_create_user({"web_auth": "pam", "web_admin_group": "patronus-admin"}, "patronus-admin")
        self.assertIn("patronus-admin", g)

    def test_desktop_user_without_pam_profile_gets_portage(self):
        self.check_create_user({}, "horcrux-admin")

    def test_hardware_profile_desktop_user_gets_portage(self):
        r = target(["root:x:0:", "wheel:x:10:", "audio:x:18:"])
        try:
            inst = FakeInst(r, {"desktop": {"enable": True, "user": "owner", "password_hash": "x"}})
            hwprofile.add_desktop_user(inst, inst.c["desktop"])
            self.assertIn(["groupadd", "-f", "-r", "portage"], inst.runs)
            self.assertIn("portage", useradd_groups(inst.runs))
        finally:
            shutil.rmtree(r)

    def test_installer_builds_the_eix_cache(self):
        s = open(os.path.join(COMMON, "overlay", "usr", "lib", "live-installer", "liveinst", "system.py")).read()
        self.assertRegex(s, r'os\.path\.exists\(f"\{T\}/usr/bin/eix-update"\):\s*\n\s*self\.run\(\["eix-update", "-q"\], chroot=True, check=False\)')

    def test_eix_is_in_every_image(self):
        atoms = [l.split("#")[0].strip() for l in open(os.path.join(COMMON, "packages.txt"))]
        self.assertIn("app-portage/eix", atoms)
        self.assertIn("app-portage/gentoolkit", atoms)


class Gate(unittest.TestCase):
    GOOD = ("GROUPS hxadmin wheel portage horcrux-admin\nEQUERY-RC=0\nEQ sys-apps/portage-3.0.83-r1\nEIX-RC=0\n"
            "EIX [I] sys-apps/portage (3.0.83-r1@11/17/2026): The package management system\nCFG-BAD 0\nTREE-SYNCED\nEIX-CACHE -rw-rw-r-- 1 portage portage 1 x\nPG-DONE\n")

    def test_good(self):
        self.assertTrue(HG.portage_group_verdict(self.GOOD)[0])

    def test_unreadable_portage_config_fails(self):
        # an administrator that cannot read a file under /etc/portage (0600 root, 0700 directory) is the failure this catches
        bad = self.GOOD.replace("CFG-BAD 0", "CFG-BAD 1\nCFG-UNREADABLE /etc/portage/package.use/horcrux-gnome")
        self.assertFalse(HG.portage_group_verdict(bad)[0])
        self.assertIn("etc-portage-readable=FAIL", HG.portage_group_verdict(bad)[1])

    def test_dangling_symlink_before_the_first_sync_is_not_an_access_failure(self):
        # /etc/portage/make.profile points into the tree that first boot has not synced yet: find -readable follows links, so it was reported unreadable
        o = self.GOOD.replace("TREE-SYNCED", "TREE-EMPTY").replace("EIX-RC=0", "EIX-RC=1").replace(
            "EIX [I] sys-apps/portage (3.0.83-r1@11/17/2026): The package management system", "EIX No matches found").replace(
            "CFG-BAD 0", "CFG-DANGLING 1\nCFG-DANGLING-LINK /etc/portage/make.profile\nCFG-BAD 0")
        self.assertTrue(HG.portage_group_verdict(o)[0])

    def test_dangling_symlink_after_the_sync_fails(self):
        o = self.GOOD.replace("CFG-BAD 0", "CFG-DANGLING 1\nCFG-DANGLING-LINK /etc/portage/make.profile\nCFG-BAD 0")
        self.assertFalse(HG.portage_group_verdict(o)[0])

    def test_real_unreadable_file_still_fails_while_the_tree_is_empty(self):
        o = self.GOOD.replace("TREE-SYNCED", "TREE-EMPTY").replace("CFG-BAD 0", "CFG-DANGLING 1\nCFG-BAD 1\nCFG-UNREADABLE /etc/portage/package.use/x")
        self.assertFalse(HG.portage_group_verdict(o)[0])

    def test_script_skips_dangling_links_in_the_access_probe(self):
        s = HG.portage_group_script("hxadmin")
        self.assertIn("-prune -o -xtype l -prune -o", s)
        self.assertIn("CFG-DANGLING", s)

    def test_run_without_the_config_probe_proves_nothing(self):
        self.assertFalse(HG.portage_group_verdict(self.GOOD.replace("CFG-BAD 0\n", ""))[0])

    def test_script_probes_effective_access_as_the_user(self):
        s = HG.portage_group_script("hxadmin")
        self.assertIn("-type d ! -executable -o ! -readable", s)        # access(2) as that user, not mode bits
        self.assertIn("/etc/portage/gnupg -prune", s)                   # keyring is secret by design

    def test_not_in_portage_fails(self):
        self.assertFalse(HG.portage_group_verdict(self.GOOD.replace(" portage horcrux-admin", " horcrux-admin"))[0])

    def test_missing_eix_cache_fails(self):
        bad = self.GOOD.replace("EIX [I] sys-apps/portage (3.0.83-r1@11/17/2026): The package management system",
                                "EIX cannot open database file /var/cache/eix/portage.eix for reading").replace("EIX-RC=0", "EIX-RC=1")
        self.assertFalse(HG.portage_group_verdict(bad)[0])

    def test_permission_denied_fails(self):
        self.assertFalse(HG.portage_group_verdict(self.GOOD.replace("EQ sys-apps/portage-3.0.83-r1", "EQ sys-apps/portage-3.0.83-r1\nEQ Permission denied: '/var/cache/edb/x'"))[0])

    def test_synced_tree_must_find_portage(self):
        self.assertFalse(HG.portage_group_verdict(self.GOOD.replace("EIX [I] sys-apps/portage (3.0.83-r1@11/17/2026): The package management system",
                                                                     "EIX No matches found"))[0])

    def test_empty_tree_needs_only_a_readable_cache(self):
        o = self.GOOD.replace("EIX [I] sys-apps/portage (3.0.83-r1@11/17/2026): The package management system", "EIX No matches found") \
                     .replace("EIX-RC=0", "EIX-RC=1").replace("TREE-SYNCED", "TREE-EMPTY")
        self.assertTrue(HG.portage_group_verdict(o)[0])

    def test_equery_failure_fails(self):
        self.assertFalse(HG.portage_group_verdict(self.GOOD.replace("EQUERY-RC=0", "EQUERY-RC=127"))[0])

    def test_gate_waits_only_while_firstboot_runs(self):
        seen = []
        outs = [self.GOOD.replace("PG-DONE", "FIRSTBOOT-RUNNING\nPG-DONE"), self.GOOD]
        HG.time.sleep, real = (lambda s: None), HG.time.sleep
        try:
            HG.portage_group_gate(lambda cmd: (0, outs.pop(0)), lambda n, ok, info: seen.append((n, ok)), "hxadmin", wait=600)
        finally:
            HG.time.sleep = real
        self.assertEqual(seen, [("admin-user-in-portage-group-equery-eix-work", True)])
        self.assertEqual(outs, [])

    def test_script_runs_the_tools_as_the_user(self):
        s = HG.portage_group_script("hxadmin")
        self.assertIn("U=hxadmin", s)
        self.assertIn("setpriv --reuid=$U", s)
        self.assertIn("equery", s)
        self.assertIn("eix -e", s)

    def test_wired_into_the_qemu_gates(self):
        q = open(os.path.join(COMMON, "qemu-test.py")).read()
        m = re.search(r"def ssh_checks\(self\):.*?(?=\n    def |\nclass |\ndef |\Z)", q, re.S)
        self.assertIn("HG.portage_group_gate", m.group(0))
        z = open(os.path.join(COMMON, "zfs-enc-test.py")).read()
        for fn in ("server_checks", "desktop_checks"):
            m = re.search(r"def " + fn + r"\(self.*?(?=\n    def |\Z)", z, re.S)
            self.assertIn("qt.HG.portage_group_gate", m.group(0), fn)


@unittest.skipUnless(_HORCRUX_TREE, "Horcrux repo tree only")
class HorcruxScripts(unittest.TestCase):
    def test_server_installer_script(self):
        s = open(os.path.join(REPO, "build", "installer", "horcrux-installer.sh")).read()
        self.assertIn('groupadd -f -r portage', s)
        self.assertIn('local groups="wheel,portage"', s)
        self.assertNotIn("useradd -m -G wheel,kvm,libvirt,docker", s)         # a missing docker group made useradd create no user at all

    def test_iso_build_scripts(self):
        self.assertIn('local user_groups="wheel,portage"', open(os.path.join(REPO, "build", "scripts", "build-gentoo-iso.sh")).read())
        for f in ("build/catalyst/files/fsscript-livecd.sh", "build/catalyst/files/livecd-stage1.sh", "build/catalyst/manual-build.sh"):
            s = open(os.path.join(REPO, f)).read()
            for line in [l for l in s.splitlines() if "useradd" in l and not l.lstrip().startswith("#")]:
                self.assertRegex(line, r"-G \S*portage", f)


@unittest.skipUnless(_HORCRUX_TREE, "Horcrux repo tree only")
class SafetyNetPackage(unittest.TestCase):
    def ebuild(self):
        eb = sorted(glob.glob(os.path.join(CORE, "horcrux-core-1-r*.ebuild")), key=lambda p: int(re.search(r"-r(\d+)\.ebuild$", p).group(1)))
        self.assertGreaterEqual(int(re.search(r"-r(\d+)\.ebuild$", eb[-1]).group(1)), 6)      # revision bump: existing installs pick it up
        return open(eb[-1]).read()

    def test_core_ships_and_runs_it(self):
        e = self.ebuild()
        for atom in ("app-portage/eix", "app-portage/gentoolkit", "acct-group/portage"):
            self.assertIn(atom, e)
        self.assertIn('dosbin "${FILESDIR}"/horcrux-admin-groups', e)
        self.assertIn("newexe \"${FILESDIR}\"/postsync-admin-groups 60-horcrux-admin-groups", e)
        post = e[e.index("pkg_postinst()"):]
        self.assertIn("/usr/sbin/horcrux-admin-groups", post)
        self.assertIn("elog", post)
        self.assertIn("eix-update", post)

    def test_firstboot_indexes_eix_before_the_done_marker(self):
        f = open(os.path.join(REPO, "gentoo", "sys-apps", "horcrux-firstboot", "files", "horcrux-firstboot")).read()
        self.assertLess(f.index("eix-update -q"), f.rindex("> \"$DONE_MARKER\"\n"))
        self.assertTrue(glob.glob(os.path.join(REPO, "gentoo", "sys-apps", "horcrux-firstboot", "horcrux-firstboot-1-r*.ebuild"))
                        or any(int(re.search(r"-r(\d+)\.ebuild$", p).group(1)) > 4
                               for p in glob.glob(os.path.join(REPO, "gentoo", "sys-apps", "horcrux-firstboot", "horcrux-firstboot-1-r*.ebuild"))))

    def fake_root(self, group, passwd):
        r = tempfile.mkdtemp()
        os.makedirs(r + "/etc")
        open(r + "/etc/group", "w").write(group)
        open(r + "/etc/passwd", "w").write(passwd)
        return r

    GROUP = ("root:x:0:root\nwheel:x:10:root,ron,svc\nportage:x:250:portage,amy\nhorcrux-admin:x:990:amy,bob\npatronus-admin:x:991:\n"
             "users:x:100:\ncarl:x:1003:\n")
    PASSWD = ("root:x:0:0::/root:/bin/bash\nportage:x:250:250::/var/lib/portage/home:/sbin/nologin\nsvc:x:500:500::/:/sbin/nologin\n"
              "ron:x:1000:1000::/home/ron:/bin/bash\namy:x:1001:1001::/home/amy:/bin/bash\nbob:x:1002:100::/home/bob:/bin/bash\n"
              "carl:x:1003:990::/home/carl:/bin/bash\nnobody:x:65534:65534::/:/sbin/nologin\ndan:x:1004:1004::/home/dan:/bin/bash\n")

    def run_script(self, root, dry=True):
        env = dict(os.environ, ROOT=root)
        if dry:
            env["DRY_RUN"] = "1"
        return subprocess.run(["sh", os.path.join(CORE, "files", "horcrux-admin-groups")], env=env, capture_output=True, text=True)

    def test_dry_run_picks_human_admins_only(self):
        r = self.fake_root(self.GROUP, self.PASSWD)
        try:
            p = self.run_script(r)
            self.assertEqual(p.returncode, 0, p.stderr)
            # ron (wheel), bob (horcrux-admin), carl (primary group horcrux-admin); amy already in portage; root/svc/nobody/dan never
            self.assertEqual(sorted(p.stdout.splitlines()), ["would add bob to portage", "would add carl to portage", "would add ron to portage"])
        finally:
            shutil.rmtree(r)

    def test_missing_portage_group_is_created_first(self):
        r = self.fake_root(self.GROUP.replace("portage:x:250:portage,amy\n", ""), self.PASSWD)
        try:
            out = self.run_script(r).stdout.splitlines()
            self.assertEqual(out[0], "would create group portage")
            self.assertIn("would add amy to portage", out)
        finally:
            shutil.rmtree(r)

    def test_nothing_to_do_prints_nothing(self):
        r = self.fake_root("wheel:x:10:ron\nportage:x:250:ron\n", "ron:x:1000:1000::/home/ron:/bin/bash\n")
        try:
            p = self.run_script(r)
            self.assertEqual((p.returncode, p.stdout), (0, ""))
        finally:
            shutil.rmtree(r)

    @unittest.skipUnless(os.geteuid() == 0 and shutil.which("gpasswd"), "needs root and shadow's gpasswd")
    def test_real_run_is_idempotent(self):
        r = self.fake_root(self.GROUP, self.PASSWD)
        try:
            p = self.run_script(r, dry=False)
            self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
            self.assertEqual(sorted(p.stdout.splitlines()), ["added bob to portage", "added carl to portage", "added ron to portage"])
            line = [l for l in open(r + "/etc/group") if l.startswith("portage:")][0].strip()
            self.assertEqual(sorted(line.split(":")[3].split(",")), ["amy", "bob", "carl", "portage", "ron"])
            p = self.run_script(r, dry=False)
            self.assertEqual((p.returncode, p.stdout), (0, ""))
        finally:
            shutil.rmtree(r)


if __name__ == "__main__":
    unittest.main()
