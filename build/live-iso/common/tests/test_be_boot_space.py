"""/boot space handling of horcrux-be: snapshot copies are hard links into a content store, snapshots and their /boot copy
live and die together, a snapshot never fails because /boot is full (pure logic: fake `zfs`, scaled-down sizes)."""
import contextlib
import io
import os
import sys
import tempfile
import time
import types
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import be as BE  # noqa: E402

MB = 1 << 20
TOTAL = 40 * MB


class FakeZfs:
    """just enough of zfs(8): snapshots of rpool/ROOT/<be>, clones (origin), destroy refused while a clone depends on it"""

    def __init__(self):
        self.snaps = []  # (be, name, creation counter)
        self.clones = {}  # be -> origin 'be@snap'
        self.n = 0
        self.calls = []

    def run(self, cmd, **kw):
        self.calls.append(cmd)
        ok = types.SimpleNamespace(returncode=0, stdout="", stderr="")
        if cmd[:2] == ["zfs", "snapshot"]:
            be, _, name = cmd[2].partition("@")
            be = be.split("/")[-1]
            self.n += 1
            self.snaps.append((be, name, self.n))
            return ok
        if cmd[:2] == ["zfs", "destroy"]:
            be, _, name = cmd[-1].partition("@")
            be = be.split("/")[-1]
            if f"{be}@{name}" in self.clones.values():
                return types.SimpleNamespace(returncode=1, stdout="", stderr="snapshot has dependent clones")
            self.snaps = [s for s in self.snaps if (s[0], s[1]) != (be, name)]
            return ok
        if cmd[:2] == ["zfs", "clone"]:
            self.clones[cmd[-1].split("/")[-1]] = cmd[-2].split("/ROOT/")[-1]
            return ok
        if cmd[:3] == ["zfs", "list", "-H"] and "snapshot" in cmd:
            ok.stdout = "".join(f"rpool/ROOT/{b}@{s}\t{c}\n" for b, s, c in sorted(self.snaps, key=lambda x: x[2]))
            return ok
        if cmd[:3] == ["zfs", "list", "-H"] and "origin" in cmd:
            ok.stdout = "-\n" + "".join(f"rpool/ROOT/{o}\n" for o in self.clones.values())
            return ok
        if cmd[:3] == ["zfs", "list", "-H"] and "-d" in cmd:
            ok.stdout = "rpool/ROOT\nrpool/ROOT/horcrux\n" + "".join(f"rpool/ROOT/{b}\n" for b in self.clones)
            return ok
        raise AssertionError(cmd)


