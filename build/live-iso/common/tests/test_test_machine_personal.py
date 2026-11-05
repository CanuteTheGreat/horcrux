"""Test machines carry NO personal apps/data: horcrux-test-machine-check (denylist + world allowlist) and the policy's consistency."""
import glob
import os
import re
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
FILES = os.path.join(REPO, "gentoo", "app-misc", "horcrux-test-machine", "files")
CHECK = os.path.join(FILES, "horcrux-test-machine-check")
HAVE = os.path.exists(CHECK)


def mk(root, rel, text=""):
    p = os.path.join(root, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    open(p, "w").write(text)


def run(root):
    r = subprocess.run([sys.executable, CHECK, "--root", root, "--policy", FILES], capture_output=True, text=True)
    return r.returncode, r.stdout


@unittest.skipUnless(HAVE, "needs the Horcrux repo tree (not the vendored common/ in Patronus)")
class TestPersonalGuard(unittest.TestCase):
    def clean_root(self):
        d = tempfile.mkdtemp()
        mk(d, "var/lib/portage/world", "app-misc/horcrux-test-machine\nsys-fs/zfs\nwww-client/firefox-bin\nnet-vpn/tailscale\nxfce-base/xfce4-meta\n"
                                       "sys-firmware/apple-t2-wifi-firmware\napp-misc/hogshead-machine\n")
        mk(d, "var/db/pkg/www-client/firefox-bin-159.0/PF", "x")
        mk(d, "home/tester/.config/mozilla/firefox/profiles.ini", "x")
        return d

    def test_clean_machine_passes(self):
        rc, out = run(self.clean_root())
        self.assertEqual(rc, 0, out)

    def test_every_personal_app_fails(self):
        for atom, ver in (("www-client/brave-bin", "1.98.63"), ("net-misc/nextcloud-client", "4.0.4"), ("net-im/discord", "1.0.168"),
                          ("net-im/signal-desktop-bin", "7.1"), ("media-sound/spotify", "1.2"), ("app-admin/keepassxc", "2.7.9-r1"),
                          ("app-admin/proton-pass-bin", "1.2"), ("net-vpn/proton-vpn-gtk-app", "4.17.1")):
            d = self.clean_root()
            mk(d, f"var/db/pkg/{atom}-{ver}/PF", "x")
            rc, out = run(d)
            self.assertEqual(rc, 1, atom)
            self.assertIn(f"VIOLATION package {atom}", out)

    def test_world_allowlist_catches_unknown_atoms(self):
        d = self.clean_root()
        mk(d, "var/lib/portage/world", "app-misc/horcrux-test-machine\ngames-fps/some-game\n=net-misc/something-new-1.0:0\n")
        rc, out = run(d)
        self.assertEqual(rc, 1)
        self.assertIn("world-not-allowlisted games-fps/some-game", out)
        self.assertIn("world-not-allowlisted net-misc/something-new", out)

    def test_leftover_config_and_data_fail(self):
        for rel in ("home/tester/.config/Nextcloud/nextcloud.cfg", "home/tester/Nextcloud/x", "home/tester/.config/BraveSoftware/Brave-Browser/Local State",
                    "home/tester/.config/autostart/Nextcloud.desktop", "home/tester/.config/discord/Cookies", "opt/brave.com/brave/brave",
                    "usr/share/applications/discord.desktop", "etc/xdg/autostart/nextcloud.desktop", "home/tester/.ssh/id_ed25519",
                    "root/.config/Signal/x", "etc/ctg-backup/env", "var/lib/flatpak/app/com.spotify.Client/x", "opt/Thing.AppImage"):
            d = self.clean_root()
            mk(d, rel, "x")
            rc, out = run(d)
            self.assertEqual(rc, 1, rel)

    def test_agent_ssh_authorized_keys_and_firefox_profile_are_fine(self):
        d = self.clean_root()
        mk(d, "home/tester/.ssh/authorized_keys", "ssh-ed25519 AAAA agent")
        mk(d, "root/.ssh/authorized_keys", "ssh-ed25519 AAAA agent")
        self.assertEqual(run(d)[0], 0)

    def test_missing_policy_is_not_a_pass(self):
        r = subprocess.run([sys.executable, CHECK, "--policy", "/nonexistent"], capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)


@unittest.skipUnless(HAVE, "needs the Horcrux repo tree")
class TestPolicyConsistency(unittest.TestCase):
    def lines(self, f):
        return [x.split("#")[0].strip() for x in open(os.path.join(FILES, f)) if x.split("#")[0].strip()]

    def test_owner_layer_apps_are_denied_except_agent_plumbing(self):
        sys.path.insert(0, os.path.dirname(CHECK))
        import importlib.machinery
        import importlib.util
        loader = importlib.machinery.SourceFileLoader("htmc", CHECK)
        spec = importlib.util.spec_from_loader("htmc", loader)
        m = importlib.util.module_from_spec(spec)
        loader.exec_module(m)
        deny = self.lines("personal-apps.denylist")
        plumbing = {"net-vpn/tailscale", "app-admin/rustinion-agent", "app-admin/rustinion-bin"}
        owner = os.path.join(REPO, "build", "live-iso", "project")
        atoms = []
        for f in (os.path.join(owner, "variants", "owner-common", "packages.txt"), os.path.join(owner, "public-forbidden.txt")):
            atoms += [x.split("#")[0].split()[0] for x in open(f) if x.split("#")[0].strip()]
        for a in atoms:
            if a in plumbing:
                continue
            self.assertTrue(m.matches(a, deny), f"{a} (an owner/forbidden app) is not on the test-machine denylist")

    def test_allowlist_never_allows_a_denied_package(self):
        import fnmatch
        deny = self.lines("personal-apps.denylist")
        for pat in self.lines("world.allowlist"):
            for d in deny:
                if "*" not in pat:
                    self.assertFalse(fnmatch.fnmatchcase(pat, d), f"allowlist {pat} is denied by {d}")

    def test_ebuild_ships_policy_and_blocks_the_big_ones(self):
        eb = open(sorted(glob.glob(os.path.join(REPO, "gentoo", "app-misc", "horcrux-test-machine", "*.ebuild")))[-1]).read()
        for f in ("personal-apps.denylist", "personal-paths.denylist", "world.allowlist", "horcrux-test-machine-check"):
            self.assertIn(f, eb)
        for b in ("www-client/brave-bin", "net-misc/nextcloud-client", "net-im/discord", "media-sound/spotify", "app-admin/keepassxc"):
            self.assertRegex(eb, r"!\s*" + re.escape(b))

    def test_surface_profile_has_no_brave(self):
        eb = open(sorted(glob.glob(os.path.join(REPO, "gentoo", "app-misc", "horcrux-surface-desktop", "*.ebuild")))[-1]).read()
        self.assertNotIn("brave", eb)


if __name__ == "__main__":
    unittest.main()
