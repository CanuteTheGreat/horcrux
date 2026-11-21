"""Machine profiles + hardware layers + Apple firmware capture: one stock installer ISO for every machine (pure logic, no disks, no network)."""
import importlib.util
import io
import os
import sys
import tarfile
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
LI = os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer")
sys.path.insert(0, LI)
from liveinst import applefw as AF  # noqa: E402
from liveinst import hwlayers as HL  # noqa: E402
from liveinst import machineprofile as MP  # noqa: E402
from liveinst import t2 as T2  # noqa: E402

_spec = importlib.util.spec_from_file_location("horcrux_profile", os.path.join(LI, "horcrux_profile.py"))
E = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(E)

REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
PROJECT_LAYERS = os.path.join(HERE, "..", "..", "project", "overlay", "usr", "share", "horcrux", "hwlayers")
PKG = os.path.join(REPO, "gentoo", "sys-apps", "horcrux-profile", "files")


def tree(root, files):
    for rel, data in files.items():
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data if isinstance(data, bytes) else data.encode())


GOOD = {
    "schema": 1,
    "machine": {"hostname": "box1", "edition": "gnome", "timezone": "America/Los_Angeles", "locale": "en_US.UTF-8"},
    "disk": {"disks": ["first"], "filesystem": "zfs", "encryption": True, "swap": "auto"},
    "users": [{"name": "alice", "fullname": "Alice", "admin": True, "groups": ["docker", "portage"], "ssh_keys": ["ssh-ed25519 AAAAC3Nza test"]},
              {"name": "bob", "admin": False, "groups": ["audio"]}],
    "software": {"packages": ["www-client/firefox-bin", "app-misc/tmux"], "sets": ["@horcrux"]},
    "portage": {"use": {"app-misc/tmux": "-vim-syntax"}, "accept_keywords": {"app-misc/tmux": "~amd64"}},
    "repos": [{"name": "private", "snapshot": "bundle:repos/private", "priority": 60, "source_ok": True}],
    "binrepos": [{"name": "extra", "uri": "https://example.invalid/bin", "priority": 5}],
    "wifi": [{"ssid": "Home Net", "psk_file": "bundle:secrets/wifi"}],
    "files": [{"path": "/etc/motd.d/box", "content": "hello\n"}],
    "services": {"enable": ["sshd"]},
}

T2_LAYER = {"id": "apple-t2", "packages": ["sys-apps/tiny-dfr"], "match": {"pci": ["106b:1801"]},
            "firmware_package": {"atom": "sys-firmware/apple-t2-firmware-local", "distfile": "fw.tar"},
            "kernel": {"packages": ["sys-kernel/gentoo-kernel"], "binrepo": {"name": "t2", "uri": "https://example.invalid/t2", "priority": 9000}},
            "portage": {"accept_keywords": {"sys-apps/tiny-dfr": "~amd64"}}}


class Schema(unittest.TestCase):
    def test_good_profile_valid_with_bundle(self):
        with tempfile.TemporaryDirectory() as b:
            tree(b, {"repos/private/profiles/repo_name": "private\n", "secrets/wifi": "correct horse battery\n"})
            self.assertEqual(E.problems(GOOD, b), [])

    def test_missing_bundle_file_reported(self):
        with tempfile.TemporaryDirectory() as b:
            errs = E.problems(GOOD, b)
            self.assertTrue(any("bundle:secrets/wifi" in e for e in errs), errs)

    def test_plaintext_password_refused_and_not_echoed(self):
        bad = dict(GOOD, users=[{"name": "alice", "password": "hunter2hunter2"}])
        errs = E.problems(bad)
        self.assertTrue(any("plain-text" in e for e in errs))
        self.assertFalse(any("hunter2" in e for e in errs))

    def test_bad_values(self):
        bad = {"schema": 2, "machine": {"hostname": "-x", "edition": "kde"}, "users": [{"name": "root"}],
               "software": {"packages": ["not an atom"]}, "binrepos": [{"name": "x", "uri": "http://plain"}],
               "files": [{"path": "/boot/x", "content": "", "source": "bundle:y"}], "wifi": [{"ssid": "n", "psk": "short"}]}
        errs = " | ".join(E.problems(bad))
        for frag in ("schema", "hostname", "edition", "root", "not a package atom", "https://", "refusing path", "exactly one", "psk"):
            self.assertIn(frag, errs)

    def test_portage_wildcards_allowed_only_in_portage_tables(self):
        mp = dict(GOOD, portage={"accept_keywords": {"*/*::private": "~amd64"}, "license": {"*/*": "*"}})
        self.assertEqual([e for e in E.problems(mp) if "bundle:" not in e], [])
        bad = dict(GOOD, software={"packages": ["*/*"]})
        self.assertTrue(any("not a package atom" in e for e in E.problems(bad)))

    def test_bundle_reference_cannot_escape(self):
        with self.assertRaises(ValueError):
            E.bundle_path("/b", "bundle:../../etc/shadow")
        self.assertEqual(E.bundle_path("/b", "bundle:repos/x"), "/b/repos/x")
        self.assertIsNone(E.bundle_path("/b", "/etc/x"))


