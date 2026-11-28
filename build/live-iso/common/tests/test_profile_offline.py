"""Offline machine-profile install (G4): an [offline] payload (binpkgs + ::gentoo snapshot) replaces every network step at install
time, `stage-offline` writes it, the installer drops it from the stored bundle, and [[files]] sources are copied byte for byte."""
import importlib.util
import io
import json
import os
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
LI = os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer")
sys.path.insert(0, LI)
from liveinst import machineprofile as MP  # noqa: E402

_spec = importlib.util.spec_from_file_location("horcrux_profile", os.path.join(LI, "horcrux_profile.py"))
E = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(E)

PROFILE = {"schema": 1, "machine": {"hostname": "box1", "edition": "server"},
           "software": {"packages": ["app-editors/vim"]}, "offline": {"dir": "bundle:offline"}}


def tree(root, files):
    for rel, data in files.items():
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data if isinstance(data, bytes) else data.encode())


class Recorder(E.Sys):
    def __init__(self, *a, **k):
        super().__init__(*a, **k)
        self.cmds = []

    def run(self, cmd, check=True, capture=False, env=None):
        self.cmds.append((cmd, dict(env or {})))
        return "[binary   N    ] app-editors/vim-9.1::gentoo\n" if capture else 1


class Schema(unittest.TestCase):
    def test_offline_table_validated_and_not_a_required_bundle_ref(self):
        self.assertEqual(E.problems(PROFILE), [])
        with tempfile.TemporaryDirectory() as b:
            # the stored bundle of an installed machine no longer has the payload: later validate/apply must not fail on it
            self.assertEqual(E.problems(PROFILE, b), [])
        self.assertTrue(E.problems(dict(PROFILE, offline={"dir": "/abs"})))
        self.assertTrue(E.problems(dict(PROFILE, offline={"dir": "bundle:offline", "binpkgs": "x"})))

    def test_payload_requires_packages_index(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(RuntimeError):
                E.offline_payload(d)
            tree(d, {"binpkgs/Packages": "", "gentoo.tar.xz": b""})
            p = E.offline_payload(d)
            self.assertTrue(p["binpkgs"].endswith("binpkgs") and p["gentoo"].endswith("gentoo.tar.xz"))


class OfflineApply(unittest.TestCase):
    def _bundle(self, b):
        tree(b, {"offline/binpkgs/Packages": "", "offline/manifest.json": json.dumps({"image_build_id": "x"})})

    def test_install_uses_only_the_payload(self):
        with tempfile.TemporaryDirectory() as b:
            self._bundle(b)
            sy = Recorder(b, log=lambda *_: None, dry=True)
            rec = E.apply(PROFILE, [], sy, install=True, sync=True)
        cmds = [c for c, _ in sy.cmds]
        self.assertFalse([c for c in cmds if c[:1] in (["emerge-webrsync"], ["emaint"])], "no network sync offline")
        em = [(c, env) for c, env in sy.cmds if c[:1] == ["emerge"]]
        self.assertTrue(em)
        for c, env in em:
            self.assertNotIn("--getbinpkg", c)
            self.assertIn("--usepkgonly", c)
            self.assertEqual(env.get("PKGDIR"), os.path.join(b, "offline", "binpkgs"))
        self.assertTrue(rec.get("offline"))

    def test_later_apply_is_online_again(self):
        with tempfile.TemporaryDirectory() as b:
            sy = Recorder(b, log=lambda *_: None, dry=True)
            E.apply(PROFILE, [], sy, install=False, sync=True)
        cmds = [c for c, _ in sy.cmds]
        self.assertTrue([c for c in cmds if c[:1] == ["emerge-webrsync"]])
        em = [c for c in cmds if c[:1] == ["emerge"] and "--deselect" not in c]
        self.assertTrue(all("--getbinpkg" in c for c in em))

    def test_missing_payload_at_install_is_a_clear_error(self):
        with tempfile.TemporaryDirectory() as b:
            with self.assertRaisesRegex(RuntimeError, "offline payload"):
                E.apply(PROFILE, [], Recorder(b, log=lambda *_: None, dry=True), install=True)

    def test_kernel_layer_reinstall_is_offline_too(self):
        lay = {"id": "k", "kernel": {"packages": ["sys-kernel/gentoo-kernel"], "reinstall": ["sys-fs/zfs-kmod"]}}
        with tempfile.TemporaryDirectory() as b:
            self._bundle(b)
            sy = Recorder(b, log=lambda *_: None, dry=True)
            E.apply(PROFILE, [lay], sy, install=True)
        re = [(c, env) for c, env in sy.cmds if c[:2] == ["emerge", "--oneshot"]]
        self.assertTrue(re)
        self.assertNotIn("--getbinpkg", re[0][0])
        self.assertIn("PKGDIR", re[0][1])

    def test_stage_offline_fetches_into_the_payload(self):
        with tempfile.TemporaryDirectory() as b, tempfile.TemporaryDirectory() as out:
            mp = {k: v for k, v in PROFILE.items() if k != "offline"}
            sy = Recorder(b, log=lambda *_: None, dry=True)
            E.apply(mp, [], sy, install=True, stage_dir=out)
            em = [(c, env) for c, env in sy.cmds if c[:1] == ["emerge"] and "--fetchonly" not in c]
            self.assertTrue(all("--getbinpkg" in c and env.get("PKGDIR") == os.path.join(out, "binpkgs") for c, env in em))
            self.assertTrue(all("buildpkg" in env.get("FEATURES", "") for c, env in em))
            self.assertTrue([c for c, _ in sy.cmds if c[:1] == ["emerge-webrsync"]])

    def test_collect_binrepo_cache_instance(self):
        with tempfile.TemporaryDirectory() as d:
            cache, pk, vdb = os.path.join(d, "cache"), os.path.join(d, "pk"), os.path.join(d, "vdb")
            tree(cache, {"app-editors/vim/vim-9.2-1.gpkg.tar": "one", "app-editors/vim/vim-9.2-3.gpkg.tar": "three",
                         "dev-libs/x/x-1-1.gpkg.tar": "x"})
            tree(vdb, {"app-editors/vim-9.2/BUILD_ID": "3\n"})
            with mock.patch.object(E, "binrepo_caches", return_value=[cache]):
                got = E.collect_from_caches(E.Sys(None, log=lambda *_: None), ["app-editors/vim-9.2", "dev-libs/x-1"], pk, vdb=vdb)
            self.assertEqual(got, {"app-editors/vim-9.2", "dev-libs/x-1"})
            self.assertEqual(open(os.path.join(pk, "app-editors/vim/vim-9.2-3.gpkg.tar")).read(), "three")   # the installed build
            self.assertFalse(os.path.exists(os.path.join(pk, "app-editors/vim/vim-9.2-1.gpkg.tar")))
            self.assertEqual(E.installed_in_pkgdir(pk), {"app-editors/vim-9.2", "dev-libs/x-1"})

    def test_pkgdir_inventory(self):
        with tempfile.TemporaryDirectory() as d:
            tree(d, {"app-editors/vim/vim-9.2.1275-1.gpkg.tar": "", "sys-apps/horcrux-profile/horcrux-profile-1-r1-2.gpkg.tar": "",
                     "dev-libs/old-1.tbz2": "", "Packages": ""})
            self.assertEqual(E.installed_in_pkgdir(d), {"app-editors/vim-9.2.1275", "sys-apps/horcrux-profile-1-r1", "dev-libs/old-1"})

    def test_offline_turns_image_getbinpkg_default_off(self):
        with tempfile.TemporaryDirectory() as b:
            self._bundle(b)
            sy = Recorder(b, log=lambda *_: None, dry=True)
            E.apply(PROFILE, [], sy, install=True)
        em = [(c, env) for c, env in sy.cmds if c[:1] == ["emerge"] and "--deselect" not in c]
        self.assertTrue(all("--getbinpkg=n" in c and env.get("FEATURES") == "-getbinpkg" for c, env in em))


class GentooSnapshot(unittest.TestCase):
    def _tar(self, path, prefix):
        with tarfile.open(path, "w:xz") as t:
            for name, data in ((f"{prefix}sys-apps/x/x-1.ebuild", b"EAPI=8\n"), (f"{prefix}profiles/repo_name", b"gentoo\n")):
                ti = tarfile.TarInfo(name)
                ti.size = len(data)
                t.addfile(ti, io.BytesIO(data))

    def test_unpack_both_layouts_and_never_over_a_real_tree(self):
        for prefix in ("", "gentoo/", "./"):
            with tempfile.TemporaryDirectory() as d:
                src, dst = os.path.join(d, "gentoo.tar.xz"), os.path.join(d, "repo")
                self._tar(src, prefix)
                sy = E.Sys(None, log=lambda *_: None)
                E.unpack_gentoo(sy, src, dst)
                self.assertTrue(os.path.isfile(os.path.join(dst, "sys-apps/x/x-1.ebuild")), prefix)
                self.assertEqual(os.stat(os.path.join(dst, "profiles/repo_name")).st_mode & 0o777, 0o644)
                calls = []
                sy.run = lambda *a, **k: calls.append(a)
                E.unpack_gentoo(sy, src, dst)           # already a tree: left alone
                self.assertEqual(calls, [])


class BinaryFiles(unittest.TestCase):
    def test_bundle_file_source_copied_byte_for_byte(self):
        key = bytes(range(256)) * 2          # raw ZFS keys are 32 arbitrary bytes; not UTF-8
        with tempfile.TemporaryDirectory() as b, tempfile.TemporaryDirectory() as root:
            tree(b, {"keys/k": key})
            dst = os.path.join(root, "etc/zfs/keys/k")
            mp = {"schema": 1, "machine": {"hostname": "box1"}, "files": [{"path": dst, "source": "bundle:keys/k", "mode": "0400"}]}
            sy = Recorder(b, log=lambda *_: None)
            with mock.patch.object(E, "next_generation", return_value=(1, {})), \
                    mock.patch.object(E, "GEN_DIR", os.path.join(root, "st", "generations")), mock.patch.object(E, "STATE", os.path.join(root, "st")):
                E.apply(mp, [], sy, install=True, sync=False)
            self.assertEqual(open(dst, "rb").read(), key)
            self.assertEqual(os.stat(dst).st_mode & 0o777, 0o400)


class InstallerDropsPayload(unittest.TestCase):
    def test_env_d_file_runs_env_update(self):
        mp = {"schema": 1, "machine": {"hostname": "box1"}, "files": [{"path": "/etc/env.d/99editor", "content": "EDITOR=vim\n"}]}
        with tempfile.TemporaryDirectory() as b:
            sy = Recorder(b, log=lambda *_: None, dry=True)
            with mock.patch.object(E.shutil, "which", side_effect=lambda x: "/usr/bin/" + x):
                E.apply(mp, [], sy, install=True, sync=False)
        self.assertIn(["env-update"], [c for c, _ in sy.cmds])
    def test_drop(self):
        with tempfile.TemporaryDirectory() as st:
            tree(st, {"bundle/offline/binpkgs/Packages": "", "bundle/profile.toml": "x"})
            MP.drop_offline_payload(PROFILE, st)
            self.assertFalse(os.path.exists(os.path.join(st, "bundle/offline")))
            self.assertTrue(os.path.exists(os.path.join(st, "bundle/profile.toml")))
            MP.drop_offline_payload({"offline": {"dir": "bundle:."}}, st)          # never the whole bundle
            MP.drop_offline_payload({"offline": {"dir": "bundle:../x"}}, st)
            self.assertTrue(os.path.exists(os.path.join(st, "bundle/profile.toml")))


if __name__ == "__main__":
    unittest.main()
