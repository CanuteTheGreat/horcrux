"""Entry points build their own args namespace: a CLI option added later must not crash the ones that never define it
(run 8771: boot-modes-test.py -> Runner.qemu_args -> AttributeError 'headless_gates'), and the live initramfs driver list
must only name modules the kernel has (aarch64 has no `vmd`)."""
import argparse
import importlib.util
import os
import re
import subprocess
import sys
import tempfile
import types
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load(name, fname):
    sys.path.insert(0, ROOT)
    spec = importlib.util.spec_from_file_location(name, os.path.join(ROOT, fname))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class ArgDefaults(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.qt = load("qemu_test_mod", "qemu-test.py")

    def test_namespace_without_the_option_gets_the_default(self):
        for ns in (argparse.Namespace(project="patronus"), types.SimpleNamespace(project="patronus")):
            self.assertFalse(hasattr(ns, "headless_gates"))
            self.qt.with_arg_defaults(ns)
            self.assertIs(ns.headless_gates, False)

    def test_an_explicit_value_is_kept(self):
        ns = argparse.Namespace(headless_gates=True)
        self.qt.with_arg_defaults(ns)
        self.assertIs(ns.headless_gates, True)

    def test_runner_accepts_a_namespace_without_it(self):
        ns = argparse.Namespace(project="patronus", arch="x86_64", mode="uefi", media="usb", flow="auto", fs="default", out=tempfile.gettempdir())
        r = self.qt.Runner(ns)
        self.assertIs(r.a.headless_gates, False)

    def test_every_namespace_built_in_a_script_still_works(self):
        # the scripts that construct Namespaces by hand are listed so a new one is noticed
        for fn in ("boot-modes-test.py", "zfs-enc-test.py"):
            src = open(os.path.join(ROOT, fn)).read()
            self.assertRegex(src, r"(Simple)?Namespace\(")


class LiveInitramfsDrivers(unittest.TestCase):
    def script(self):
        src = open(os.path.join(ROOT, "setup-rootfs.sh")).read()
        m = re.search(r"DRIVERS=\$\(python3 - \"\$kver\" <<'PY'\n(.*?)\nPY\n", src, re.S)
        self.assertTrue(m, "driver filter missing in setup-rootfs.sh")
        return m.group(1)

    def run_filter(self, dep, builtin):
        with tempfile.TemporaryDirectory() as d:
            kd = os.path.join(d, "lib", "modules", "6.18.60-x")
            os.makedirs(kd)
            open(os.path.join(kd, "modules.dep"), "w").write(dep)
            open(os.path.join(kd, "modules.builtin"), "w").write(builtin)
            code = self.script().replace("/lib/modules/", d + "/lib/modules/")
            p = subprocess.run([sys.executable, "-c", code, "6.18.60-x"], capture_output=True, text=True, timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            return p.stdout.split(), p.stderr

    DEP = "kernel/fs/squashfs/squashfs.ko.xz:\nkernel/drivers/scsi/virtio_scsi.ko.xz:\nkernel/drivers/pci/controller/vmd.ko.xz:\n"
    BUILTIN = "kernel/drivers/nvme/host/nvme.ko\nkernel/fs/xfs/xfs.ko\n"

    def test_x86_keeps_vmd(self):
        out, _ = self.run_filter(self.DEP, self.BUILTIN)
        self.assertIn("vmd", out)

    def test_arm64_without_vmd_drops_it_and_keeps_the_rest(self):
        dep = "\n".join(l for l in self.DEP.splitlines() if "vmd" not in l) + "\n"
        out, err = self.run_filter(dep, self.BUILTIN)
        self.assertNotIn("vmd", out)
        self.assertIn("vmd", err)
        for need in ("squashfs", "virtio_scsi", "nvme", "xfs"):
            self.assertIn(need, out)

    def test_build_fails_early_on_dracut_errors_and_missing_zfs(self):
        src = open(os.path.join(ROOT, "setup-rootfs.sh")).read()
        self.assertIn("dracut\\[E\\]", src)
        self.assertRegex(src, r"FATAL: the initramfs of .* has no zfs module")


if __name__ == "__main__":
    unittest.main()
