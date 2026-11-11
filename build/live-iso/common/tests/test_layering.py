"""Layering rules (server-first Horcrux): core (headless) -> desktop edition -> machine layers.

These read the Horcrux repo tree (build/live-iso/project, gentoo/); the vendored copy of common/ in Patronus has none of it, so every test skips there.
"""
import os
import re
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
LIVE = os.path.join(REPO, "build", "live-iso")
GENTOO = os.path.join(REPO, "gentoo")
HAVE = os.path.isdir(os.path.join(GENTOO, "sys-apps", "horcrux-core"))


def atoms(path):
    out = []
    for line in open(path):
        line = line.split("#")[0].strip()
        if line:
            out.append(line.split()[0].split("::")[0])
    return out


def ebuild(cat, pn):
    d = os.path.join(GENTOO, cat, pn)
    f = sorted(x for x in os.listdir(d) if x.endswith(".ebuild"))[-1]
    return open(os.path.join(d, f)).read()


ATOM = re.compile(r"^(!!?)?([<>]=?|=|~)?[a-z0-9][a-z0-9+_.-]*/[A-Za-z0-9+_.*-]+(:[A-Za-z0-9+_./*=-]*)?(\\[[^\\]]*\\])?$|^\\|\\|$|^[()]$|^[a-z0-9_+-]+\\?$")