class Boot(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.boot = self.tmp.name
        self.z = FakeZfs()
        self.c = BE.conf("/nonexistent")
        self.c["KEEP"] = "50"
        self.c["BOOT_MIN_FREE_MB"] = "0"  # the test /boot is only 40 MB
        os.makedirs(os.path.join(self.boot, "be", "horcrux"))
        self.stack = [mock.patch.object(BE.subprocess, "run", self.z.run), mock.patch.object(BE, "running_be", lambda: "horcrux"),
                      mock.patch.object(BE, "active_be", lambda pool: "horcrux"), mock.patch.object(BE, "boot_space", self.space)]
        for p in self.stack:
            p.start()

    def tearDown(self):
        for p in self.stack:
            p.stop()
        self.tmp.cleanup()

    def space(self, boot=None):
        seen, used = set(), 0
        for dp, _d, fns in os.walk(self.boot):
            for n in fns:
                st = os.stat(os.path.join(dp, n))
                if st.st_ino not in seen:
                    seen.add(st.st_ino)
                    used += st.st_size
        return TOTAL, TOTAL - used

    def pct(self):
        t, f = self.space()
        return 100 * (t - f) / t

    def kernel(self, tag, mb=4):
        """install a 'new kernel' as the running BE's kernel (what `horcrux-be sync` does)"""
        kv = f"7.2.{tag}"
        k, i = os.path.join(self.boot, f"vmlinuz-{kv}"), os.path.join(self.boot, f"initramfs-{kv}.img")
        for p, n in ((k, mb * MB), (i, mb * MB // 2)):
            with open(p, "wb") as f:
                f.write(os.urandom(1024) * (n // 1024))  # random-ish but cheap
                f.write(kv.encode() + p.encode())
        for old in os.listdir(self.boot):
            if old.startswith(("vmlinuz-", "initramfs-")) and kv not in old:
                os.unlink(os.path.join(self.boot, old))
        return BE.cmd_sync(self.c, self.boot)

    def snap(self, name):
        return BE.cmd_snapshot(self.c, name, self.boot)

    def snap_dirs(self):
        d = os.path.join(self.boot, "be", ".snap")
        return sorted(os.listdir(d)) if os.path.isdir(d) else []

    def objects(self):
        return os.listdir(os.path.join(self.boot, "be", ".objects"))


class Dedupe(Boot):
    def test_identical_kernel_costs_nothing(self):
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            base = self.space()[1]
            for i in range(10):
                self.snap(f"pre-upgrade-20261105-1000{i:02d}")
        self.assertLess(base - self.space()[1], 4096, "ten snapshots of the same kernel must not use any extra space (bar tiny kver files)")
        self.assertEqual(len(self.snap_dirs()), 10)
        self.assertEqual(len(self.objects()), 2)  # vmlinuz + initramfs, stored once
        ino = {os.stat(os.path.join(self.boot, "be", ".snap", d, "vmlinuz")).st_ino for d in self.snap_dirs()}
        self.assertEqual(len(ino), 1)

    def test_sync_replaces_files_instead_of_writing_through_links(self):
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            self.snap("pre-upgrade-20261105-100000")
            before = open(os.path.join(self.boot, "be", ".snap", "horcrux@pre-upgrade-20261105-100000", "vmlinuz"), "rb").read()
            self.kernel("2")
        after = open(os.path.join(self.boot, "be", ".snap", "horcrux@pre-upgrade-20261105-100000", "vmlinuz"), "rb").read()
        self.assertEqual(before, after, "the snapshot's kernel must keep the OLD kernel after the BE got a new one")
        self.assertNotEqual(before, open(os.path.join(self.boot, "be", "horcrux", "vmlinuz"), "rb").read())

    def test_legacy_plain_copies_are_deduped(self):
        # what older horcrux-be versions left: every snapshot a full private copy
        for d in ("horcrux", ".snap/horcrux@pre-upgrade-20261105-100000", ".snap/horcrux@create-20261105-100001"):
            p = os.path.join(self.boot, "be", d)
            os.makedirs(p, exist_ok=True)
            open(os.path.join(p, "vmlinuz"), "wb").write(b"K" * (3 * MB))
            open(os.path.join(p, "initramfs.img"), "wb").write(b"I" * MB)
            open(os.path.join(p, "kver"), "w").write("7.2.1\n")
        before = self.space()[1]
        saved = BE.dedupe(self.boot)
        self.assertEqual(saved, 8 * MB)
        self.assertEqual(self.space()[1] - before, 8 * MB)
        self.assertEqual(open(os.path.join(self.boot, "be", "horcrux", "kver")).read(), "7.2.1\n")
        self.assertEqual(BE.dedupe(self.boot), 0, "idempotent")


class Lifecycle(Boot):
    def test_destroying_a_snapshot_removes_its_boot_copy_and_unshared_objects(self):
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            self.snap("pre-upgrade-20261105-100000")
            self.kernel("2")  # BE moved on: the snapshot is now the only user of kernel 1
            self.assertEqual(len(self.objects()), 4)
            self.assertTrue(BE.destroy_snapshot(self.c, "horcrux", "pre-upgrade-20261105-100000", self.boot))
        self.assertEqual(self.snap_dirs(), [])
        self.assertEqual(len(self.objects()), 2)

    def test_orphans_from_a_plain_zfs_destroy_are_cleaned(self):
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            self.snap("pre-upgrade-20261105-100000")
            self.snap("pre-upgrade-20261105-100100")
            self.z.snaps = [s for s in self.z.snaps if s[1].endswith("100100")]  # `zfs destroy` done behind our back
            BE.prune(self.c, self.boot)
        self.assertEqual(self.snap_dirs(), ["horcrux@pre-upgrade-20261105-100100"])

    def test_zfs_failure_does_not_wipe_the_copies(self):
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            self.snap("pre-upgrade-20261105-100000")
        bad = types.SimpleNamespace(returncode=1, stdout="", stderr="zfs: not loaded")
        with mock.patch.object(BE.subprocess, "run", lambda *a, **k: bad):
            BE.prune(self.c, self.boot)
        self.assertEqual(len(self.snap_dirs()), 1)

    def test_keep_applies_to_pre_upgrade_and_create_but_not_user_names(self):
        self.c["KEEP"] = "2"
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            for i in range(4):
                self.snap(f"pre-upgrade-20261105-10000{i}")
                self.snap(f"create-20261105-11000{i}")
            self.snap("before-my-experiment")
        names = {d.split("@")[1] for d in self.snap_dirs()}
        self.assertEqual(names, {"pre-upgrade-20261105-100002", "pre-upgrade-20261105-100003", "create-20261105-110002",
                                 "create-20261105-110003", "before-my-experiment"})

    def test_snapshot_a_clone_depends_on_is_never_pruned(self):
        self.c["KEEP"] = "1"
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            BE.cmd_create(self.c, "newbe", None, self.boot)
            self.assertTrue(os.path.isfile(os.path.join(self.boot, "be", "newbe", "vmlinuz")))
            self.snap("pre-upgrade-20261105-100000")
            self.snap("pre-upgrade-20261105-100001")
        names = {d.split("@")[1] for d in self.snap_dirs()}
        self.assertTrue(any(n.startswith("create-") for n in names), names)  # the clone's origin survived KEEP=1 pruning
        self.assertIn("pre-upgrade-20261105-100001", names)


class FullBoot(Boot):
    def test_thirty_snapshots_and_kernel_updates_stay_under_80_percent(self):
        worst = 0
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("0")
            for i in range(30):
                if i % 3 == 0:
                    self.kernel(str(i))  # a different kernel every third snapshot (6 MB each in this scaled test)
                self.snap(f"pre-upgrade-20261105-{100000 + i}")
                worst = max(worst, self.pct())
        self.assertLessEqual(worst, 80.0)
        self.assertGreaterEqual(len(self.snap_dirs()), 2, "pruning should keep the newest snapshots, not everything or nothing")
        self.assertIn("horcrux@pre-upgrade-20261105-100029", self.snap_dirs())
        self.assertTrue(os.path.isfile(os.path.join(self.boot, "be", "horcrux", "vmlinuz")), "the running BE's kernel is never pruned")

    def test_user_named_and_origin_snapshots_survive_pressure(self):
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("0")
            self.snap("keep-me")
            for i in range(12):
                self.kernel(str(i + 1))
                self.snap(f"pre-upgrade-20261105-{100000 + i}")
        self.assertIn("horcrux@keep-me", self.snap_dirs())
        self.assertIn("keep-me", {s[1] for s in self.z.snaps})

    def test_no_room_at_all_skips_the_copy_with_a_message_and_does_not_fail(self):
        with mock.patch.object(BE, "grubcfg"):
            self.kernel("1")
            filler = os.path.join(self.boot, "filler")
            open(filler, "wb").write(b"x" * (TOTAL * 85 // 100 - (TOTAL - self.space()[1])))  # push /boot to 85%
            self.assertGreater(self.pct(), 80)
            buf = io.StringIO()
            with contextlib.redirect_stderr(buf):
                name = self.snap("keep-me")  # must not raise
                with open(os.path.join(self.boot, "vmlinuz-9.9"), "wb") as f:
                    f.write(b"n" * 5 * MB)
                with open(os.path.join(self.boot, "initramfs-9.9.img"), "wb") as f:
                    f.write(b"i" * 2 * MB)
                rc = BE.cmd_sync(self.c, self.boot)  # must not raise either
            self.assertEqual(name, "horcrux@keep-me")
            self.assertIn("no room", buf.getvalue())
            self.assertIn(("horcrux", "keep-me"), [(s[0], s[1]) for s in self.z.snaps])
            self.assertEqual(rc, None)

    def test_auto_snapshot_never_raises_on_oserror(self):
        with mock.patch.object(BE, "grubcfg"), mock.patch.object(BE.shutil, "which", lambda n: "/sbin/zfs"), \
                mock.patch.object(BE, "copy_tree", side_effect=OSError(28, "No space left on device")):
            self.kernel("1")
            self.c["AUTO_SNAPSHOT"] = "yes"
            with mock.patch("os.path.getmtime", side_effect=OSError):
                BE.cmd_auto(self.c, self.boot)
        self.assertEqual(len(self.z.snaps), 1)  # the ZFS snapshot (the real safety net) stands
        self.assertEqual(self.snap_dirs(), [])  # no half-written copy left behind


if __name__ == "__main__":
    unittest.main()
