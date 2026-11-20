"""Install from an ISO FILE on the target disk (dracut iso-scan/filename= + rd.live.ram=1, e.g. a remote reinstall that puts the ISO on a
spare partition of the machine's own disk): before anything touches the disks the installer releases that medium and copies the
system from the RAM copy; without the RAM copy it refuses, so it never reads from a disk it is erasing.
Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import os
import sys
import tempfile
import types
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import system as S  # noqa: E402


def fake_root(isoscan=True, ram=True):
    t = tempfile.mkdtemp()
    os.makedirs(t + "/run/initramfs")
    if isoscan:
        os.symlink("/dev/nvme0n1p3", t + "/run/initramfs/isoscandev")
    if ram:
        open(t + "/run/initramfs/squashed.img", "w").write("x")
    return t


def lsblk(cmd, *a, **k):
    if cmd[:3] == ["lsblk", "-no", "PKNAME"]:
        return 0, "nvme0n1\n"
    return 1, ""


class IsoScanRelease(unittest.TestCase):
    def inst(self):
        runs = []
        return types.SimpleNamespace(run=lambda cmd, **kw: runs.append(cmd), log=lambda m: None, runs=runs)

    def test_no_isoscan_does_nothing(self):
        i = self.inst()
        with mock.patch.object(S, "sh", side_effect=lsblk):
            self.assertFalse(S.release_isoscan_medium(i, ["/dev/nvme0n1"], root=fake_root(isoscan=False)))
        self.assertEqual(i.runs, [])

    def test_iso_on_another_disk_does_nothing(self):
        i = self.inst()
        with mock.patch.object(S, "sh", side_effect=lsblk):
            self.assertFalse(S.release_isoscan_medium(i, ["/dev/sda"], root=fake_root()))
        self.assertEqual(i.runs, [])

    def test_iso_on_target_without_ram_copy_is_refused_before_anything_runs(self):
        i = self.inst()
        with mock.patch.object(S, "sh", side_effect=lsblk):
            with self.assertRaises(S.InstallError):
                S.release_isoscan_medium(i, ["/dev/nvme0n1"], root=fake_root(ram=False))
        self.assertEqual(i.runs, [])

    def test_iso_on_target_with_ram_copy_releases_only_the_iso(self):
        i = self.inst()
        with mock.patch.object(S, "sh", side_effect=lsblk):
            self.assertTrue(S.release_isoscan_medium(i, ["/dev/nvme0n1"], root=fake_root()))
        cmds = " ; ".join(r[-1] for r in i.runs)
        self.assertIn("umount -l /run/initramfs/live", cmds)
        self.assertIn("umount -l /run/initramfs/isoscan", cmds)
        self.assertNotIn("losetup -D", cmds)          # never every loop: the running root's squashfs loop must stay
        self.assertIn("^/run/initramfs/isoscan/", cmds)

    def test_encrypted_install_discards_the_disk_first(self):
        self.assertIn("blkdiscard -f '/dev/nvme0n1'", S.discard_cmd({"zfs_encrypt": True}, "/dev/nvme0n1"))
        self.assertIn("blkdiscard", S.discard_cmd({"fs": "xfs-luks"}, "/dev/sda"))
        self.assertEqual(S.discard_cmd({"zfs_encrypt": False, "fs": "zfs"}, "/dev/sda"), "")
        self.assertEqual(S.discard_cmd({"zfs_encrypt": True, "test_machine_no_encryption": True}, "/dev/sda"), "")

    def test_ram_copy_is_preferred_by_find_squashfs(self):
        self.assertEqual(S.RAM_SQUASHFS[0], "/run/initramfs/squashed.img")


if __name__ == "__main__":
    unittest.main()