class DesiredState(unittest.TestCase):
    def test_atoms_edition_layers_profile_order(self):
        with tempfile.TemporaryDirectory() as dd:
            a = E.desired_atoms(GOOD, [T2_LAYER], distdir=dd)
            self.assertEqual(a[:2], ["gnome-base/gnome", "gnome-extra/horcrux-gnome-extras"])     # full GNOME from the gnome-light ISO
            self.assertIn("sys-kernel/gentoo-kernel", a)
            self.assertNotIn("sys-firmware/apple-t2-firmware-local", a)                          # no firmware captured
            open(os.path.join(dd, "fw.tar"), "w").close()
            self.assertIn("sys-firmware/apple-t2-firmware-local", E.desired_atoms(GOOD, [T2_LAYER], distdir=dd))
            self.assertEqual(a[-3:], ["@horcrux", "www-client/firefox-bin", "app-misc/tmux"])

    def test_server_edition_adds_nothing(self):
        mp = dict(GOOD, machine={"hostname": "s", "edition": "server"}, software={})
        self.assertEqual(E.desired_atoms(mp, []), [])

    def test_portage_files(self):
        f = E.portage_files(GOOD, [T2_LAYER])
        self.assertIn("app-misc/tmux -vim-syntax", f["/etc/portage/package.use/horcrux-profile"])
        kw = f["/etc/portage/package.accept_keywords/horcrux-profile"]
        self.assertIn("sys-apps/tiny-dfr ~amd64", kw)
        self.assertIn("app-misc/tmux ~amd64", kw)
        bins = f["/etc/portage/binrepos.conf/horcrux-profile.conf"]
        self.assertIn("[t2]", bins)
        self.assertIn("priority = 9000", bins)
        repos = f["/etc/portage/repos.conf/horcrux-profile.conf"]
        self.assertIn("[private]", repos)
        self.assertIn("auto-sync = no", repos)

    def test_admins_get_wheel_and_portage(self):
        g = E.user_groups(GOOD)
        self.assertEqual(g["alice"][:2], ["wheel", "portage"])
        self.assertEqual(g["alice"].count("portage"), 1)
        self.assertEqual(g["bob"], ["audio"])

    def test_source_build_guard(self):
        out = ("[binary   R    ] www-client/firefox-bin-160.0.1::gentoo  USE=\"\"\n"
               "[ebuild  N     ] app-misc/tmux-3.6::gentoo  USE=\"\"\n"
               "[ebuild  N     ] sys-apps/tiny-dfr-0.3.8-r1::horcrux  USE=\"\"\n"
               "[ebuild  N     ] app-misc/owner-tools-1::private  \n")
        b = E.source_builds(out)
        self.assertEqual(len(b), 3)
        self.assertEqual(E.forbidden_source_builds(b, GOOD), ["app-misc/tmux-3.6::gentoo"])
        self.assertEqual(E.forbidden_source_builds(b, dict(GOOD, software={"allow_source": ["app-misc/tmux"]})), [])

    def test_plan_lists_changes_and_removals(self):
        state = {"world": {"app-misc/tmux"}, "files": {}, "groups": {"alice": {"wheel"}},
                 "prev": {"packages": ["app-misc/old"], "files": ["/etc/old"]}}
        txt = "\n".join(E.plan(GOOD, [], state))
        self.assertIn("+ package www-client/firefox-bin", txt)
        self.assertNotIn("+ package app-misc/tmux", txt)
        self.assertIn("- package app-misc/old", txt)
        self.assertIn("- file /etc/old", txt)
        self.assertIn("user alice: add groups portage,docker", txt)

    def test_wifi_files(self):
        iwd, nm = E.wifi_files({"ssid": "Home Net"}, "secretpass")
        self.assertIn("Passphrase=secretpass", iwd)
        self.assertIn("key-mgmt=wpa-psk", nm)
        self.assertEqual(E.iwd_name("Home Net", False), "Home Net.psk")
        self.assertEqual(E.iwd_name("café", True), "=" + "café".encode().hex() + ".open")

    def test_dry_apply_runs_binary_only_emerge(self):
        cmds = []

        class Fake(E.Sys):
            def run(self, cmd, check=True, capture=False, env=None):
                cmds.append(cmd)
                return "[binary   N    ] app-misc/tmux-3.6::gentoo\n" if capture else 1

        with tempfile.TemporaryDirectory() as b:
            tree(b, {"repos/private/profiles/repo_name": "private\n", "secrets/wifi": "correct horse battery\n"})
            rec = E.apply(GOOD, [], Fake(b, log=lambda *_: None, dry=True), install=True, sync=False)
        em = [c for c in cmds if c[:1] == ["emerge"] and "--pretend" not in c]
        self.assertTrue(em and "--getbinpkg" in em[0] and "--select" in em[0])
        self.assertIn("@horcrux", rec["packages"])


