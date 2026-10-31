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


class Profile(unittest.TestCase):
    def test_toml(self):
        p = tomllib.load(open(os.path.join(ROOT, "hwprofile.toml"), "rb"))
        self.assertEqual(p["match"]["sys_vendor"], ["Microsoft Corporation"])
        sp1 = next(m for m in p["models"] if m["id"] == "surface-pro-1")
        self.assertEqual(sp1["kernel"], "linux-surface")
        self.assertIn("Surface with Windows 8 Pro", sp1["product_name"])
        self.assertIn("Surface Pro", sp1["product_name"])
        self.assertIn("button.lid_init_state=open", sp1["kernel_cmdline"])
        self.assertEqual(p["defaults"]["fs"], "xfs-luks")
        for key in ("services_add", "services_del"):                   # top-level keys (a key after [defaults] would silently land in that table)
            self.assertIn(key, p)
        self.assertIn("zram-swap:default", p["services_add"])
        self.assertIn("elogind:boot", p["services_add"])

    def test_desktop_package_has_xfce_stack(self):
        eb = open(os.path.join(ROOT, "..", "app-misc", "horcrux-surface-desktop", "horcrux-surface-desktop-1.ebuild")).read()
        for atom in ("xfce-base/xfce4-meta", "x11-misc/lightdm", "app-accessibility/onboard", "xfce-base/xfce4-power-manager",
                     "www-client/brave-bin", "www-client/firefox-bin", "net-vpn/tailscale", "app-admin/rustinion-bin",
                     "sys-firmware/intel-microcode", "sys-firmware/sof-firmware", "x11-drivers/xf86-input-wacom", "gnome-extra/iio-sensor-proxy"):
            self.assertIn(atom, eb)
        self.assertNotIn("gnome-base/gnome", eb)                      # owner decision: XFCE, not GNOME
        self.assertNotIn("zram-generator", eb)                        # systemd-only generator; OpenRC zram-swap instead
