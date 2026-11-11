"""horcrux-bin versioning: a new manager tarball gets a NEW Portage version, so a stale binary package can never satisfy
`emerge -u horcrux-bin` (lab 2026-11-10: FEATURES=buildpkg + --usepkg reinstalled the cached 0.1.0 binpkg and ignored the new tarball)."""
import hashlib
import os
import re
import shutil
import subprocess
import tempfile
import unittest

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
D = os.path.join(REPO, "gentoo", "app-emulation", "horcrux-bin")
HAVE = os.path.isdir(D)
H = lambda c: c * 64


def _bump(tmp, stamp, sx="", sa=""):
    args = ["bash", os.path.join(tmp, "bump-horcrux-bin.sh"), "--dir", tmp, "--stamp", stamp]
    if sx:
        args += ["--sha256-x86_64", sx]
    if sa:
        args += ["--sha256-aarch64", sa]
    return subprocess.run(args, capture_output=True, text=True)


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class BumpHorcruxBin(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        for f in ("bump-horcrux-bin.sh", "horcrux-bin-0.1.0.ebuild"):
            with open(os.path.join(D, f)) as s, open(os.path.join(self.tmp, f), "w") as o:
                o.write(s.read())

    def stamped(self):
        return sorted(f for f in os.listdir(self.tmp) if re.match(r"horcrux-bin-0\.1\.0_p\d{12}\.ebuild$", f))

    def test_each_tarball_gets_a_strictly_newer_version(self):
        self.assertEqual(_bump(self.tmp, "202611100100", H("a")).returncode, 0)
        self.assertEqual(_bump(self.tmp, "202611100200", H("b")).returncode, 0)
        self.assertEqual(self.stamped(), ["horcrux-bin-0.1.0_p202611100100.ebuild", "horcrux-bin-0.1.0_p202611100200.ebuild"])

    def test_an_older_or_equal_stamp_for_a_different_tarball_is_refused(self):
        _bump(self.tmp, "202611100200", H("a"))
        for st in ("202611100100", "202611100200"):
            r = _bump(self.tmp, st, H("b"))
            self.assertEqual(r.returncode, 2, r.stderr)
        self.assertEqual(len(self.stamped()), 1)

    def test_same_tarball_is_idempotent(self):
        _bump(self.tmp, "202611100100", H("a"), H("c"))
        r = _bump(self.tmp, "202611100999", H("a"), H("c"))
        self.assertEqual(r.returncode, 0)
        self.assertIn("unchanged", r.stdout)
        self.assertEqual(len(self.stamped()), 1)

    def test_checksums_land_in_the_ebuild_and_the_old_ones_are_pruned(self):
        for i in range(5):
            _bump(self.tmp, "20261110%04d" % (100 + i), H("0123456789abcdef"[i]))
        self.assertEqual(len(self.stamped()), 3)
        body = open(os.path.join(self.tmp, self.stamped()[-1])).read()
        self.assertIn('HORCRUX_MANAGER_SHA256_x86_64="%s"' % H("4"), body)
        self.assertIn('HORCRUX_MANAGER_SHA256_aarch64=""', body)

    def test_a_bad_stamp_or_checksum_is_rejected(self):
        self.assertEqual(_bump(self.tmp, "2026-11-10", H("a")).returncode, 64)
        self.assertEqual(_bump(self.tmp, "202611100100", "abc").returncode, 64)


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class BaseEbuild(unittest.TestCase):
    def test_the_base_ebuild_carries_the_checksum_placeholders_and_verifies_fetched_tarballs(self):
        eb = open(os.path.join(D, "horcrux-bin-0.1.0.ebuild")).read()
        self.assertRegex(eb, r'(?m)^HORCRUX_MANAGER_SHA256_x86_64=""$')
        self.assertRegex(eb, r'(?m)^HORCRUX_MANAGER_SHA256_aarch64=""$')
        self.assertIn("sha256sum", eb)

    def test_the_workflow_cuts_versions_from_the_published_tarball(self):
        wf = open(os.path.join(REPO, ".forgejo", "workflows", "bump-horcrux-bin.yml")).read()
        self.assertIn("bump-horcrux-bin.sh", wf)
        self.assertIn("horcrux-manager-daily-x86_64.tar.zst", wf)
        self.assertIn("[skip ci]", wf)


if __name__ == "__main__":
    unittest.main()

@unittest.skipUnless(HAVE and shutil.which("zstd") and shutil.which("bash"), "needs the Horcrux tree, zstd and bash")
class RepublishedAsset(unittest.TestCase):
    """The daily manager asset is republished on every main build; a stamped ebuild cut for the previous
    tarball must still install the newer one (it used to die in src_unpack and block `emerge -u @world`)."""

    def _run(self, want):
        eb = open(os.path.join(D, "horcrux-bin-0.1.0.ebuild")).read()
        m = re.search(r"(?ms)^_tarball_name\(\).*?^src_unpack\(\) \{.*?^\}\n", eb)
        self.assertIsNotNone(m, "src_unpack not found")
        with tempfile.TemporaryDirectory() as w:
            tree = os.path.join(w, "tree")
            for d in ("usr/bin", "usr/share/horcrux/ui"):
                os.makedirs(os.path.join(tree, d))
            for b in ("usr/bin/horcrux-api", "usr/bin/horcrux"):
                open(os.path.join(tree, b), "w").write("#!/bin/sh\n")
                os.chmod(os.path.join(tree, b), 0o755)
            tb = os.path.join(w, "fetched.tar.zst")
            subprocess.run(["tar", "--zstd", "-cf", tb, "-C", tree, "."], check=True)
            real = hashlib.sha256(open(tb, "rb").read()).hexdigest()
            want = real if want is None else want
            script = f"""
set -u
einfo() {{ echo "einfo: $*"; }}
ewarn() {{ echo "ewarn: $*"; }}
die() {{ echo "die: $*"; exit 1; }}
curl() {{ while [ $# -gt 1 ]; do [ "$1" = -o ] && out=$2; shift; done; cp {tb} "$out"; }}
uname() {{ echo x86_64; }}
PVR=0.1.0_p1
EPREFIX={w}/none
T={w}; WORKDIR={w}/work; S=$WORKDIR/root
HORCRUX_MANAGER_SHA256_x86_64={want}
HORCRUX_MANAGER_TARBALL=
HORCRUX_MANAGER_URL=
{m.group(0)}
src_unpack
echo "fetched=${{HORCRUX_FETCHED_SHA256:-}}"
test -x $S/usr/bin/horcrux-api
"""
            r = subprocess.run(["bash", "-c", script], capture_output=True, text=True)
            return r, real

    def test_a_republished_tarball_installs_with_a_warning(self):
        r, got = self._run("0" * 64)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("ewarn: the daily manager tarball was republished", r.stdout)
        self.assertIn("fetched=" + got, r.stdout)

    def test_the_cut_for_tarball_installs_without_a_warning(self):
        r, _ = self._run(None)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertNotIn("ewarn", r.stdout)