def rdepend(text):
    m = re.search(r'RDEPEND="(.*?)"', text, re.S)
    return [x.strip() for x in m.group(1).split() if x.strip()] if m else []


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class Layering(unittest.TestCase):
    def test_public_desktop_has_nothing_personal(self):
        forb = set(atoms(os.path.join(LIVE, "project", "public-forbidden.txt")))
        self.assertIn("app-admin/rustinion-agent", forb)
        self.assertIn("www-client/brave-bin", forb)
        gl = set(atoms(os.path.join(LIVE, "project", "variants", "gnome-light", "packages.txt")))
        self.assertEqual(sorted(gl & forb), [], "personal apps in the PUBLIC gnome-light image")
        for f in ("packages.txt", "packages-late.txt"):
            self.assertEqual(sorted(set(atoms(os.path.join(LIVE, "project", f))) & forb), [], f"personal apps in the public server image ({f})")
        self.assertEqual(sorted(set(atoms(os.path.join(LIVE, "common", "packages.txt"))) & forb), [])

    def test_owner_apps_live_in_machine_layers(self):
        owner = set(atoms(os.path.join(LIVE, "project", "variants", "owner-common", "packages.txt")))
        for a in ("net-misc/nextcloud-client", "app-antivirus/clamav"):
            self.assertIn(a, owner)
        self.assertNotIn("www-client/brave-bin", owner)     # dropped 2026-11-11 (owner uses Firefox, from the public base)
        self.assertNotIn("net-vpn/tailscale", owner)     # retired 2026-11-10 (WireGuard hub replaces it)
        self.assertIn("net-vpn/tailscale", set(atoms(os.path.join(LIVE, "project", "public-forbidden.txt"))))
        for v in ("gringotts", "hogshead"):
            txt = "".join(open(os.path.join(LIVE, "project", "variants", v, f)).read() for f in os.listdir(os.path.join(LIVE, "project", "variants", v)) if f.startswith("Dockerfile"))
            self.assertIn("owner-common/packages.txt", txt, f"{v} does not install the owner apps")
        # Steam is for HOGSMEADE only: no active (non-comment) line of any shared list, Dockerfile, profile or overlay package mentions it
        shared = [os.path.join(LIVE, "project", "packages.txt"), os.path.join(LIVE, "project", "packages-late.txt"), os.path.join(LIVE, "common", "packages.txt")]
        for v in ("owner-common", "gnome-light", "core"):
            vd = os.path.join(LIVE, "project", "variants", v)
            shared += [os.path.join(d, f) for d, _, fs in os.walk(vd) for f in fs if not f.endswith(".md")]
        for d in ("horcrux-core", "horcrux-time", "horcrux-firstboot", "horcrux-backup", "horcrux-gnome-defaults"):
            for c in ("sys-apps", "net-misc", "gnome-extra"):
                dd = os.path.join(GENTOO, c, d)
                if os.path.isdir(dd):
                    shared += [os.path.join(r, f) for r, _, fs in os.walk(dd) for f in fs]
        for f in shared:
            self.assertNotRegex(open(f, errors="replace").read(), r"(?im)^[^#\n]*\bsteam\b", f)

    def test_core_carries_every_shared_fix_and_desktop_depends_on_it(self):
        core = rdepend(ebuild("sys-apps", "horcrux-core"))
        for need in ("net-misc/chrony", "net-misc/horcrux-time", "net-misc/horcrux-chrony-nm", "sys-apps/horcrux-firstboot", "sys-apps/horcrux-backup", "sys-apps/horcrux-repo-config", "sys-fs/zfs", "dev-util/pahole"):
            self.assertIn(need, core)
        gd = rdepend(ebuild("gnome-extra", "horcrux-gnome-defaults"))
        self.assertIn("sys-apps/horcrux-core", gd)
        for moved in ("net-misc/chrony", "net-misc/horcrux-chrony-nm", "sys-apps/horcrux-repo-config"):
            self.assertNotIn(moved, gd, f"{moved} must come from the core, not the GNOME package")
        # the core is headless: nothing graphical in any core package's RDEPEND
        gui = re.compile(r"^(gnome-|gui-|x11-|kde-|xfce-|media-libs/mesa|net-misc/networkmanager)")
        for cat, pn in (("sys-apps", "horcrux-core"), ("net-misc", "horcrux-time"), ("net-misc", "horcrux-chrony-nm"), ("sys-apps", "horcrux-firstboot"), ("sys-apps", "horcrux-backup")):
            self.assertEqual([d for d in rdepend(ebuild(cat, pn)) if gui.match(d)], [], f"{cat}/{pn} pulls a display package")

    def test_dependency_strings_are_clean_atoms(self):
        """a `#` comment inside RDEPEND="..." is part of the string: Portage then masks the package as `corruption` (it broke the aarch64 build)"""
        for cat, pn in (("sys-apps", "horcrux-core"), ("net-misc", "horcrux-time"), ("net-misc", "horcrux-chrony-nm"), ("sys-apps", "horcrux-firstboot"),
                        ("sys-apps", "horcrux-backup"), ("gnome-extra", "horcrux-gnome-defaults")):
            for tok in rdepend(ebuild(cat, pn)):
                self.assertRegex(tok, ATOM, f"{cat}/{pn}: {tok!r} is not a dependency atom")

    def test_core_is_not_machine_specific(self):
        core = "\n".join(l for l in (ebuild("sys-apps", "horcrux-core") + ebuild("net-misc", "horcrux-time") + ebuild("sys-apps", "horcrux-firstboot")).splitlines() if not l.lstrip().startswith("#"))
        for word in ("rustinion", "gringotts", "hogshead", "hogsmeade", "madammalkins", "brave", "tailscale", "canutethegreat/", "steam"):
            self.assertNotIn(word, core.lower().replace("canutethegreat/horcrux", ""), word)
        paths = open(os.path.join(GENTOO, "sys-apps", "horcrux-backup", "files", "paths")).read()
        self.assertNotIn("canutethegreat", paths)
        self.assertFalse(os.path.exists(os.path.join(GENTOO, "sys-apps", "horcrux-backup", "files", "required")), "the owner's required list belongs to the machine layer (owner-common/overlay)")

    def test_build_applies_core_to_every_image(self):
        sh = open(os.path.join(LIVE, "build.sh")).read()
        self.assertIn('CHAIN="core $CHAIN"', sh)
        self.assertIn("surface-kernel", sh)      # the Surface image opts out until its meta package depends on the core
        self.assertTrue(os.path.isfile(os.path.join(LIVE, "project", "variants", "core", "Dockerfile")))
        df = open(os.path.join(LIVE, "project", "variants", "core", "Dockerfile")).read()
        self.assertIn("sys-apps/horcrux-core", df)
        self.assertIn("public-forbidden.txt", df)

    def test_installer_enables_core_services(self):
        t = open(os.path.join(LIVE, "project", "overlay", "etc", "live-installer", "profile.toml")).read()
        m = re.search(r"(?m)^services = \[(.*?)\]", t)
        self.assertIsNotNone(m)
        svcs = re.findall(r'"([^"]+)"', m.group(1))
        for s in ("horcrux-api", "sshd", "chronyd", "horcrux-timesync", "horcrux-firstboot", "cronie"):
            self.assertIn(s, svcs)
        dl = open(os.path.join(LIVE, "project", "variants", "gnome-light", "overlay", "usr", "share", "horcrux", "desktop", "services.list")).read()
        self.assertNotIn("tailscale", dl.replace("Tailscale", ""))

    def test_server_gate_wired_in_ci_for_both_arches(self):
        ci = open(os.path.join(LIVE, "common", "ci-run.sh")).read()
        self.assertRegex(ci, r"zfs-enc-test\.py.*--server --public --late-network")
        self.assertIn('[ "$IMG_DE" = server ]', ci)       # not gated on x86_64: aarch64 runs it too


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class TimeConfig(unittest.TestCase):
    CMD = os.path.join(GENTOO, "net-misc", "horcrux-time", "files", "horcrux-time-config")

    def run_cfg(self, root, mode):
        return subprocess.run(["sh", self.CMD, mode, root], capture_output=True, text=True)

    def test_fix_makes_a_months_off_clock_step_and_is_idempotent(self):
        r = tempfile.mkdtemp()
        try:
            os.makedirs(r + "/etc/chrony")
            os.makedirs(r + "/etc/conf.d")
            open(r + "/etc/chrony/chrony.conf", "w").write("pool 2.gentoo.pool.ntp.org iburst\nmakestep 1.0 3\n")
            open(r + "/etc/conf.d/hwclock", "w").write('clock="local"\n')
            self.assertEqual(self.run_cfg(r, "check").returncode, 1)
            self.assertEqual(self.run_cfg(r, "fix").returncode, 0)
            conf = open(r + "/etc/chrony/chrony.conf").read()
            self.assertIn("makestep 1 -1", conf)
            self.assertNotIn("makestep 1.0 3", conf)
            self.assertIn("rtcsync", conf)
            self.assertIn('clock="UTC"', open(r + "/etc/conf.d/hwclock").read())
            first = (conf, open(r + "/etc/conf.d/hwclock").read())
            self.assertEqual(self.run_cfg(r, "fix").returncode, 0)
            self.assertEqual(first, (open(r + "/etc/chrony/chrony.conf").read(), open(r + "/etc/conf.d/hwclock").read()))
            self.assertEqual(self.run_cfg(r, "check").returncode, 0)
        finally:
            shutil.rmtree(r)

    def test_fix_adds_a_source_when_none(self):
        r = tempfile.mkdtemp()
        try:
            self.assertEqual(self.run_cfg(r, "fix").returncode, 0)
            self.assertRegex(open(r + "/etc/chrony/chrony.conf").read(), r"(?m)^pool \S+ iburst$")
        finally:
            shutil.rmtree(r)


if __name__ == "__main__":
    unittest.main()
