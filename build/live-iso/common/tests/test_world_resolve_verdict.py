"""zfs-enc-test's emerge-world gate judges the full output: rebuild notices pass, real blockers fail (lab j02099/j02119 false positive)."""
import os
import re
import subprocess
import unittest

COMMON = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
SRC = open(os.path.join(COMMON, "zfs-enc-test.py")).read()
NS = {"re": re}
exec(SRC[SRC.index("WORLD_BLOCKERS ="):SRC.index("\nclass ZRun")], NS)

REBUILD = ("Calculating dependencies  ... done!\n[ebuild  N] virtual/dist-kernel-6.18.63\n"
           "!!! The following packages are causing rebuilds (warning, conflict-free):\n")


def run_cmd(full, rc):
    """Run the real WORLD_RESOLVE_CMD with `emerge` stubbed to print `full` and exit rc."""
    cmd = NS["WORLD_RESOLVE_CMD"].replace("/tmp/world.txt", "$W")
    script = f"W=$(mktemp); emerge() {{ printf '%s' \"$FULL\"; return {rc}; }}; {cmd}; rm -f $W"
    return subprocess.run(["bash", "-c", script], env={"FULL": full, "PATH": os.environ["PATH"]}, capture_output=True, text=True).stdout


class WorldResolve(unittest.TestCase):
    def verdict(self, full, rc=0):
        return NS["world_resolve_verdict"](run_cmd(full, rc))[0]

    def test_rebuild_notice_with_long_output_passes(self):
        self.assertTrue(self.verdict(REBUILD + "\n".join(f"[ebuild U] pkg/p{i}" for i in range(80)) + "\n"))

    def test_calculating_line_outside_tail_window_still_counts(self):
        self.assertTrue(self.verdict("Calculating dependencies  ... done!\n" + "\n".join(f"line {i}" for i in range(200)) + "\n"))

    def test_real_blockers_fail(self):
        for marker in ("[blocks B ] a/b (\"a/b\" is soft blocking c/d)", "x is blocking y", "masked by: missing keyword", "!!! All ebuilds that could satisfy x have been masked.",
                       "Multiple package instances within a single package slot"):
            self.assertFalse(self.verdict("Calculating dependencies  ... done!\n" + marker + "\n"), marker)

    def test_nonzero_rc_and_missing_calc_fail(self):
        self.assertFalse(self.verdict("Calculating dependencies  ... done!\n", rc=1))
        self.assertFalse(self.verdict("some other output\n"))


if __name__ == "__main__":
    unittest.main()
