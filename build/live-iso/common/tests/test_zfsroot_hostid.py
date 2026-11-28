"""A ZFS install's initramfs carries the installed /etc/hostid (the pool's), or the install rebuilds it / fails."""
import os
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import zfsroot as Z  # noqa: E402
from liveinst import zfsswap  # noqa: E402
from liveinst import system  # noqa: E402


class Inst:
    def __init__(self, target, hostid=b"\x01\x02\x03\x04"):
        self.target, self.ran, self.logs = target, [], []
        os.makedirs(target + "/etc", exist_ok=True)
        if hostid is not None:
            open(target + "/etc/hostid", "wb").write(hostid)

    def run(self, cmd, **kw):
        self.ran.append(cmd)
        if "line_cb" in kw:
            for l in kw.get("_out", ["zfs.ko\n"]):
                kw["line_cb"](l)

    def log(self, m):
        self.logs.append(m)


def fake_lsinitrd(contents):
    """contents: list of bytes returned by successive `lsinitrd -f` calls"""
    it = iter(contents)

    def f(cmd, **kw):
        return subprocess.CompletedProcess(cmd, 0, stdout=next(it), stderr=b"")
    return f


class Hostid(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.mkdtemp()
        os.makedirs(self.t + "/lib/modules/6.12.1-gentoo")
        self.p = mock.patch.object(zfsswap, "_newest_kver", lambda inst: "6.12.1-gentoo")
        self.p.start()
        self.addCleanup(self.p.stop)

    def test_dracut_conf_forces_the_file_into_every_initramfs(self):
        self.assertIn('install_items+=" /etc/hostid "', Z.HOSTID_DRACUT_CONF)

    def test_matching_initramfs_is_left_alone(self):
        i = Inst(self.t)
        with mock.patch("subprocess.run", fake_lsinitrd([b"\x01\x02\x03\x04"])):
            Z.ensure_initramfs_hostid(i)
        self.assertEqual(i.ran, [])

    def test_missing_hostid_triggers_one_rebuild_then_passes(self):
        i = Inst(self.t)
        with mock.patch("subprocess.run", fake_lsinitrd([b"", b"\x01\x02\x03\x04"])):
            Z.ensure_initramfs_hostid(i)
        self.assertEqual([c[0] for c in i.ran], ["dracut", "lsinitrd"])
        self.assertIn("--no-hostonly", i.ran[0])

    def test_wrong_hostid_after_rebuild_fails_the_install(self):
        i = Inst(self.t)
        with mock.patch("subprocess.run", fake_lsinitrd([b"\x09\x09\x09\x09", b"\x09\x09\x09\x09"])):
            with self.assertRaises(system.InstallError):
                Z.ensure_initramfs_hostid(i)

    def test_no_installed_hostid_is_an_internal_error(self):
        i = Inst(self.t, hostid=None)
        with self.assertRaises(system.InstallError):
            Z.ensure_initramfs_hostid(i)


class SetLiveHostid(unittest.TestCase):
    """G2: a profile/answer-file hostid is written to the live /etc/hostid (zgenhostid -f) before the pool is created."""
    class SpyInst:
        def __init__(self):
            self.ran = []

        def run(self, cmd, **kw):
            self.ran.append(cmd)

    def test_valid_hostid_calls_zgenhostid_f(self):
        i = self.SpyInst()
        Z.set_live_hostid(i, "59c06e2f")
        self.assertEqual(i.ran, [["zgenhostid", "-f", "0x59c06e2f"]])

    def test_0x_prefix_kept(self):
        i = self.SpyInst()
        Z.set_live_hostid(i, "0x59C06E2F")
        self.assertEqual(i.ran, [["zgenhostid", "-f", "0x59c06e2f"]])

    def test_empty_falls_back_to_generate(self):
        i = self.SpyInst()
        Z.set_live_hostid(i, "")
        self.assertEqual(i.ran, ["test -e /etc/hostid || zgenhostid"])

    def test_bad_hostid_refused(self):
        with self.assertRaises(system.InstallError):
            Z.set_live_hostid(self.SpyInst(), "zzzz")
        self.assertEqual(Z.hostid_problem("59c06e2f"), "")
        self.assertNotEqual(Z.hostid_problem("123456789"), "")   # 9 digits


if __name__ == "__main__":
    unittest.main()
