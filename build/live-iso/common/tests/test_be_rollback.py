"""horcrux-be: pins, `rollback` with no argument (= boot the previous version), and the guard that keeps user data and VM disks
outside every boot environment (pure logic, fake `zfs`/`findmnt`)."""
import os
import sys
import types
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import test_be_boot_space as T  # noqa: E402

BE = T.BE


class Rollback(T.Boot):
    def test_pinned_snapshot_survives_prune_and_destroy(self):
        self.c["KEEP"] = "2"
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            for i in range(5):
                self.snap(f"pre-upgrade-20261105-10000{i}")
                if i == 0:
                    BE.cmd_pin(self.c, "horcrux@pre-upgrade-20261105-100000")
        names = [n for _b, n, _c in self.z.snaps]
        self.assertIn("pre-upgrade-20261105-100000", names)  # pinned: kept although far beyond KEEP
        self.assertEqual(len(names), 3)  # the pin + the newest 2
        with self.assertRaises(SystemExit):
            with mock.patch.object(BE.sys, "argv", ["horcrux-be", "destroy-snapshot", "horcrux@pre-upgrade-20261105-100000"]), \
                    mock.patch.object(BE.os, "geteuid", lambda: 0), mock.patch.object(BE, "conf", lambda: self.c):
                BE.main()
        BE.cmd_pin(self.c, "horcrux@pre-upgrade-20261105-100000", False)
        self.assertEqual(self.z.pins, set())

    def test_pin_unknown_snapshot_is_an_error(self):
        with self.assertRaises(SystemExit):
            BE.cmd_pin(self.c, "horcrux@nope")

    def test_previous_snapshot_is_the_newest_automatic_one(self):
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            self.snap("manual-20261105-100000")
            self.snap("pre-upgrade-20261105-100001")
            self.snap("pre-upgrade-20261105-100002")
            self.snap("manual-20261105-100003")
        self.assertEqual(BE.previous_snapshot(self.c), "horcrux@pre-upgrade-20261105-100002")

    def test_previous_snapshot_without_any_is_an_error(self):
        with self.assertRaises(SystemExit):
            BE.previous_snapshot(self.c)


def fake_findmnt(table):
    def run(cmd, **kw):
        if cmd[0] == "findmnt":
            return types.SimpleNamespace(returncode=0, stdout=table.get(cmd[-1], "") + "\n", stderr="")
        raise AssertionError(cmd)
    return run


class DataIsolation(unittest.TestCase):
    def setUp(self):
        import tempfile
        self.t = tempfile.TemporaryDirectory()
        self.paths = {n: os.path.join(self.t.name, n) for n in ("home", "vms", "libvirt")}
        for p in self.paths.values():
            os.makedirs(p)
        self.c = BE.conf("/nonexistent")
        self.c["DATA_PATHS"] = " ".join(self.paths.values())

    def tearDown(self):
        self.t.cleanup()

    def check(self, table):
        with mock.patch.object(BE.subprocess, "run", fake_findmnt(table)):
            return BE.data_isolation(self.c)

    def test_own_datasets_are_safe(self):
        r = self.check({p: f"rpool/{n}" for n, p in self.paths.items()})
        self.assertTrue(all(x["safe"] for x in r))
        with mock.patch.object(BE.subprocess, "run", fake_findmnt({p: f"rpool/{n}" for n, p in self.paths.items()})):
            self.assertEqual(BE.isolation_guard(self.c), [])

    def test_vm_disks_on_the_boot_environment_block_a_rollback(self):
        open(os.path.join(self.paths["vms"], "100.qcow2"), "w").close()
        table = {p: f"rpool/{n}" for n, p in self.paths.items()}
        table[self.paths["vms"]] = "rpool/ROOT/horcrux"
        with mock.patch.object(BE.subprocess, "run", fake_findmnt(table)):
            with self.assertRaises(SystemExit) as e:
                BE.isolation_guard(self.c)
            self.assertIn(self.paths["vms"], str(e.exception))
            self.assertEqual(len(BE.isolation_guard(self.c, force=True)), 1)

    def test_an_empty_directory_on_the_root_is_fine_and_absent_paths_too(self):
        table = {p: "rpool/ROOT/horcrux" for p in self.paths.values()}
        self.assertTrue(all(x["safe"] for x in self.check(table)))
        self.c["DATA_PATHS"] += " /nonexistent/x"
        self.assertTrue(all(x["safe"] for x in self.check(table)))


class Installer(unittest.TestCase):
    def test_vm_storage_is_a_shared_dataset(self):
        sys.path.insert(0, os.path.join(T.HERE, "..", "overlay", "usr", "lib", "live-installer"))
        from liveinst import zfsroot as Z
        d = dict(Z.datasets())
        self.assertIn("var/lib/horcrux/vms", d)
        self.assertFalse([n for n in d if n.startswith("ROOT/") and "vms" in n])
        # Patronus uses the same installer: it must not get Horcrux's VM datasets
        p = dict(Z.datasets(False, "patronus"))
        self.assertFalse([n for n in p if "horcrux" in n and not n.startswith("ROOT/")], sorted(p))
        self.assertIn("var/log", p)


if __name__ == "__main__":
    unittest.main()
