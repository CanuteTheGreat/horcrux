"""Static checks for the Surface profile (no VM). Run: python3 -m unittest discover -s gentoo/surface/tests"""
import importlib.machinery
import importlib.util
import os
import re
import subprocess
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

    def test_lightdm_greeter_never_runs_as_root(self):
        conf = open(os.path.join(ROOT, "..", "app-misc", "horcrux-surface-desktop", "files", "root", "etc", "lightdm", "lightdm.conf.d", "50-horcrux-surface.conf")).read()
        sect, vals = None, {}
        for line in conf.splitlines():
            line = line.strip()
            if line.startswith("[") and line.endswith("]"):
                sect = line
            elif "=" in line and not line.startswith("#"):
                vals[(sect, line.split("=")[0].strip())] = line.split("=", 1)[1].strip()
        self.assertEqual(vals.get(("[LightDM]", "greeter-user")), "lightdm")   # lab finding 2026-11-04: greeter ran as root on the Surface
        self.assertNotIn(("[Seat:*]", "autologin-user"), vals)

    def test_xfce_media_keys_are_bound(self):
        """lab finding 2026-11-05: the stock XFCE shortcut channel binds no XF86Audio* key, so the volume keys did nothing"""
        base = os.path.join(ROOT, "..", "app-misc", "horcrux-surface-desktop", "files", "root")
        sh = open(os.path.join(base, "usr", "bin", "horcrux-xfce-mediakeys")).read()
        for key, frag in (("XF86AudioRaiseVolume", "5%+"), ("XF86AudioLowerVolume", "5%-"), ("XF86AudioMute", "set-mute @DEFAULT_AUDIO_SINK@ toggle"),
                          ("XF86AudioMicMute", "@DEFAULT_AUDIO_SOURCE@")):
            self.assertRegex(sh, rf'bind {key} ".*wpctl .*{re.escape(frag)}')
        self.assertIn("xfce4-keyboard-shortcuts", sh)
        self.assertIn("|| exit 1", sh)                        # the stamp is only written when every key was set
        ds = open(os.path.join(base, "etc", "xdg", "autostart", "horcrux-xfce-mediakeys.desktop")).read()
        self.assertIn("Exec=/usr/bin/horcrux-xfce-mediakeys", ds)
        self.assertIn("OnlyShowIn=XFCE;", ds)
        subprocess.check_call(["sh", "-n", os.path.join(base, "usr", "bin", "horcrux-xfce-mediakeys")])

    def test_no_personal_overlay_in_the_profile(self):
        base = os.path.join(ROOT, "..", "app-misc", "horcrux-surface-desktop", "files", "root", "etc", "portage")
        self.assertFalse(os.path.exists(os.path.join(base, "repos.conf", "canutethegreat-overlay.conf")))
        # the Horcrux overlay's repos.conf belongs to sys-apps/horcrux-repo-config (sync-uri, auto-sync): a second copy here (auto-sync = no) would override it
        self.assertFalse(os.path.exists(os.path.join(base, "repos.conf", "horcrux.conf")))
        kw = open(os.path.join(base, "package.accept_keywords", "horcrux-surface")).read()
        lic = open(os.path.join(base, "package.license", "horcrux-surface")).read()
        for bad in ("nextcloud", "discord", "proton"):
            self.assertNotIn(bad, kw + lic)

    def test_desktop_package_has_xfce_stack(self):
        eb = open(os.path.join(ROOT, "..", "app-misc", "horcrux-surface-desktop", "horcrux-surface-desktop-1-r4.ebuild")).read()
        for atom in ("xfce-base/xfce4-meta", "x11-misc/lightdm", "app-accessibility/onboard", "xfce-base/xfce4-power-manager",
                     "www-client/firefox-bin", "app-admin/rustinion-agent",
                     "sys-firmware/intel-microcode", "sys-firmware/sof-firmware", "x11-drivers/xf86-input-wacom", "gnome-extra/iio-sensor-proxy"):
            self.assertIn(atom, eb)
        self.assertNotIn("net-vpn/tailscale", eb)                     # Tailscale retired 2026-11-10 (WireGuard hub replaces it)
        self.assertFalse(os.path.exists(os.path.join(ROOT, "root", "etc", "local.d", "50-surface-tailscale.start")))
        self.assertNotIn("gnome-base/gnome", eb)                      # owner decision: XFCE, not GNOME
        self.assertNotIn("zram-generator", eb)                        # systemd-only generator; OpenRC zram-swap instead
