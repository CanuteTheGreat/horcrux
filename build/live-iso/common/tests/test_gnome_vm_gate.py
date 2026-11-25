"""gnome-vm-gate.py: the desktop VM gate that runs the attach-mode GNOME tests on every desktop ISO (both inits). Static checks (no VM)."""
import importlib.util
import os
import re
import tomllib
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load():
    spec = importlib.util.spec_from_file_location("gvg", os.path.join(ROOT, "gnome-vm-gate.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


class Gate(unittest.TestCase):
    def test_every_registered_test_exists_and_takes_the_attach_arguments(self):
        m = load()
        self.assertIn("session-lifecycle", m.TESTS)
        for name, (script, _extra) in m.TESTS.items():
            path = os.path.join(ROOT, script)
            self.assertTrue(os.path.exists(path), name)
            src = open(path).read()
            for opt in ("--qmp", "--port", "--key", "--user", "--out"):
                self.assertIn(f'"{opt}"', src, f"{name} lacks {opt}")

    def test_ci_run_runs_the_gate_on_desktop_isos_with_the_image_init(self):
        sh = open(os.path.join(ROOT, "ci-run.sh")).read()
        m = re.search(r'name=gnome-vm-gate.*?\n(?:.*\n){0,6}', sh)
        self.assertTrue(m, "ci-run.sh does not run gnome-vm-gate")
        block = sh[sh.index("name=gnome-vm-gate") - 900: sh.index("name=gnome-vm-gate") + 900]
        self.assertIn('[ "$IMG_DE" != server ]', block)         # desktop images only
        self.assertIn('--init "$INIT"', block)                  # OpenRC and systemd images alike
        self.assertIn("GNOME_VM_TESTS:-session-lifecycle", block)
        self.assertIn("failed_list", block)                     # a red gate fails the run (and so blocks the publish)

    def test_answer_file_is_a_swapless_desktop_install(self):
        # qemu-test.py asserts "disk-no-swap-partition" for every install; a desktop gets a hibernate partition unless the answer file says none
        a = tomllib.load(open(os.path.join(ROOT, "tests/data/gnome-session-answers.toml"), "rb"))
        self.assertEqual(a["global"]["install_type"], "desktop")
        self.assertEqual(a["global"]["swap"], "none")
        self.assertEqual(a["user"]["name"], "owner")


if __name__ == "__main__":
    unittest.main()
