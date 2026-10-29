"""Unit tests for liveinst/portableswap.py (the shared portable-swap stage) and its xfs-luks/LVM hooks. No disks touched.
Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import portableswap as PS  # noqa: E402
from liveinst import luksroot as LR  # noqa: E402
from liveinst import system as S  # noqa: E402

PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["xfs-luks", "ext4", "xfs", "zfs"], "swap_mb": 4096, "network_mode": "management",
           "services": [], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}
SURFACE = {"id": "surface", "defaults": {"fs": "xfs-luks", "swap": "hibernate"}}


def fake(tmp, chassis=None, supplies=()):
    dmi, ps = os.path.join(tmp, "dmi"), os.path.join(tmp, "ps")
    os.makedirs(dmi)
    os.makedirs(ps)
    if chassis is not None:
        open(os.path.join(dmi, "chassis_type"), "w").write(f"{chassis}\n")
    for name, typ, scope in supplies:
        d = os.path.join(ps, name)
        os.makedirs(d)
        open(os.path.join(d, "type"), "w").write(typ + "\n")
        if scope:
            open(os.path.join(d, "scope"), "w").write(scope + "\n")
    return dmi, ps


class Detect(unittest.TestCase):
    def check(self, chassis, supplies=()):
        with tempfile.TemporaryDirectory() as t:
            dmi, ps = fake(t, chassis, supplies)
            return PS.detect_portable(dmi, ps)

    def test_laptop_chassis_types_are_portable(self):
        for ct in (8, 9, 10, 11, 14, 30, 31, 32):
            ok, why = self.check(ct)
            self.assertTrue(ok, ct)
            self.assertIn(f"chassis type {ct}", why)

    def test_server_and_desktop_chassis_are_not(self):
        for ct in (1, 2, 3, 4, 5, 6, 7, 13, 17, 23, 25):
            self.assertFalse(self.check(ct)[0], ct)
        self.assertFalse(self.check(None)[0])                      # QEMU / no DMI

    def test_system_battery_counts_even_with_a_generic_chassis(self):
        ok, why = self.check(1, [("BAT0", "Battery", "System")])
        self.assertTrue(ok)
        self.assertIn("BAT0", why)
        self.assertTrue(self.check(None, [("BAT1", "Battery", None)])[0])    # old kernels have no scope file

    def test_peripheral_batteries_and_ac_do_not_count(self):
        self.assertFalse(self.check(3, [("hid-aa:bb-battery", "Battery", "Device"), ("AC", "Mains", None), ("ups", "UPS", None)])[0])


class Plan(unittest.TestCase):
    def laptop(self):
        return True, "DMI chassis type 10 (Notebook)"

    def server(self):
        return False, "no battery, chassis type 1"

    def test_size_is_ram_x_1_125_rounded_up_to_gib(self):
        self.assertEqual(PS.swap_mb(3900), 4608)       # Surface Pro 1: 4 GB installed, MemTotal ~3.8 GB -> 4.5 GiB
        self.assertEqual(PS.swap_mb(4096), 4608)
        self.assertEqual(PS.swap_mb(7800), 9216)       # 8 GB -> 9 GiB
        self.assertEqual(PS.swap_mb(15800), 18432)
        self.assertEqual(PS.swap_mb(512), 2048)        # floor
        self.assertEqual(PS.swap_mb(10 ** 7), PS.MAX_SWAP_MB)
        self.assertGreaterEqual(PS.swap_mb(3900), 3900)  # always at least RAM

    def test_auto_follows_detection(self):
        p = PS.plan({"fs": "xfs-luks"}, 3900, detect=self.laptop)
        self.assertEqual((p["mode"], p["mb"]), ("hibernate", 4608))
        self.assertIn("portable", p["why"])
        p = PS.plan({"fs": "xfs-luks"}, 3900, detect=self.server)
        self.assertEqual((p["mode"], p["mb"]), ("none", 0))
        self.assertIn("server", p["why"])

    def test_hardware_profile_and_desktop_type_are_portable(self):
        p = PS.plan({"fs": "xfs-luks"}, 3900, SURFACE, detect=self.server)
        self.assertEqual(p["mode"], "hibernate")
        self.assertIn("surface", p["why"])
        p = PS.plan({"fs": "xfs-luks", "install_type": "desktop"}, 3900, detect=self.server)
        self.assertEqual(p["mode"], "hibernate")
        self.assertIn("desktop", p["why"])
        p = PS.plan({"fs": "xfs-luks", "desktop": {"enable": True}}, 3900, detect=self.server)
        self.assertEqual(p["mode"], "hibernate")

    def test_overrides_win_both_ways(self):
        self.assertEqual(PS.plan({"fs": "xfs-luks", "swap": "none"}, 3900, SURFACE, detect=self.laptop)["mode"], "none")
        self.assertEqual(PS.plan({"fs": "xfs-luks", "swap": "NONE", "install_type": "desktop"}, 3900, detect=self.laptop)["mode"], "none")
        p = PS.plan({"fs": "xfs-luks", "swap": "hibernate"}, 3900, detect=self.server)
        self.assertEqual((p["mode"], p["mb"]), ("hibernate", 4608))
        self.assertEqual(PS.plan({"fs": "xfs-luks", "swap": "bogus"}, 3900, detect=self.server)["mode"], "none")   # unknown = auto
        self.assertTrue(PS.validate({"swap": "bogus"}))
        self.assertEqual(PS.validate({"swap": "auto"}) + PS.validate({"swap": "none"}) + PS.validate({"swap": "hibernate"}) + PS.validate({}), [])

    def test_server_default_is_never_swap(self):
        for fs in ("xfs-luks", "ext4", "xfs", "zfs"):
            self.assertEqual(PS.plan({"fs": fs}, 64000, detect=self.server)["mode"], "none")

    def test_zfs_only_with_the_encrypted_layout(self):
        p = PS.plan({"fs": "zfs", "swap": "hibernate"}, 3900, detect=self.laptop)
        self.assertEqual(p["mode"], "none")              # zfsroot has no PORTABLE_SWAP in this tree
        self.assertIn("no portable-swap support", p["why"])
        for fs in ("xfs-luks", "ext4", "xfs"):
            self.assertEqual(PS.plan({"fs": fs, "swap": "hibernate"}, 3900)["mode"], "hibernate")

    def test_disk_fit(self):
        self.assertIsNone(PS.fit_problem(4608, 0))             # unknown size: do not block
        self.assertIsNone(PS.fit_problem(4608, 24 * 1024))
        self.assertIn("less than 16 GiB", PS.fit_problem(4608, 16 * 1024))

    def test_describe(self):
        self.assertIn("4.5 GiB", PS.describe({"mode": "hibernate", "mb": 4608, "why": "portable: battery BAT0 present"}))
        self.assertTrue(PS.describe({"mode": "none", "mb": 0, "why": "server: x"}).startswith("none"))


class Tools(unittest.TestCase):
    def test_missing_lvm_tools_are_reported(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(PS.missing_tools({"fs": "xfs-luks"}, path=d), list(PS.LVM_TOOLS))
            self.assertEqual(PS.missing_tools({"fs": "ext4"}, path=d), [])          # plain swap partition needs no LVM
            for t in PS.LVM_TOOLS:
                p = os.path.join(d, t)
                open(p, "w").write("#!/bin/sh\n")
                os.chmod(p, 0o755)
            self.assertEqual(PS.missing_tools({"fs": "xfs-luks"}, path=d), [])


class Config(unittest.TestCase):
    def test_answer_file_swap_key(self):
        for val, want in (("hibernate", "hibernate"), ("None", "none"), ("auto", "auto")):
            with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
                f.write(f'[global]\nroot_password_hash = "$6$s$' + "a" * 86 + f'"\n[disk]\nfilesystem = "xfs-luks"\nswap = "{val}"\ndisks = ["/dev/vda"]\n')
            c = S.config_from_answers(PROFILE, f.name)
            os.unlink(f.name)
            self.assertEqual(c["swap"], want)
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[global]\nroot_password_hash = "x"\n[disk]\nfilesystem = "xfs-luks"\n')
        self.assertEqual(S.config_from_answers(PROFILE, f.name)["swap"], "auto")      # default
        os.unlink(f.name)
        self.assertEqual(S.default_config(PROFILE)["swap"], "auto")

    def test_validate_flags_a_bad_swap_value(self):
        c = S.default_config(PROFILE)
        c.update(disks=["/dev/vda"], fs="xfs-luks", luks_passphrase="x" * 14, root_hash="$6$s$" + "a" * 86, swap="sometimes")
        self.assertTrue(any("swap" in e for e in S.validate_config(PROFILE, c)))
        c["swap"] = "hibernate"
        self.assertFalse(any("swap" in e for e in S.validate_config(PROFILE, c)))

    def test_surface_profile_asks_for_hibernate(self):
        import tomllib
        p = tomllib.load(open(os.path.join(HERE, "..", "..", "..", "..", "gentoo", "surface", "hwprofile.toml"), "rb"))
        self.assertEqual(p["defaults"]["swap"], "hibernate")
        self.assertEqual(PS.plan({"fs": "xfs-luks"}, 3900, p, detect=lambda: (False, "x"))["mode"], "hibernate")


class Text(unittest.TestCase):
    def test_fstab_line_zram_has_higher_priority(self):
        l = PS.fstab_line("/dev/mapper/hxvg-swap")
        self.assertIn("swap", l)
        self.assertIn(f"pri={PS.SWAP_PRIO}", l)
        self.assertLess(PS.SWAP_PRIO, 100)                # zram-swap.init uses --priority 100

    def test_dracut_and_resume_arg(self):
        self.assertIn("resume", PS.dracut_conf())
        self.assertIn("lvm", PS.dracut_conf("lvm dm"))
        self.assertEqual(PS.resume_arg("aaaa-bbbb"), "resume=UUID=aaaa-bbbb")

    def test_power_policy(self):
        self.assertIn("HandleLidSwitch=suspend", PS.LOGIND_CONF)
        self.assertIn("HandleLidSwitchDocked=ignore", PS.LOGIND_CONF)
        self.assertIn("HandlePowerKey=suspend", PS.LOGIND_CONF)
        self.assertNotIn("HandleLidSwitch=hibernate", PS.LOGIND_CONF)
        self.assertIn("HibernateMode=shutdown", PS.SLEEP_CONF)

    def test_upower_conf_rewrites_keys_and_keeps_the_rest(self):
        orig = "[UPower]\nEnableWattsUpPro=false\nUsePercentageForPolicy=false\n#PercentageAction=2\nPercentageAction=2\nCriticalPowerAction=HybridSleep\nIgnoreLid=false\n"
        out = PS.upower_conf(orig)
        self.assertIn("CriticalPowerAction=Hibernate", out)
        self.assertIn("UsePercentageForPolicy=true", out)
        self.assertEqual(out.count("PercentageAction="), 1)
        self.assertIn("EnableWattsUpPro=false", out)
        self.assertIn("IgnoreLid=false", out)
        self.assertNotIn("HybridSleep", out)
        self.assertEqual(PS.upower_conf(out), out)       # idempotent
        bare = PS.upower_conf("[UPower]\nIgnoreLid=false\n")
        self.assertIn("PercentageCritical=7", bare)
        self.assertTrue(PS.upower_conf("").startswith("[UPower]"))

    def test_initramfs_resume_check(self):
        self.assertEqual(PS.verify_initramfs("usr/lib/dracut/hooks/cmdline/10-parse-resume.sh\n"), [])
        self.assertEqual(PS.verify_initramfs("usr/lib/dracut/hooks/pre-mount/10-resume.sh\n"), [])
        self.assertEqual(PS.verify_initramfs("cryptsetup\n"), ["resume hook"])


class LuksLvm(unittest.TestCase):
    class Inst:
        swap_mode, swap_mb = "hibernate", 4608

    class Srv:
        swap_mode, swap_mb = "none", 0

    def test_server_layout_is_unchanged(self):
        self.assertFalse(LR.lvm_on(self.Srv()))
        a = LR.kernel_args("U-U-I-D")
        self.assertIn("root=/dev/mapper/hxroot", a)
        self.assertNotIn("lvm", a)
        self.assertNotIn("resume", a)
        self.assertNotIn("swap", LR.fstab_text("UUID=r", "b", "e"))

    def test_portable_layout(self):
        self.assertTrue(LR.lvm_on(self.Inst()))
        a = LR.kernel_args("U-U-I-D", "resume=UUID=S", lvm=True)
        for part in ("rd.luks.uuid=U-U-I-D", "rd.lvm.vg=hxvg", "root=/dev/mapper/hxvg-root", "resume=UUID=S", "rd.luks.allow-discards=U-U-I-D"):
            self.assertIn(part, a)
        self.assertEqual(a.count("rd.luks.uuid"), 1)       # ONE unlock for root and swap
        f = LR.fstab_text("UUID=r", "b", "e", True, LR.SWAP_LV)
        self.assertIn("/dev/mapper/hxvg-swap\tnone\tswap\tsw,pri=10", f)

    def test_initramfs_verify_requires_lvm_and_resume_only_when_portable(self):
        listing = "cryptsetup\ndm-crypt\nusbhid\nxhci-pci\nhid-multitouch\nhid-microsoft\n"
        self.assertEqual(LR.verify_initramfs(listing), [])
        self.assertEqual(set(LR.verify_initramfs(listing, lvm=True)), {"lvm", "resume hook"})
        self.assertEqual(LR.verify_initramfs(listing + "usr/sbin/lvm\nhooks/pre-mount/10-resume.sh\n", lvm=True), [])

    def test_dm_mod_is_forced_into_the_initramfs(self):
        self.assertIn("dm_mod", LR.INITRAMFS_DRIVERS.split())


if __name__ == "__main__":
    unittest.main()
