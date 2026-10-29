"""Static checks for the Surface profile (no VM). Run: python3 -m unittest discover -s gentoo/surface/tests"""
import importlib.machinery
import importlib.util
import os
import tomllib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..")


def load(path, name):
    loader = importlib.machinery.SourceFileLoader(name, path)
    spec = importlib.util.spec_from_loader(name, loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


class Hidpi(unittest.TestCase):
    def test_scale(self):
        h = load(os.path.join(ROOT, "overlay/usr/bin/surface-hidpi"), "hidpi")
        self.assertEqual(h.scale_for(h.ppi_of(1920, 23.4)), 1.5)      # Surface Pro 1: 1920 px on ~23.4 cm
        self.assertEqual(h.scale_for(h.ppi_of(1920, 53.0)), 1.0)      # 24" 1080p monitor
        self.assertEqual(h.scale_for(h.ppi_of(3840, 34.4)), 2.0)
        self.assertEqual(h.scale_for(0), 1.0)                         # no EDID size (VM)

    def test_edid(self):
        h = load(os.path.join(ROOT, "overlay/usr/bin/surface-hidpi"), "hidpi2")
        blob = bytes([0, 255, 255, 255, 255, 255, 255, 0]) + bytes(13) + bytes([23, 13]) + bytes(105)
        self.assertEqual(h.edid_size_cm(blob), (23, 13))
        self.assertEqual(h.edid_size_cm(b"junk"), (0, 0))


class Profile(unittest.TestCase):
    def test_toml(self):
        p = tomllib.load(open(os.path.join(ROOT, "hwprofile.toml"), "rb"))
        self.assertEqual(p["match"]["sys_vendor"], ["Microsoft Corporation"])
        sp1 = next(m for m in p["models"] if m["id"] == "surface-pro-1")
        self.assertEqual(sp1["kernel"], "mainline")
        self.assertIn("Surface with Windows 8 Pro", sp1["product_name"])
        self.assertIn("Surface Pro", sp1["product_name"])
        self.assertIn("button.lid_init_state=open", sp1["kernel_cmdline"])
        self.assertEqual(p["defaults"]["fs"], "xfs-luks")
        for key in ("services_add", "services_del"):                   # top-level keys (a key after [defaults] would silently land in that table)
            self.assertIn(key, p)
        self.assertIn("zram-swap:default", p["services_add"])
        self.assertIn("elogind:boot", p["services_add"])

    def test_no_systemd_zram_generator_or_x11(self):
        pk = open(os.path.join(ROOT, "packages.txt")).read()
        self.assertNotIn("zram-generator", pk)                        # systemd-only generator; OpenRC zram-swap instead
        self.assertNotIn("gnome", pk)
        self.assertIn("gui-apps/wvkbd", pk)

    def test_sway_config_touch_bits(self):
        c = open(os.path.join(ROOT, "overlay/etc/sway/config")).read()
        for need in ("surface-hidpi", "wvkbd-mobintl", "surface-status", "height 48"):
            self.assertIn(need, c)

    def test_sway_config_only_uses_compiled_in_features(self):
        c = open(os.path.join(ROOT, "overlay/etc/sway/config")).read()
        self.assertNotIn("tray_output", c)        # the binpkg-matching sway is built without tray support: the line is an error

    def test_nm_powersave_off(self):
        self.assertIn("wifi.powersave = 2", open(os.path.join(ROOT, "overlay/etc/NetworkManager/conf.d/10-surface.conf")).read())

    def test_scripts_executable(self):
        for rel in ("overlay/usr/bin/surface-hidpi", "overlay/usr/bin/surface-status", "overlay/usr/bin/surface-osk",
                    "overlay/usr/bin/horcrux-surface-report", "overlay/usr/libexec/surface-session", "overlay/etc/init.d/zram-swap"):
            self.assertTrue(os.access(os.path.join(ROOT, rel), os.X_OK), rel)


if __name__ == "__main__":
    unittest.main()
