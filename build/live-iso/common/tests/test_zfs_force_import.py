"""zfsforce=1 is no longer on every ZFS-root boot entry (it defeated ZFS's "pool is in use by another host" check and hid host id
mismatches). be.conf ZFS_FORCE_IMPORT = auto (default) | yes | no; auto forces only entries whose initramfs host id is not the pool's."""
import os
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import test_be_boot_space as T  # noqa: E402

BE = T.BE
HID = (0x12345678).to_bytes(4, sys.byteorder)


def menu(text):
    """{menu id: has zfsforce} of the kernel entries"""
    out, cur = {}, None
    for line in text.splitlines():
        if line.startswith("menuentry"):
            cur = line.split("--id ")[1].split()[0]
        elif line.strip().startswith("linux ") and cur:
            out[cur] = "zfsforce=1" in line
    return out


class Render(unittest.TestCase):
    def cfg(self, **kw):
        return BE.render_grub(["horcrux"], "horcrux", "rpool", "1111-2222", kernels={"horcrux": "7.2.1"}, previous={"horcrux": ["7.2.0"]},
                              good={"horcrux": "7.2.1"}, **kw)

    def test_default_has_no_force(self):
        self.assertEqual(menu(self.cfg()), {"be-horcrux": False, "be-horcrux-k-7.2.0": False})

    def test_force_all_and_per_entry(self):
        self.assertEqual(menu(self.cfg(force=True)), {"be-horcrux": True, "be-horcrux-k-7.2.0": True})
        self.assertEqual(menu(self.cfg(force={"be-horcrux": False, "be-horcrux-k-7.2.0": True})), {"be-horcrux": False, "be-horcrux-k-7.2.0": True})
        self.assertTrue(menu(self.cfg(force={}))["be-horcrux"])          # unknown entry in a plan = the safe side

    def test_no_double_space_without_force(self):
        self.assertNotIn("  ro", self.cfg().replace("    linux", "linux"))


class Plan(unittest.TestCase):
    def plan(self, conf, ph, img):
        return BE.force_plan(conf, "/boot", "rpool", ["horcrux"], {}, {"horcrux": ["7.2.0"]}, pool_hid=ph, img_hid=lambda p: img.get(p))

    def test_mode_parsing(self):
        for v, want in (("yes", "yes"), ("NO", "no"), ("auto", "auto"), ("", "auto"), ("bogus", "auto")):
            self.assertEqual(BE.force_mode({"ZFS_FORCE_IMPORT": v}), want)
        self.assertEqual(BE.force_mode({}), "auto")
        self.assertEqual(BE.conf("/nonexistent")["ZFS_FORCE_IMPORT"], "auto")      # an existing install without the key

    def test_yes_no(self):
        self.assertIs(self.plan({"ZFS_FORCE_IMPORT": "yes"}, 1, {}), True)
        self.assertIs(self.plan({"ZFS_FORCE_IMPORT": "no"}, 1, {}), False)

    def test_auto_matching_hostid_drops_force(self):
        img = {"/boot/be/horcrux/initramfs.img": 77, "/boot/be/horcrux/kernels/7.2.0/initramfs.img": 77}
        self.assertEqual(self.plan({}, 77, img), {"be-horcrux": False, "be-horcrux-k-7.2.0": False})

    def test_auto_keeps_force_where_it_cannot_import_cleanly(self):
        img = {"/boot/be/horcrux/initramfs.img": 77, "/boot/be/horcrux/kernels/7.2.0/initramfs.img": 99}      # older initramfs: other host id
        self.assertEqual(self.plan({}, 77, img), {"be-horcrux": False, "be-horcrux-k-7.2.0": True})
        self.assertEqual(self.plan({}, 77, {}), {"be-horcrux": True, "be-horcrux-k-7.2.0": True})               # initramfs has no host id
        self.assertEqual(self.plan({}, 0, img), {"be-horcrux": True, "be-horcrux-k-7.2.0": True})            # pool host id unknown


class HostId(unittest.TestCase):
    def test_hostid_of(self):
        self.assertEqual(BE.hostid_of(HID), 0x12345678)
        for bad in (b"", b"\0\0\0\0", b"abc", b"abcde", None):
            self.assertIsNone(BE.hostid_of(bad))

    def test_parse_zdb(self):
        self.assertEqual(BE.parse_hostid("    version: 5000\n    hostid: 305419896\n    hostname: 'x'\n"), 305419896)
        self.assertIsNone(BE.parse_hostid("    hostid: 0\n"))
        self.assertIsNone(BE.parse_hostid("nothing"))

    def test_pool_hostid_falls_back_to_label(self):
        def run(cmd, **kw):
            r = mock.Mock(returncode=0, stderr="")
            if cmd[:2] == ["zdb", "-C"]:
                r.returncode, r.stdout = 1, ""
            elif cmd[0] == "zpool":
                r.stdout = "rpool\t10G\t1G\n\t/dev/vda4\t10G\t1G\n"
            else:
                r.stdout = "    hostid: 42\n"
            return r
        with mock.patch.object(BE.subprocess, "run", run):
            self.assertEqual(BE.pool_hostid("rpool"), 42)


class Installer(unittest.TestCase):
    def inst(self, mode, initrd_hid, sys_hid=HID):
        from importlib import import_module
        sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "overlay", "usr", "lib", "live-installer"))
        zr = import_module("liveinst.zfsroot")
        inst = mock.Mock(target="/nonexistent", c={"zfs_force_import": mode, "zfs_pool": "rpool"})
        return zr, inst, mock.patch.object(zr, "_initramfs_hostid", lambda i, img: initrd_hid), \
            mock.patch.object(BE, "pool_hostid", lambda p: BE.hostid_of(sys_hid))

    def test_static_grub_arg(self):
        for mode, ih, want in (("auto", HID, ""), ("auto", b"", " zfsforce=1"), ("auto", b"\x01\x02\x03\x04", " zfsforce=1"),
                               ("yes", HID, " zfsforce=1"), ("no", b"", "")):
            zr, inst, p1, p2 = self.inst(mode, ih)
            with p1, p2:
                self.assertEqual(zr.force_arg(inst, "7.2.1"), want, (mode, ih))

    def test_be_conf_written_with_the_setting(self):
        zr, inst, *_ = self.inst("no", b"")
        self.assertIn('ZFS_FORCE_IMPORT="no"', zr.BE_CONF.format(pool="rpool", cmdline="", kglob="*", force="no"))


class Rescue(unittest.TestCase):
    def test_rescue_keeps_force_by_default(self):
        sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "overlay", "usr", "lib", "live-installer"))
        from rescue import scan
        self.assertIn("zfsforce=1", scan.static_zfs_grubcfg("H", "6.1", "u", "rpool/ROOT", ""))
        self.assertNotIn("zfsforce", scan.static_zfs_grubcfg("H", "6.1", "u", "rpool/ROOT", "", force=False))


if __name__ == "__main__":
    unittest.main()
