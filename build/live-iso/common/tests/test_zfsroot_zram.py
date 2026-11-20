"""ZFS installs use the image's retrying zram-swap script, not zram-init (which gave up silently on a slow first boot)."""
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import zfsroot as Z  # noqa: E402
from liveinst import initsys  # noqa: E402


class Inst:
    def __init__(self, target):
        self.target, self.ran, self.files = target, [], {}

    def write(self, path, text, mode=None, append=False):
        self.files[path] = text

    def run(self, cmd, **kw):
        self.ran.append(cmd)


class ZfsZram(unittest.TestCase):
    def _openrc(self):
        t = tempfile.mkdtemp()
        os.makedirs(t + "/sbin")
        os.symlink("/usr/sbin/openrc-init", t + "/sbin/init")
        return Inst(t)

    def test_openrc_enables_zram_swap_and_not_zram_init(self):
        i = self._openrc()
        orig, initsys.target_init = initsys.target_init, lambda root="/": "openrc"
        try:
            Z.configure_zram(i)
        finally:
            initsys.target_init = orig
        self.assertIn(["rc-update", "add", "zram-swap", "default"], i.ran)
        self.assertIn(["rc-update", "del", "zram-init", "boot"], i.ran)
        self.assertNotIn(["rc-update", "add", "zram-init", "boot"], i.ran)
        conf = i.files["/etc/conf.d/zram-swap"]
        self.assertIn("zram_percent=50", conf)
        self.assertRegex(conf, r"zram_max_mb=\d+")
        self.assertNotIn("/etc/conf.d/zram-init", i.files)


if __name__ == "__main__":
    unittest.main()