class Layers(unittest.TestCase):
    def facts(self, pci=(), vendor="", product="", cpu=""):
        return {"pci": set(pci), "sys_vendor": vendor, "product_name": product, "cpu_vendor": cpu}

    def test_shipped_layers_parse_and_match(self):
        layers = HL.load_all(PROJECT_LAYERS)
        ids = {l["id"] for l in layers}
        self.assertTrue({"apple-t2", "microsoft-surface", "intel-cpu"} <= ids)
        mac = HL.detect(layers, self.facts(pci={"106b:1801", "8086:a3b1"}, vendor="Apple Inc.", product="MacBookPro16,2", cpu="GenuineIntel"))
        self.assertEqual([l["id"] for l in mac], ["apple-t2", "intel-cpu"])
        surf = HL.detect(layers, self.facts(vendor="Microsoft Corporation", product="Surface Pro 7", cpu="GenuineIntel"))
        self.assertIn("microsoft-surface", [l["id"] for l in surf])
        amd = HL.detect(layers, self.facts(pci={"1022:1480"}, cpu="AuthenticAMD"))
        self.assertEqual(amd, [])
        # no apple blob and no owner material in any public layer
        for l in layers:
            txt = repr(l).lower()
            self.assertNotIn("bundle:", txt)

    def test_facts_from_fake_sysfs(self):
        with tempfile.TemporaryDirectory() as d:
            tree(d, {"pci/0000:00:01.0/vendor": "0x106b\n", "pci/0000:00:01.0/device": "0x1801\n",
                     "dmi/sys_vendor": "Apple Inc.\n", "dmi/product_name": "MacBookPro16,2\n", "cpuinfo": "vendor_id\t: GenuineIntel\n"})
            f = HL.facts(os.path.join(d, "pci"), os.path.join(d, "dmi"), os.path.join(d, "cpuinfo"))
        self.assertIn("106b:1801", f["pci"])
        self.assertEqual(f["cpu_vendor"], "GenuineIntel")

    def test_select_spec(self):
        layers = [T2_LAYER, {"id": "x", "match": {"cpu_vendor": ["GenuineIntel"]}}]
        self.assertEqual(HL.select(layers, "none"), [])
        self.assertEqual([l["id"] for l in HL.select(layers, ["x"])], ["x"])
        with self.assertRaises(ValueError):
            HL.select(layers, ["nope"])
        self.assertEqual([l["id"] for l in HL.select(layers, "auto", self.facts(), force=["apple-t2"])], ["apple-t2"])

    def test_binrepo_has(self):
        idx = "PACKAGES: 2\n\nCPV: sys-kernel/gentoo-kernel-7.2.15\nSIZE: 1\n\nCPV: sys-fs/zfs-kmod-2.3.9\n"
        self.assertTrue(HL.binrepo_has("https://x", "sys-kernel/gentoo-kernel", fetch=lambda u: idx))
        self.assertFalse(HL.binrepo_has("https://x", "sys-kernel/gentoo-kernel-bin", fetch=lambda u: idx))

    def test_package_copies_identical(self):
        """the installer's engine and layers are byte-identical to what sys-apps/horcrux-profile installs (Horcrux tree only)"""
        if not os.path.isdir(PKG):
            self.skipTest("no gentoo/sys-apps/horcrux-profile in this tree")
        self.assertEqual(open(os.path.join(LI, "horcrux_profile.py"), "rb").read(), open(os.path.join(PKG, "horcrux_profile.py"), "rb").read())
        for n in os.listdir(os.path.join(PKG, "hwlayers")):
            self.assertEqual(open(os.path.join(PKG, "hwlayers", n), "rb").read(), open(os.path.join(PROJECT_LAYERS, n), "rb").read(), n)


