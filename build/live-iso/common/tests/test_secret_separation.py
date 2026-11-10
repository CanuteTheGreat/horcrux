"""The disk passphrase is ONLY for the disk (owner rule, 2026-11-03: the Surface image used it as the desktop password).

Regression tests for every front end: the shared installer core (answer file + guided stages + the go() guard), the per-machine
auto installers (Surface bash, gringotts python), the legacy shell installer and the QEMU gate that checks /etc/shadow.
All passwords here are throw-away test values; nothing is read from or written to a real system.
Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import importlib.machinery
import importlib.util
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.join(HERE, "..")
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
sys.path.insert(0, os.path.join(COMMON, "overlay", "usr", "lib", "live-installer"))
from liveinst import secretcheck as SC  # noqa: E402
from liveinst import stages as ST  # noqa: E402
from liveinst import system as S  # noqa: E402

DISK_PW = "Wq4#vN8-correct-horse-Zx2"       # test-only disk passphrase
OTHER_PW = "Bd6!hP3-tablet-user-Rk9"        # test-only account password
PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs", "xfs-luks"], "swap_mb": 0, "network_mode": "management",
           "services": [], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}


def sha512(pw, salt="unittest1"):
    return subprocess.check_output(["openssl", "passwd", "-6", "-salt", salt, pw], text=True).strip()


def cfg(**kw):
    c = S.default_config(PROFILE)
    c.update(hostname="a.example.com", disks=["/dev/vda"], fs="xfs-luks", luks_passphrase=DISK_PW)
    c.update(kw)
    return c


class HashCheck(unittest.TestCase):
    def test_verifies_only_the_right_secret(self):
        h = sha512(DISK_PW)
        self.assertIs(SC.hash_verifies(DISK_PW, h), True)
        self.assertIs(SC.hash_verifies(DISK_PW + "x", h), False)

    def test_locked_or_empty_is_not_a_match(self):
        for h in ("", "!", "*", "!$6$x$y"):
            self.assertIsNone(SC.hash_verifies(DISK_PW, h))

    def test_other_salts_and_schemes(self):
        for alg in ("-5", "-1"):
            h = subprocess.check_output(["openssl", "passwd", alg, "-salt", "abcdefgh", DISK_PW], text=True).strip()
            self.assertIs(SC.hash_verifies(DISK_PW, h), True, alg)


class ReuseRules(unittest.TestCase):
    def test_user_plaintext_equals_passphrase(self):
        c = cfg(install_type="desktop", user_name="owner", user_password=DISK_PW)
        self.assertTrue(any("disk passphrase" in e for e in SC.reuse_problems(c)))

    def test_user_hash_of_passphrase(self):          # exactly what horcrux-surface-auto did (openssl passwd -6 of the passphrase)
        c = cfg(install_type="desktop", user_name="owner", user_hash=sha512(DISK_PW, "qemuuser1"))
        self.assertTrue(any("disk passphrase" in e for e in SC.reuse_problems(c)))

    def test_desktop_section_hash_and_plain(self):
        for d in ({"enable": True, "user": "u", "password_hash": sha512(DISK_PW)}, {"enable": True, "user": "u", "password": DISK_PW}):
            self.assertTrue(SC.reuse_problems(cfg(desktop=d)), d)

    def test_root_must_not_be_the_passphrase(self):
        self.assertTrue(SC.reuse_problems(cfg(root_password=DISK_PW)))
        self.assertTrue(SC.reuse_problems(cfg(root_hash=sha512(DISK_PW))))
        self.assertTrue(SC.reuse_problems(cfg(web_password=DISK_PW)))

    def test_zfs_passphrase_counts_too(self):
        c = cfg(fs="zfs", zfs_encrypt=True, zfs_passphrase=DISK_PW, luks_passphrase="", user_name="o", user_password=DISK_PW, install_type="desktop")
        self.assertTrue(SC.reuse_problems(c))

    def test_distinct_secrets_pass(self):
        c = cfg(install_type="desktop", user_name="owner", user_hash=sha512(OTHER_PW), root_hash="", root_password="")
        self.assertEqual(SC.reuse_problems(c), [])

    def test_root_and_user_must_differ(self):
        c = cfg(install_type="desktop", user_name="owner", user_password=OTHER_PW, root_password=OTHER_PW)
        self.assertTrue(any("root and" in e for e in SC.reuse_problems(c)))
        c = cfg(install_type="desktop", user_name="owner", user_password=OTHER_PW, root_hash=sha512(OTHER_PW))
        self.assertTrue(any("root and" in e for e in SC.reuse_problems(c)))

    def test_no_encryption_no_passphrase_rule(self):
        c = cfg(fs="xfs", luks_passphrase="", user_name="o", user_password=DISK_PW, install_type="desktop")
        self.assertEqual(SC.reuse_problems(c), [])

    def test_messages_never_contain_the_secrets(self):
        c = cfg(install_type="desktop", user_name="owner", user_password=DISK_PW, root_password=DISK_PW)
        self.assertNotIn(DISK_PW, " ".join(SC.reuse_problems(c)))


class InstallerRefuses(unittest.TestCase):
    def test_validate_config_refuses_identical(self):
        c = cfg(install_type="desktop", user_name="owner", user_hash=sha512(DISK_PW), root_hash="")
        errs = S.validate_config(PROFILE, c, unattended=True)
        self.assertTrue(any("disk passphrase" in e for e in errs), errs)

    def test_validate_config_accepts_separate_user_with_locked_root(self):
        """desktop user + sudo, no root password, no ssh key: valid (root is locked by default), not 'unreachable'"""
        c = cfg(install_type="desktop", user_name="owner", user_hash=sha512(OTHER_PW))
        errs = S.validate_config(PROFILE, c, unattended=True)
        self.assertFalse(any("disk passphrase" in e or "unreachable" in e for e in errs), errs)

    def test_go_refuses_before_touching_the_disk(self):
        c = cfg(install_type="desktop", user_name="owner", user_password=DISK_PW)
        inst = S.Installer(PROFILE, c, lambda line: None, lambda p, n: None)
        calls = []
        inst.run = lambda *a, **k: calls.append(a)     # nothing may run: no partitioning, no chpasswd
        with self.assertRaises(S.InstallError) as cm:
            inst.go()
        self.assertIn("refusing", str(cm.exception))
        self.assertEqual(calls, [])
        self.assertNotIn(DISK_PW, str(cm.exception))

    def test_root_locked_when_no_password_given(self):
        calls = []
        c = cfg(install_type="desktop", user_name="owner", user_hash=sha512(OTHER_PW))
        inst = S.Installer(PROFILE, c, lambda line: None, lambda p, n: None)
        inst.run = lambda cmd, **k: calls.append(cmd)
        inst.target = "/nonexistent-target"
        try:
            inst.cfg_accounts()
        except Exception:     # noqa: the ssh-key part touches the filesystem; only the first commands matter
            pass
        self.assertIn(["passwd", "-l", "root"], calls)


class GuidedStages(unittest.TestCase):
    def ctx(self, **kw):
        return ST.Ctx(PROFILE, cfg(**kw))

    def test_user_stage_refuses_the_passphrase(self):
        msg = ST.GnomeStage().validate(self.ctx(install_type="desktop"), {"pw": DISK_PW, "pw2": DISK_PW})
        self.assertIn("disk passphrase", msg)

    def test_user_stage_accepts_a_different_password(self):
        self.assertIsNone(ST.GnomeStage().validate(self.ctx(install_type="desktop"), {"pw": OTHER_PW, "pw2": OTHER_PW}))

    def test_user_stage_still_needs_two_matching_entries(self):
        self.assertIn("do not match", ST.GnomeStage().validate(self.ctx(), {"pw": OTHER_PW, "pw2": OTHER_PW + "x"}))

    def test_surface_desktop_stage_refuses_the_passphrase(self):
        self.assertIn("disk passphrase", ST.DesktopStage().validate(self.ctx(), {"pw": DISK_PW, "pw2": DISK_PW}))

    def test_root_stage_refuses_the_passphrase(self):
        self.assertIn("disk passphrase", ST.BY_ID["rootpass"].validate(self.ctx(), {"pw": DISK_PW, "pw2": DISK_PW}))

    def test_root_stage_optional_and_locked_on_a_user_machine(self):
        st = ST.BY_ID["rootpass"]
        self.assertIsNotNone(st.validate(self.ctx(), {"pw": "", "pw2": ""}), "a server still needs a root password")
        st._user_machine = lambda ctx: True
        try:
            self.assertIsNone(st.validate(self.ctx(), {"pw": "", "pw2": ""}))
            f = {x.key: x for x in st.fields(self.ctx())}
            self.assertTrue(f["pw"].optional)
        finally:
            del st._user_machine

    def test_user_stage_refuses_the_root_password(self):
        ctx = self.ctx(install_type="desktop")
        ctx.c["root_password"] = OTHER_PW
        self.assertIn("share a password", ST.GnomeStage().validate(ctx, {"pw": OTHER_PW, "pw2": OTHER_PW}))

    def test_disk_stage_refuses_a_passphrase_an_account_already_uses(self):
        ctx = self.ctx(install_type="desktop", user_name="owner", user_password=OTHER_PW)
        self.assertIn("disk passphrase", ST.BY_ID["disk"].validate(ctx, {"fs": "xfs-luks", "lpw": OTHER_PW, "lpw2": OTHER_PW}))


@unittest.skipUnless(os.path.exists(os.path.join(ROOT, "build", "installer", "horcrux-installer.sh")), "reads Horcrux-only files (absent in a vendored copy, e.g. Patronus)")
class SurfaceAuto(unittest.TestCase):
    """the real prompt loop of horcrux-surface-auto, run in bash with scripted stdin"""
    SCRIPT = os.path.join(ROOT, "gentoo", "app-misc", "horcrux-surface-desktop", "files", "root", "usr", "bin", "horcrux-surface-auto")

    def text(self):
        return open(self.SCRIPT).read()

    def run_loop(self, stdin):
        t = self.text()
        a, b = t.index("mark WAITING-FOR-PASSPHRASE"), t.index("# answer file in tmpfs")
        pre = "B=; G=; R=; Y=; N=; USER_NAME=canutethegreat\nmark() { echo \"MARK $*\"; }\n"
        post = '\n[ "$u1" = "$p1" ] && echo ACCOUNT-EQUALS-DISK; echo "UHASH=$(printf %s "$u1" | openssl passwd -6 -salt s1 -stdin)"\n'
        r = subprocess.run(["bash", "-c", pre + t[a:b] + post], input=stdin, capture_output=True, text=True, timeout=60)
        return r.stdout

    def test_identical_entry_is_refused_then_a_separate_password_accepted(self):
        out = self.run_loop(f"{DISK_PW}\n{DISK_PW}\n{DISK_PW}\n{OTHER_PW}\n{OTHER_PW}\n")
        self.assertIn("MARK LOGIN-PASSWORD-REFUSED-SAME-AS-DISK", out)
        self.assertIn("only for the disk", out)
        self.assertNotIn("ACCOUNT-EQUALS-DISK", out)
        self.assertIn("UHASH=" + sha512(OTHER_PW, "s1"), out)           # the account hash is the LOGIN password's, not the passphrase's
        self.assertNotIn(sha512(DISK_PW, "s1"), out)

    def test_script_has_no_derivation_of_the_account_from_the_passphrase(self):
        t = self.text()
        self.assertNotIn("also the desktop password", t)
        self.assertNotIn("SAME passphrase", t)
        self.assertNotRegex(t, r"openssl passwd[^\n]*\$p1")
        self.assertRegex(t, r"uh=\$\(printf '%s' \"\$u1\" \| openssl passwd -6 -stdin\)")
        self.assertNotIn("root_password_hash", t.split('cat > "$ans"')[1])      # root: no password in the answer file -> locked

    def test_the_answer_file_keeps_root_locked_but_the_install_valid(self):
        c = cfg(desktop={"enable": True, "user": "canutethegreat", "password_hash": sha512(OTHER_PW)}, root_hash="", ssh_keys=[])
        errs = S.validate_config(PROFILE, c, unattended=True)
        self.assertFalse(any("unreachable" in e for e in errs), errs)

    def test_shell_syntax(self):
        subprocess.run(["bash", "-n", self.SCRIPT], check=True)

    def test_restart_needs_nothing_from_the_usb_stick(self):
        """the live system runs from the stick: after the prompt only builtins + sysrq, and the target is checked closed BEFORE the prompt"""
        t = self.text()
        end = t[t.index("hard_restart() {"):]
        self.assertIn("/proc/sysrq-trigger", end)
        self.assertLess(t.index("target_closed; then") if "target_closed; then" in t else t.index("if target_closed"), t.index("read -r _\n    mark \"RESTARTING\""))
        self.assertIn("TARGET-CLOSED", t)
        after_prompt = t[t.index('read -r _\n    mark "RESTARTING"'):t.index("mark \"INSTALL-FAILED")]
        self.assertNotRegex(after_prompt.split("exec /sbin/reboot")[0], r"(?m)^\s*(/sbin/reboot|reboot|sync|shutdown)\b")


@unittest.skipUnless(os.path.exists(os.path.join(ROOT, "build", "installer", "horcrux-installer.sh")), "reads Horcrux-only files (absent in a vendored copy, e.g. Patronus)")
class GringottsInstall(unittest.TestCase):
    PATH = os.path.join(ROOT, "build", "live-iso", "project", "variants", "gringotts", "repo", "app-misc", "gringotts-tools", "files", "usr", "bin",
                        "gringotts-install")

    def mod(self):
        src = open(self.PATH).read().replace("\nif __name__", "\nif False and __name__")
        m = type(sys)("gri")
        m.__file__ = self.PATH
        try:
            exec(compile(src, self.PATH, "exec"), m.__dict__)      # noqa: S102 (our own script, throw-away module)
        except SystemExit:
            pass
        return m

    def feed(self, m, answers):
        it = iter(answers)
        m.getpass.getpass = lambda prompt="": next(it)
        m.say = lambda *a, **k: self.said.append(" ".join(str(x) for x in a))

    def setUp(self):
        self.said = []

    def test_enter_is_not_a_shortcut_for_the_passphrase(self):
        m = self.mod()
        self.feed(m, ["", OTHER_PW, OTHER_PW])
        self.assertEqual(m.ask_login(DISK_PW), OTHER_PW)
        self.assertTrue(any("At least 8" in s for s in self.said))

    def test_passphrase_refused_as_login_password(self):
        m = self.mod()
        self.feed(m, [DISK_PW, OTHER_PW, OTHER_PW])
        self.assertEqual(m.ask_login(DISK_PW), OTHER_PW)
        self.assertTrue(any("only for unlocking the disk" in s for s in self.said))

    def test_script_text(self):
        t = open(self.PATH).read()
        self.assertNotIn("Press Enter to use the SAME", t)
        self.assertNotIn("return passphrase", t)


@unittest.skipUnless(os.path.exists(os.path.join(ROOT, "build", "installer", "horcrux-installer.sh")), "reads Horcrux-only files (absent in a vendored copy, e.g. Patronus)")
class LegacyInstaller(unittest.TestCase):
    PATH = os.path.join(ROOT, "build", "installer", "horcrux-installer.sh")

    def test_no_default_root_password_on_the_installed_system(self):
        t = open(self.PATH).read()
        self.assertNotIn('echo "root:horcrux"', t)
        self.assertNotIn("Default root password", t)

    def test_live_account_is_locked_on_the_target(self):
        self.assertIn("passwd -l horcrux", open(self.PATH).read())   # the live medium's horcrux:horcrux account must not survive the install

    def test_root_and_user_passwords_must_differ(self):
        self.assertIn("must differ", open(self.PATH).read())

    def test_shell_syntax(self):
        subprocess.run(["bash", "-n", self.PATH], check=True)


class NoBakedCredentials(unittest.TestCase):
    """installer code must never set a literal password (the live ISO build scripts' test accounts are the live medium only)"""

    def test_installer_python_has_no_literal_chpasswd(self):
        base = os.path.join(COMMON, "overlay", "usr", "lib", "live-installer")
        for dp, _dn, fn in os.walk(base):
            for f in fn:
                if f.endswith(".py"):
                    for i, line in enumerate(open(os.path.join(dp, f)), 1):
                        if "chpasswd" in line and ":horcrux" in line.lower():
                            self.fail(f"{f}:{i} sets a literal password")


@unittest.skipUnless(os.path.exists(os.path.join(ROOT, "build", "installer", "horcrux-installer.sh")), "reads Horcrux-only files (absent in a vendored copy, e.g. Patronus)")
class ShadowGate(unittest.TestCase):
    """the QEMU gate: FAILS when an installed account's hash verifies against the known test passphrase"""

    def gate(self, shadow):
        spec = importlib.util.spec_from_file_location("qt_gate", os.path.join(COMMON, "qemu-test.py"))
        qt = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(qt)
        got = {}
        qt.shadow_reuse_gate(lambda cmd: (0, shadow), lambda n, ok, info="": got.update(ok=ok, info=info), DISK_PW)
        return got

    def test_fails_when_user_hash_is_the_passphrase(self):
        got = self.gate(f"root:!:19000:::::\nowner:{sha512(DISK_PW)}:19000:0:99999:7:::\n")
        self.assertFalse(got["ok"])
        self.assertIn("REUSE: owner", got["info"])
        self.assertNotIn(DISK_PW, got["info"])

    def test_fails_when_root_hash_is_the_passphrase(self):
        got = self.gate(f"root:{sha512(DISK_PW)}:19000::::::\nowner:{sha512(OTHER_PW)}:19000::::::\n")
        self.assertFalse(got["ok"])
        self.assertIn("REUSE: root", got["info"])

    def test_passes_with_separate_secrets_and_locked_root(self):
        got = self.gate(f"root:!:19000::::::\nowner:{sha512(OTHER_PW)}:19000::::::\nbin:*:19000::::::\n")
        self.assertTrue(got["ok"], got)

    def test_fails_on_an_unverifiable_hash_rather_than_a_vacuous_pass(self):
        self.assertFalse(self.gate("owner:$zz$weird$hash:19000::::::\n")["ok"])

    def test_fails_when_shadow_cannot_be_read(self):
        self.assertFalse(self.gate("")["ok"])

    def test_every_harness_calls_the_gate(self):
        for rel in ("build/live-iso/common/zfs-enc-test.py", "gentoo/surface/tests/surface-xfce-test.py"):
            self.assertIn("shadow_reuse_gate", open(os.path.join(ROOT, rel)).read(), rel)


if __name__ == "__main__":
    unittest.main()
