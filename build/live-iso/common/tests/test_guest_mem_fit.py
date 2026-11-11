"""Regression (daily gnome-light, runs 13121/13532): the desktop hibernate test started a 6144 MB guest inside a job container capped at
6 GiB; the kernel OOM-killed QEMU at 41% of the install and the harness reported "installer-finished timeout". The guest is sized to the cap."""
import importlib.util
import os
import tempfile
import unittest

COMMON = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class GuestMemFit(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        sp = importlib.util.spec_from_file_location("qt_memfit", os.path.join(COMMON, "qemu-test.py"))
        cls.qt = importlib.util.module_from_spec(sp)
        sp.loader.exec_module(cls.qt)

    def test_no_limit_or_roomy_limit_keeps_the_wanted_size(self):
        self.assertEqual(self.qt.fit_guest_mem(6144, None), (6144, ""))
        self.assertEqual(self.qt.fit_guest_mem(6144, 7168), (6144, ""))      # the 7g lanes
        self.assertEqual(self.qt.fit_guest_mem(8192, 11264), (8192, ""))

    def test_guest_equal_to_the_cap_is_shrunk_below_it(self):
        mem, note = self.qt.fit_guest_mem(6144, 6144)                        # the 6g lane that killed QEMU
        self.assertEqual(mem, 5376)
        self.assertEqual(mem % 256, 0)
        self.assertLessEqual(mem + self.qt.GUEST_HEADROOM_MB, 6144)
        self.assertIn("6144 -> 5376", note)

    def test_a_cap_too_small_for_the_floor_fails_clearly(self):
        with self.assertRaises(SystemExit) as e:
            self.qt.fit_guest_mem(6144, 4096)
        self.assertIn("guest-memory-fits-the-job-container", str(e.exception))

    def test_limit_reader(self):
        with tempfile.TemporaryDirectory() as d:
            v2, v1 = os.path.join(d, "memory.max"), os.path.join(d, "limit")
            open(v2, "w").write("6442450944\n")
            self.assertEqual(self.qt.cgroup_mem_limit_mb((v2,)), 6144)
            open(v2, "w").write("max\n")
            self.assertIsNone(self.qt.cgroup_mem_limit_mb((v2,)))
            open(v1, "w").write("9223372036854771712\n")                    # cgroup v1 "unlimited"
            self.assertIsNone(self.qt.cgroup_mem_limit_mb((v1,)))
            self.assertIsNone(self.qt.cgroup_mem_limit_mb((os.path.join(d, "missing"),)))

    def test_the_desktop_run_uses_it(self):
        src = open(os.path.join(COMMON, "zfs-enc-test.py")).read()
        self.assertIn("qt.fit_guest_mem(a.mem, qt.cgroup_mem_limit_mb())", src)


if __name__ == "__main__":
    unittest.main()
