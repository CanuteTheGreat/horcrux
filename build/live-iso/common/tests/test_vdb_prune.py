import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
PRUNE = os.path.join(HERE, "..", "vdb-prune.py")


class VdbPrune(unittest.TestCase):
    def mkroot(self, contents, files):
        root = tempfile.mkdtemp()
        d = os.path.join(root, "var/db/pkg/dev-libs/foo-1")
        os.makedirs(d)
        open(os.path.join(d, "CONTENTS"), "w").write(contents)
        for f in files:
            p = os.path.join(root, f.lstrip("/"))
            os.makedirs(os.path.dirname(p), exist_ok=True)
            open(p, "w").write("x")
        return root, os.path.join(d, "CONTENTS")

    def run_prune(self, root, *extra):
        subprocess.check_call([sys.executable, PRUNE, *extra, root], stdout=subprocess.DEVNULL)

    def test_drops_only_allowlisted_missing_entries(self):
        c = ("dir /usr/include/foo\n"
             "obj /usr/include/foo/a.h d41d8cd98f00b204e9800998ecf8427e 1\n"
             "obj /usr/share/doc/foo/README d41d8cd98f00b204e9800998ecf8427e 1\n"
             "obj /usr/lib64/libfoo.a d41d8cd98f00b204e9800998ecf8427e 1\n"
             "obj /usr/lib64/libfoo.so.1 d41d8cd98f00b204e9800998ecf8427e 1\n"          # not allowlisted, missing: must stay
             "obj /lib/modules/6.1/kernel/drivers/net/can/dev/can-dev.ko d41d8cd98f00b204e9800998ecf8427e 1\n"  # the tar --exclude=dev/* casualty: must stay
             "obj /usr/include/kept.h d41d8cd98f00b204e9800998ecf8427e 1\n")           # present: must stay
        root, cf = self.mkroot(c, ["/usr/include/kept.h"])
        self.run_prune(root)
        left = open(cf).read()
        self.assertIn("libfoo.so.1", left)
        self.assertIn("can-dev.ko", left)
        self.assertIn("kept.h", left)
        self.assertNotIn("a.h", left)
        self.assertNotIn("README", left)
        self.assertNotIn("libfoo.a", left)

    def test_firmware_group_is_opt_in(self):
        c = "obj /lib/firmware/amdgpu/x.bin d41d8cd98f00b204e9800998ecf8427e 1\n"
        root, cf = self.mkroot(c, [])
        self.run_prune(root)
        self.assertIn("amdgpu", open(cf).read())
        self.run_prune(root, "--groups", "core,dev,gringotts-fw")
        self.assertNotIn("amdgpu", open(cf).read())


if __name__ == "__main__":
    unittest.main()