class InstallerSide(unittest.TestCase):
    def test_bundle_tarball_unpack_is_confined(self):
        with tempfile.TemporaryDirectory() as d:
            tb = os.path.join(d, "b.tar.gz")
            with tarfile.open(tb, "w:gz") as t:
                for name, data in (("box/profile.toml", b"schema = 1\n"), ("box/secrets/w", b"x"), ("../evil", b"x")):
                    ti = tarfile.TarInfo(name)
                    ti.size = len(data)
                    t.addfile(ti, io.BytesIO(data))
            out = MP._unpack(tb, os.path.join(d, "out"))
            self.assertTrue(os.path.exists(os.path.join(out, "profile.toml")))
            self.assertTrue(os.path.exists(os.path.join(out, "secrets/w")))
            self.assertFalse(os.path.exists(os.path.join(d, "evil")))
            self.assertEqual(os.stat(os.path.join(out, "secrets/w")).st_mode & 0o777, 0o600)

    def test_find_kernel_arg_and_http_refused(self):
        with tempfile.TemporaryDirectory() as d:
            tree(d, {"p/profile.toml": "schema = 1\n"})
            orig = MP.BUNDLE
            MP.BUNDLE = os.path.join(d, "run-bundle")
            try:
                out = MP.find("horcrux", f"quiet horcrux.profile={d}/p", blkid=lambda: (1, ""), wait=1)
                self.assertTrue(out and os.path.exists(os.path.join(out, "profile.toml")))
            finally:
                MP.BUNDLE = orig
            with self.assertRaises(ValueError):
                MP.fetch("http://example.invalid/p.toml")
            self.assertIsNone(MP.find("horcrux", "quiet", blkid=lambda: (1, ""), wait=1))

    def test_to_config(self):
        c = {"ssh_keys": [], "disks": [], "fs": "xfs"}
        errs = MP.to_config(GOOD, c, "/b", iso="gnome")
        self.assertEqual(errs, [])
        self.assertEqual((c["hostname"], c["install_type"], c["fs"], c["zfs_encrypt"], c["timezone"]), ("box1", "desktop", "zfs", True, "America/Los_Angeles"))
        self.assertEqual(c["user_name"], "alice")
        self.assertNotIn("user_hash", c)                          # no hash in the profile: the installer asks (secrets never from the profile)
        self.assertIn("ssh-ed25519 AAAAC3Nza test", c["ssh_keys"])
        self.assertTrue(MP.to_config(GOOD, {}, "/b", iso="server"))        # a GNOME machine needs the GNOME ISO

    def test_installer_stage_writes_bundle_layers_engine_call(self):
        """MP.stage/MP.accounts end to end with a fake installer (no chroot): bundle copied 0600, layer copies, the engine call"""
        with tempfile.TemporaryDirectory() as d:
            b = os.path.join(d, "b")
            tree(b, {"profile.toml": "schema = 1\n[machine]\nhostname = \"h\"\n", "secrets/w": "x"})
            runs = []

            class I:
                target = os.path.join(d, "t")
                t2 = True
                p = {"name": "horcrux", "hwlayer_dir": PROJECT_LAYERS}
                c = {"machine_profile": MP.load(b), "machine_profile_dir": b, "hostname": "h", "t2_kernel": "skip"}
                apple_fw_tar = None

                def log(self, line):
                    pass

                def run(self, cmd, check=True, chroot=False, **k):
                    runs.append((cmd, chroot))
                    return 0
            os.makedirs(os.path.join(I.target, "etc"))
            MP.stage(I())
            MP.accounts(I())
            st = os.path.join(I.target, "var/lib/horcrux/profile")
            self.assertEqual(os.stat(os.path.join(st, "bundle/secrets/w")).st_mode & 0o777, 0o600)
            self.assertTrue(os.path.exists(os.path.join(st, "layers/apple-t2.toml")))
            self.assertEqual(runs[0][0][-3:], ["apple-t2", "apply", "--install"])
            self.assertTrue(runs[0][1] and runs[1][1])
            self.assertEqual(runs[1][0][-2:], ["apply", "--accounts-only"])
            self.assertFalse(os.path.exists(os.path.join(I.target, "var/tmp/horcrux-profile")))     # engine copy removed again

    def test_edition_families(self):
        self.assertIsNone(MP.edition_problem("gnome", "gnome"))
        self.assertIsNone(MP.edition_problem("gnome-light", "gnome"))
        self.assertIsNotNone(MP.edition_problem("server", "gnome"))
        self.assertIsNotNone(MP.edition_problem("xfce", "server"))

    def test_kernel_preflight(self):
        class I:
            pass
        bad = MP.kernel_preflight(I(), [T2_LAYER], has=lambda uri, a: False)
        self.assertTrue(bad and "not published" in bad[0])
        self.assertEqual(MP.kernel_preflight(I(), [T2_LAYER], has=lambda uri, a: True), [])


