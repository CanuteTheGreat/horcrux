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
def newest_ebuild():
    """the newest revision of the package (the test used to pin -r5 and broke when -r6 landed)"""
    import glob
    import re
    d = os.path.join(REPO, "gentoo", "app-misc", "horcrux-test-machine")
    return sorted(glob.glob(os.path.join(d, "horcrux-test-machine-1-r*.ebuild")), key=lambda p: int(re.search(r"-r(\d+)\.ebuild$", p).group(1)))[-1]


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

    def test_leftover_portage_config_of_removed_apps_fails(self):
        for rel, text, what in (("etc/portage/package.accept_keywords/desktop", "dev-util/rustup ~amd64\nwww-client/brave-bin **\n", "www-client/brave-bin"),
                                ("etc/portage/package.use/desktop", "net-misc/nextcloud-client -dolphin\n", "net-misc/nextcloud-client"),
                                ("etc/portage/package.license/x", "net-im/discord all-rights-reserved\n", "net-im/discord"),
                                ("etc/portage/package.accept_keywords", "www-client/brave-bin::canutethegreat-overlay ~amd64\n", "www-client/brave-bin"),
                                ("etc/portage/package.accept_keywords/proton", "net-vpn/proton-vpn-gtk-app ~amd64\n", "net-vpn/proton-vpn-gtk-app"),
                                ("etc/portage/package.accept_keywords/y", "app-foo/bar::canutethegreat-overlay ~amd64\n", "::canutethegreat-overlay")):
            d = self.clean_root()
            mk(d, rel, text)
            rc, out = run(d)
            self.assertEqual(rc, 1, rel)
            self.assertIn("VIOLATION portage-config", out)
            self.assertIn(what, out)

    def test_personal_repository_configured_fails(self):
        for rel in ("etc/portage/repos.conf/gringotts.conf", "etc/portage/repos.conf/eselect-repo.conf"):
            d = self.clean_root()
            mk(d, rel, "[guru]\nlocation = /var/db/repos/guru\n\n[canutethegreat-overlay]\nlocation = /var/db/repos/canutethegreat-overlay\n")
            rc, out = run(d)
            self.assertEqual(rc, 1, rel)
            self.assertIn("VIOLATION portage-repo", out)

    def test_normal_portage_config_is_fine(self):
        d = self.clean_root()
        mk(d, "etc/portage/package.accept_keywords/horcrux-lab", "# comment www-client/brave-bin\nsys-kernel/horcrux-unlock ~amd64\n*/*::guru ~amd64\n")
        mk(d, "etc/portage/package.use/desktop", "net-misc/networkmanager iwd wifi\nmedia-video/pipewire sound-server\n")
        mk(d, "etc/portage/package.license/horcrux-lab", "*/* *\n")
        mk(d, "etc/portage/repos.conf/horcrux.conf", "[horcrux]\nlocation = /var/db/repos/horcrux\n[guru]\nlocation = /var/db/repos/guru\n")
        rc, out = run(d)
        self.assertEqual(rc, 0, out)

    def test_gringotts_private_packages_and_leftovers_fail(self):
        d = self.clean_root()
        mk(d, "var/lib/portage/world", "app-misc/horcrux-test-machine\napp-misc/gringotts-config\napp-misc/gringotts-tools\n")
        mk(d, "var/db/pkg/app-misc/gringotts-tools-1-r1/PF", "x")
        rc, out = run(d)
        self.assertEqual(rc, 1)
        self.assertIn("VIOLATION package app-misc/gringotts-tools", out)
        self.assertIn("VIOLATION world-denied app-misc/gringotts-config", out)
        for rel in ("usr/bin/gringotts-restore", "usr/libexec/gringotts-firstboot", "etc/init.d/gringotts-firstboot", "var/lib/gringotts/firstboot-done",
                    "etc/ssh/sshd_config.d/10-gringotts.conf", "var/db/repos/canutethegreat-overlay/profiles/repo_name"):
            d = self.clean_root()
            mk(d, rel, "x")
            self.assertEqual(run(d)[0], 1, rel)

    def test_ebuild_blocks_gringotts_and_policy_no_longer_allows_them(self):
        eb = open(newest_ebuild()).read()
        self.assertIn("!app-misc/gringotts-config", eb)
        self.assertIn("!app-misc/gringotts-tools", eb)
        allow = [ln.strip() for ln in open(os.path.join(FILES, "world.allowlist")) if ln.strip() and not ln.startswith("#")]
        self.assertFalse([a for a in allow if "gringotts" in a], allow)

    def test_postinst_moves_cfg_protected_package_use_into_place(self):
        # r3 left package.use/horcrux-test-machine as ._cfg0000_* with the real file deleted on update (hogshead/gringotts/madammalkins, 2026-11-05)
        import re, subprocess, tempfile
        eb = open(newest_ebuild()).read()
        body = re.search(r"pkg_postinst\(\) \{\n(.*?)\n\}\n", eb, re.S).group(1)
        loop = body[body.index("local c d="):body.index("done") + 4]
        with tempfile.TemporaryDirectory() as root:
            d = os.path.join(root, "etc/portage/package.use"); os.makedirs(d)
            open(os.path.join(d, "._cfg0000_horcrux-test-machine"), "w").write("sys-kernel/horcrux-unlock -encrypted\n")
            r = subprocess.run(["bash", "-c", "EROOT=%s\nf() {\n%s\n}\nf" % (root, loop)], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(os.listdir(d), ["horcrux-test-machine"])
            self.assertIn("-encrypted", open(os.path.join(d, "horcrux-test-machine")).read())

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
