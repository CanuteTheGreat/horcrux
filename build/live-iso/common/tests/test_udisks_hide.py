"""80-horcrux-udisks-hide.rules (sys-apps/horcrux-core): system storage is invisible to udisks2/GNOME Files, a user's USB vfat is not."""
import fnmatch
import importlib.util
import os
import re
import subprocess
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = os.path.abspath(os.path.join(ROOT, "..", "..", ".."))
PKG = os.path.join(REPO, "gentoo", "sys-apps", "horcrux-core")
RULES = os.path.join(PKG, "files", "80-horcrux-udisks-hide.rules")
spec = importlib.util.spec_from_file_location("udisks_gate", os.path.join(ROOT, "udisks_gate.py"))
UG = importlib.util.module_from_spec(spec)
spec.loader.exec_module(UG)
ESP = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"


def hidden(**kv):
    """tiny evaluator of the UDISKS_IGNORE rule lines: == / != with glob and | alternation on ENV{..} and KERNEL"""
    for l in open(RULES).read().splitlines():
        if l.startswith("#") or "UDISKS_IGNORE" not in l:
            continue
        ok = True
        for key, op, val in re.findall(r'(ENV\{\w+\}|KERNEL)(==|!=)"([^"]*)"', l):
            cur = kv.get(key[4:-1] if key.startswith("ENV") else "KERNEL", "")
            m = any(fnmatch.fnmatchcase(cur, alt) for alt in val.split("|"))
            ok = ok and (m if op == "==" else not m)
        if ok:
            return True
    return False


@unittest.skipUnless(os.path.isfile(RULES), "no horcrux-core rules")
class Rules(unittest.TestCase):
    def test_zfs_members_and_zvols(self):
        self.assertTrue(hidden(ID_FS_TYPE="zfs_member", KERNEL="nvme1n1p4"))
        self.assertTrue(hidden(KERNEL="zd0"))
        self.assertTrue(hidden(KERNEL="zd16p1"))
        self.assertFalse(hidden(KERNEL="sda1", ID_FS_TYPE="ext4"))

    def test_installer_esp_and_boot_copies(self):
        for lab in ("ESP", "ESP2", "ESP3"):
            self.assertTrue(hidden(ID_FS_TYPE="vfat", ID_PART_ENTRY_TYPE=ESP, ID_PART_ENTRY_NAME="ESP", ID_FS_LABEL=lab), lab)
        for lab in ("boot", "boot2", "boot3"):
            self.assertTrue(hidden(ID_FS_TYPE="ext4", ID_PART_ENTRY_NAME="boot", ID_FS_LABEL=lab), lab)

    def test_users_usb_stick_stays_visible(self):
        self.assertFalse(hidden(ID_FS_TYPE="vfat", ID_FS_LABEL="ESP"))                     # MBR stick that happens to be labelled ESP
        self.assertFalse(hidden(ID_FS_TYPE="vfat", ID_FS_LABEL="ESP", ID_PART_ENTRY_NAME="stick", ID_PART_ENTRY_TYPE="ebd0a0a2-b9e5-4433-87c0-68b6b72699c7"))
        self.assertFalse(hidden(ID_FS_TYPE="vfat", ID_FS_LABEL="MYSTICK", ID_PART_ENTRY_NAME="ESP", ID_PART_ENTRY_TYPE=ESP))
        self.assertFalse(hidden(ID_FS_TYPE="ext4", ID_FS_LABEL="boot"))
        self.assertFalse(hidden(ID_FS_TYPE="ext4", ID_PART_ENTRY_NAME="boot", ID_FS_LABEL="backups"))
        self.assertFalse(hidden(ID_FS_TYPE="vfat", ID_FS_LABEL="USB"))

    def test_swap(self):
        self.assertTrue(hidden(ID_FS_TYPE="crypto_LUKS", ID_PART_ENTRY_NAME="cryptswap"))
        self.assertTrue(hidden(DM_NAME="hxswap"))
        self.assertTrue(hidden(ID_FS_TYPE="swap"))
        self.assertFalse(hidden(ID_FS_TYPE="crypto_LUKS", ID_PART_ENTRY_NAME="vault"))     # the user's own LUKS stays

    def test_escape_hatch_remove_and_init_neutral(self):
        t = open(RULES).read()
        self.assertIn('ENV{HORCRUX_UDISKS_EXPOSE}=="1", GOTO="horcrux_udisks_end"', t)
        self.assertIn('ACTION=="remove"', t)
        self.assertEqual(t.count('LABEL="horcrux_udisks_end"'), 1)
        code = "\n".join(l for l in t.splitlines() if not l.startswith("#"))
        for bad in ("systemd", "RUN+=", "openrc"):
            self.assertNotIn(bad, code)


@unittest.skipUnless(os.path.isdir(PKG), "no horcrux-core package")
class Package(unittest.TestCase):
    def test_ebuild_installs_the_rules_and_reloads_udev(self):
        ebs = [f for f in os.listdir(PKG) if f.endswith(".ebuild")]
        self.assertEqual(len(ebs), 1, ebs)
        eb = open(os.path.join(PKG, ebs[0])).read()
        for needle in ("inherit udev", "/usr/lib/udev/rules.d", "80-horcrux-udisks-hide.rules", "udev_reload"):
            self.assertIn(needle, eb)
        self.assertNotRegex(ebs[0], r"-r1\.ebuild$")     # a revbump: installed -r1 hosts must pick it up


class Gate(unittest.TestCase):
    def ok(self, out):
        return UG.decide(out)[0]

    def test_decisions(self):
        self.assertTrue(self.ok("RULES 1\nIGN p1 0\nIGN p2 1\nIGN p3 1\nUDISKS-DONE\n"))
        self.assertTrue(self.ok("RULES 1\nIGN p1 0\nIGN p2 1\nIGN p3 1\nHINT p1 false\nHINT p2 true\nHINT p3 true\nUDISKS-DONE\n"))
        self.assertFalse(self.ok("RULES 1\nIGN p1 1\nIGN p2 1\nIGN p3 1\nUDISKS-DONE\n"))   # the user's stick got hidden
        self.assertFalse(self.ok("RULES 1\nIGN p1 0\nIGN p2 0\nIGN p3 1\nUDISKS-DONE\n"))   # ESP2 stayed visible
        self.assertFalse(self.ok("RULES 1\nIGN p1 0\nIGN p2 1\nIGN p3 1\nHINT p2 false\nUDISKS-DONE\n"))
        self.assertFalse(self.ok("RULES 0\nNO-TOOLS sgdisk\nUDISKS-DONE\n"))
        self.assertTrue(self.ok("NO-HORCRUX-CORE\nUDISKS-DONE\n"))   # Patronus
        self.assertFalse(self.ok(""))

    def test_script_is_valid_shell_and_wired(self):
        subprocess.run(["sh", "-n"], input=UG.CHECK, text=True, check=True)
        self.assertIn("UG.gate(", open(os.path.join(ROOT, "qemu-test.py")).read())


if __name__ == "__main__":
    unittest.main()
