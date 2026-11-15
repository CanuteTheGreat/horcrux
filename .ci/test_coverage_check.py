#!/usr/bin/env python3
"""Tests for coverage-check.py (run: python3 .ci/test_coverage_check.py)."""
import os, shutil, subprocess, sys, tempfile, textwrap, unittest

CI = os.path.dirname(os.path.abspath(__file__))


def run(files, ebuilds):
    d = tempfile.mkdtemp()
    try:
        os.makedirs(f"{d}/.ci"); os.makedirs(f"{d}/.nvchecker")
        for f in ("update-check.py", "security-check.py", "coverage-check.py"):
            shutil.copy(f"{CI}/{f}", f"{d}/.ci/{f}")
        for e in ebuilds:
            cat, pn = e.split("/")
            os.makedirs(f"{d}/{cat}/{pn}")
            open(f"{d}/{cat}/{pn}/{pn}-1.ebuild", "w").write("EAPI=8\n")
        for n, c in files.items():
            open(f"{d}/.nvchecker/{n}", "w").write(textwrap.dedent(c))
        r = subprocess.run([sys.executable, ".ci/coverage-check.py"], cwd=d, capture_output=True, text=True,
                           env={k: v for k, v in os.environ.items() if k != "OVERLAY_DIRS"})
        return r.returncode, r.stdout
    finally:
        shutil.rmtree(d)


NV = '''
["a/watched"]
source = "github"
github = "o/r"
["a/nosec"]
source = "cmd"
cmd = "echo 1"
["a/withnone"]
source = "cmd"
cmd = "echo 1"
'''


class T(unittest.TestCase):
    def test_all_good(self):
        rc, out = run({"nvchecker.toml": NV, "security.toml": '["a/withnone"]\nnone = "closed source"\n',
                       "excluded.txt": "b/own-*  # first-party\n"}, ["a/watched", "a/withnone", "b/own-x"])
        self.assertEqual(rc, 0, out)

    def test_unwatched_fails(self):
        rc, out = run({"nvchecker.toml": NV, "excluded.txt": ""}, ["a/watched", "c/new"])
        self.assertEqual(rc, 1)
        self.assertIn("c/new: unwatched", out)

    def test_nvchecker_without_security_mapping_fails(self):
        rc, out = run({"nvchecker.toml": NV}, ["a/nosec"])
        self.assertEqual(rc, 1)
        self.assertIn("a/nosec: nvchecker entry but no security-feed", out)

    def test_exemption_needs_reason(self):
        rc, out = run({"nvchecker.toml": NV, "excluded.txt": "b/own-*\n"}, ["b/own-x"])
        self.assertEqual(rc, 1)
        self.assertIn("no reason", out)

    def test_security_only_needs_flag(self):
        rc, out = run({"nvchecker.toml": NV, "security.toml": '["d/pin"]\ngithub = "o/p"\n'}, ["d/pin"])
        self.assertEqual(rc, 1)
        rc, out = run({"nvchecker.toml": NV, "security.toml": '["d/pin"]\ngithub = "o/p"\nno_version_check = "commit-pinned"\n'}, ["d/pin"])
        self.assertEqual(rc, 0, out)


if __name__ == "__main__":
    unittest.main()
