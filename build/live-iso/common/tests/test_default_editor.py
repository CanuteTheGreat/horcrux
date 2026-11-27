"""The default editor is vim on every Horcrux and Patronus tier (owner 2026-11-27: "I hate nano as the default editor").

`default-editor` (common/overlay/usr/sbin) turns the one setting /etc/default-editor.conf (declarative key system.editor) into /etc/env.d/99editor.
These tests run it against a fake root; the QEMU gates (qemu-test.py headless_gates.editor_gate) prove the booted image.
Runs identically in the vendored copy of common/ in Patronus."""
import os
import re
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.abspath(os.path.join(HERE, ".."))
TOOL = os.path.join(COMMON, "overlay", "usr", "sbin", "default-editor")


def atoms(path):
    return [l.split("#")[0].split()[-1] for l in open(path) if l.split("#")[0].strip()]


class Root:
    def __init__(self, bins=("/usr/bin/vim", "/usr/bin/nvim", "/bin/nano", "/usr/bin/vi"), conf=None):
        self.d = tempfile.TemporaryDirectory()
        self.root = self.d.name
        for b in bins:
            os.makedirs(os.path.dirname(self.root + b), exist_ok=True)
            open(self.root + b, "w").write("#!/bin/sh\n")
            os.chmod(self.root + b, 0o755)
        os.makedirs(self.root + "/etc", exist_ok=True)
        if conf is not None:
            open(self.root + "/etc/default-editor.conf", "w").write(conf)

    def run(self, *args):
        env = dict(os.environ, DEFAULT_EDITOR_ROOT=self.root)
        return subprocess.run(["sh", TOOL, *args], env=env, capture_output=True, text=True)

    def envd(self):
        return open(self.root + "/etc/env.d/99editor").read()


class DefaultEditor(unittest.TestCase):
    def test_default_is_vim_without_any_config(self):
        r = Root()
        p = r.run("apply")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn('EDITOR="/usr/bin/vim"\nVISUAL="/usr/bin/vim"\n', r.envd())

    def test_shipped_conf_says_vim(self):
        conf = open(os.path.join(COMMON, "overlay", "etc", "default-editor.conf")).read()
        self.assertRegex(conf, r"(?m)^EDITOR_CHOICE=vim$")

    def test_nano_is_never_picked_by_default_even_when_installed(self):
        r = Root(bins=("/bin/nano", "/usr/bin/vim"))
        r.run("apply")
        self.assertNotIn("nano", r.envd())

    def test_neovim_is_a_one_setting_alternative(self):
        r = Root()
        p = r.run("set", "neovim")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn('EDITOR="/usr/bin/nvim"', r.envd())
        self.assertRegex(open(r.root + "/etc/default-editor.conf").read(), r"(?m)^EDITOR_CHOICE=neovim$")

    def test_missing_choice_falls_back_to_vim_then_vi_never_nano(self):
        r = Root(bins=("/usr/bin/vim", "/bin/nano"), conf="EDITOR_CHOICE=neovim\n")
        p = r.run("apply")
        self.assertIn('EDITOR="/usr/bin/vim"', r.envd())
        self.assertIn("not installed", p.stderr)
        r = Root(bins=("/usr/bin/vi", "/bin/nano"))
        r.run("apply")
        self.assertIn('EDITOR="/usr/bin/vi"', r.envd())

    def test_no_editor_at_all_fails_loudly_and_writes_nothing(self):
        r = Root(bins=())
        p = r.run("apply")
        self.assertNotEqual(p.returncode, 0)
        self.assertFalse(os.path.exists(r.root + "/etc/env.d/99editor"))

    def test_if_unset_keeps_an_existing_choice(self):
        r = Root()
        r.run("set", "neovim")
        r.run("set", "vim")  # conf now vim
        open(r.root + "/etc/default-editor.conf", "w").write("EDITOR_CHOICE=vim\n")
        self.assertEqual(r.run("apply", "--if-unset").returncode, 0)
        self.assertIn("/usr/bin/vim", r.envd())
        open(r.root + "/etc/env.d/99editor", "w").write('EDITOR="/bin/special"\n')
        r.run("apply", "--if-unset")
        self.assertIn("/bin/special", r.envd())

    def test_explicit_path_and_quoting_in_conf(self):
        r = Root(bins=("/usr/bin/vim", "/opt/ed/bin/e"), conf='EDITOR_CHOICE="/opt/ed/bin/e"  # mine\n')
        r.run("apply")
        self.assertIn('EDITOR="/opt/ed/bin/e"', r.envd())

    def test_format_matches_eselect_so_there_is_no_cfg_conflict(self):
        r = Root()
        r.run("apply")
        self.assertEqual(
            r.envd(),
            '# Configuration file for eselect\n# This file has been automatically generated.\nEDITOR="/usr/bin/vim"\nVISUAL="/usr/bin/vim"\n',
        )


class ImageLists(unittest.TestCase):
    def test_common_base_list_has_vim_and_not_nano(self):
        a = atoms(os.path.join(COMMON, "packages.txt"))
        self.assertIn("app-editors/vim", a)
        self.assertNotIn("app-editors/nano", a)

    def test_vim_is_not_arch_gated(self):
        line = [l for l in open(os.path.join(COMMON, "packages.txt")) if "app-editors/vim" in l and not l.lstrip().startswith("#")]
        self.assertEqual([l.strip() for l in line], ["app-editors/vim"])

    def test_setup_rootfs_applies_the_default_and_checks_it(self):
        s = open(os.path.join(COMMON, "setup-rootfs.sh")).read()
        self.assertIn("/usr/sbin/default-editor apply", s)
        self.assertRegex(s, r"EDITOR=\"/usr/bin/vim\"")

    def test_tool_is_executable_and_posix_sh(self):
        self.assertTrue(os.access(TOOL, os.X_OK))
        self.assertTrue(open(TOOL).readline().startswith("#!/bin/sh"))


if __name__ == "__main__":
    unittest.main()