def macos_firmware_tree(root):
    """a synthetic macOS /usr/share/firmware tree in Apple's layout (made-up bytes, no Apple data)"""
    w = "wifi/C-4364__s-B2/"
    tree(root, {
        w + "ekans.trx": b"\x01firmware-ekans",
        w + "hanauma.trx": b"\x01firmware-ekans",           # identical blob: becomes a hard link
        w + "ekans.clmb": b"clm-ekans",
        w + "P-ekans_M-HRPN_V-m__m-5.1.txt": "boardrev=0x1\nmacaddr=00:90:4c:0d:f4:3e \n",
        w + "P-ekans_M-HRPN_V-m__m-6.1.txt": "boardrev=0x3\n",
        w + "P-ekans_M-HRPN_V-u__m-7.5.txt": "boardrev=0x2\n",
        "wifi/C-4355__s-C1/notes.md": "ignored",
        "wifi/C-9999__s-A0/kahana.trx": b"other chip, not imported",
    })


class AppleFirmware(unittest.TestCase):
    def test_convert_tree_names(self):
        with tempfile.TemporaryDirectory() as d:
            macos_firmware_tree(d)
            names = sorted(n for n, _ in AF.convert_tree(d))
        self.assertIn("brcmfmac4364b2-pcie.apple,ekans.bin", names)
        self.assertIn("brcmfmac4364b2-pcie.apple,hanauma.bin", names)
        self.assertIn("brcmfmac4364b2-pcie.apple,ekans.clm_blob", names)
        self.assertIn("brcmfmac4364b2-pcie.apple,ekans-HRPN-m-5.1.txt", names)
        self.assertIn("brcmfmac4364b2-pcie.apple,ekans-HRPN-m-6.1.txt", names)
        self.assertIn("brcmfmac4364b2-pcie.apple,ekans-HRPN-u.txt", names)        # one module version only: collapsed like t2linux does
        self.assertFalse(any("9999" in n for n in names))

    def test_capture_from_raw_tarball_on_esp_then_import_keeps_links(self):
        with tempfile.TemporaryDirectory() as d:
            src = os.path.join(d, "fw")
            macos_firmware_tree(src)
            esp = os.path.join(d, "esp")
            os.makedirs(esp)
            with tarfile.open(os.path.join(esp, "firmware-raw.tar.gz"), "w:gz") as t:     # what t2linux's method 1 leaves on the EFI partition
                t.add(src, arcname=".")

            class I:
                c = {"t2_firmware": "auto"}
                applefw_out = os.path.join(d, "out.tar")
                logs = []

                def log(self, l):
                    self.logs.append(l)

            inst = I()
            out = T2.capture_firmware(inst, sources=T2.mac_firmware_sources(esp_mounts=[esp]))
            self.assertEqual(out, inst.apple_fw_tar)
            with tarfile.open(out) as t:
                links = [m.name for m in t.getmembers() if m.islnk()]
            self.assertEqual(links, ["brcmfmac4364b2-pcie.apple,hanauma.bin"])
            # the package-side import (and horcrux-t2-firmware) must keep hard-linked names
            got = T2.import_firmware(out, os.path.join(d, "target"), "tar")
            self.assertIn("brcmfmac4364b2-pcie.apple,hanauma.bin", got)
            self.assertEqual(open(os.path.join(d, "target/lib/firmware/brcm/brcmfmac4364b2-pcie.apple,hanauma.bin"), "rb").read(), b"\x01firmware-ekans")

    def test_capture_none(self):
        class I:
            c = {"t2_firmware": "none"}
        self.assertIsNone(T2.capture_firmware(I(), sources=[]))


if __name__ == "__main__":
    unittest.main()
