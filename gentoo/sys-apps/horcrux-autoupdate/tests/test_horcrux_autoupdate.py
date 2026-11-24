"""Unit tests for horcrux-autoupdate (no Portage, no root, no network):  python3 -m unittest discover -s gentoo/sys-apps/horcrux-autoupdate/tests"""
import importlib.machinery
import importlib.util
import json
import os
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "files", "horcrux-autoupdate")


def load():
    loader = importlib.machinery.SourceFileLoader("hxau", SCRIPT)
    spec = importlib.util.spec_from_loader("hxau", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


WORLD_PRETEND = """\
[ebuild     U  ] sys-apps/xz-utils-5.8.2 [5.8.1]
[binary   R    ] sys-firmware/intel-microcode-20261110 [20260925]
Total: 2 packages (2 upgrades), Size of downloads: 10 KiB
"""

DEPCLEAN_PRETEND = """\
Calculating dependencies... done!
>>> These are the packages that would be unmerged:

 dev-libs/foo
    selected: 1.0
   protected: none
     omitted: none

All selected packages: dev-libs/foo-1.0 app-misc/bar-2.1-r1

>>> 'Selected' packages are slated for removal.
Packages installed:   900
"""


class Base(unittest.TestCase):
    def setUp(self):
        self.m = load()
        self._which = self.m.shutil.which
        self.t = tempfile.TemporaryDirectory()
        d = self.t.name
        m = self.m
        m.STATE_DIR, m.LOG_DIR = os.path.join(d, "state"), os.path.join(d, "log")
        m.LOCK, m.EMERGE_LOG = os.path.join(d, "lock"), os.path.join(d, "emerge.log")
        m.PRESERVED, m.RUN_REBOOT_FLAG, m.CONF = os.path.join(d, "preserved"), os.path.join(d, "reboot-flag"), os.path.join(d, "conf")
        m.root_zfs = lambda: None  # no ZFS root unless a test says so
        m.sh = self.fake_sh  # logger etc. also go through it
        self.calls = []
        self.rc = {}
        self.emerge_merges = []
        self.out = {}

    def tearDown(self):
        self.m.shutil.which = self._which
        self.t.cleanup()

    def fake_sh(self, argv, log=None, timeout=None, env=None, full=False):
        while argv and argv[0] in ("nice", "ionice"):
            argv = argv[3:] if argv[0] == "nice" else argv[3:]
        self.calls.append(argv)
        key = " ".join(argv)
        rc = 0
        for pat, r in self.rc.items():
            if pat in key:
                rc = r
        lines = []
        for pat, o in self.out.items():
            if pat in key:
                lines = o.splitlines()
        if argv[:1] == ["emerge"] and "@world" in argv and "--pretend" not in argv:
            with open(self.m.EMERGE_LOG, "a") as f:
                for i, cpv in enumerate(self.emerge_merges):
                    f.write(f"1:  ::: completed emerge ({i + 1} of {len(self.emerge_merges)}) {cpv} to /\n")
        if log:
            log.write(f"$ {key}\n")
        return rc, lines if full else lines[-25:]

    def conf(self, **kw):
        c = dict(self.m.DEFAULTS)
        c.update(kw)
        return c

    def steps(self):
        return [c[0] if c[0] != "emerge" else "emerge:" + [a for a in c if a.startswith("@") or a in ("--depclean",)][0:1].__str__() for c in self.calls]


class ConfigAndParsing(Base):
    def test_conf_parsing_quotes_comments_defaults(self):
        with open(self.m.CONF, "w") as f:
            f.write('# c\nENABLED = no\nEXTRA_EMERGE_OPTS="--jobs=1 --foo"\nbogus line\n')
        c = self.m.read_conf()
        self.assertEqual(c["ENABLED"], "no")
        self.assertEqual(c["EXTRA_EMERGE_OPTS"], "--jobs=1 --foo")
        self.assertEqual(c["INTERVAL_HOURS"], "24")
        self.assertFalse(self.m.yes(c["ENABLED"]))

    def test_world_args_prefer_binpkgs_and_guard_prebuilt(self):
        a = self.m.world_args(self.conf())
        for want in ("--getbinpkg", "--usepkg", "--update", "--deep", "--newuse", "--changed-use", "--with-bdeps=y", "--keep-going", "@world", "--ask=n",
                     "--usepkg-exclude=app-emulation/horcrux-bin"):
            self.assertIn(want, a)

    def test_world_resolution_has_changed_use_and_bdeps_switches(self):
        c = self.conf()
        self.assertEqual(c["WITH_BDEPS"], "yes")
        self.assertEqual(c["CHANGED_USE"], "yes")
        a = self.m.world_args(c)
        for want in ("--update", "--changed-use", "--newuse", "--deep", "--with-bdeps=y"):
            self.assertIn(want, a)
        self.assertNotIn("--with-bdeps=n", a)
        self.assertEqual(self.m.bdeps_opt(c), "--with-bdeps=y")
        c["WITH_BDEPS"] = "no"
        c["CHANGED_USE"] = "no"
        self.assertIn("--with-bdeps=n", self.m.world_args(c))
        self.assertNotIn("--changed-use", self.m.world_args(c))
        self.assertNotIn("--with-bdeps=y", self.m.world_args(c))

    def test_atom_of(self):
        self.assertEqual(self.m.atom_of("sys-firmware/intel-microcode-20261110_p20261111-r1"), "sys-firmware/intel-microcode")
        self.assertEqual(self.m.atom_of("app-misc/bar-2.1-r1"), "app-misc/bar")
        self.assertEqual(self.m.atom_of("dev-libs/libfoo-2-3.1"), "dev-libs/libfoo-2")

    def test_pending_and_depclean_parsers(self):
        self.assertEqual(len(self.m.parse_pending(WORLD_PRETEND)), 2)
        self.assertEqual(self.m.parse_depclean(DEPCLEAN_PRETEND), ["dev-libs/foo", "app-misc/bar"])

    def test_depclean_parser_handles_equals_prefix_and_wrapped_lines(self):
        txt = "All selected packages: =media-libs/libcdr-0.1.9 =sys-kernel/gentoo-kernel-6.18.62\n  =dev-libs/x-1-r2\n\n>>> 'Selected' packages are slated for removal.\n"
        self.assertEqual(self.m.parse_depclean(txt), ["media-libs/libcdr", "sys-kernel/gentoo-kernel", "dev-libs/x"])
        remove, kept, why = self.m.depclean_plan(self.m.parse_depclean(txt), self.conf())
        self.assertEqual((remove, kept, why), (["media-libs/libcdr", "dev-libs/x"], ["sys-kernel/gentoo-kernel"], None))

    def test_merged_since_reads_only_new_lines(self):
        with open(self.m.EMERGE_LOG, "w") as f:
            f.write("1:  ::: completed emerge (1 of 1) old/pkg-1 to /\n")
        off = os.path.getsize(self.m.EMERGE_LOG)
        with open(self.m.EMERGE_LOG, "a") as f:
            f.write("2:  >>> emerge (1 of 2) a/b-1 to /\n2:  ::: completed emerge (1 of 2) a/b-1 to /\n3:  ::: completed emerge (2 of 2) c/d-2.0-r1 to /\n")
        self.assertEqual(self.m.merged_since(off), ["a/b", "c/d"])


class Policy(Base):
    def test_depclean_keeps_protected_and_refuses_big_sets(self):
        c = self.conf()
        for a in ("sys-kernel/gentoo-kernel-bin", "sys-fs/zfs-kmod", "app-emulation/horcrux-bin", "sys-apps/horcrux-release"):
            self.assertEqual(self.m.depclean_plan([a, "dev-libs/foo"], c), (["dev-libs/foo"], [a], None))
        # an old kernel no longer blocks the cleanup of everything else (an installed older gentoo-kernel used to keep depclean off for good)
        self.assertEqual(self.m.depclean_plan(["sys-kernel/gentoo-kernel"], c), ([], ["sys-kernel/gentoo-kernel"], None))
        self.assertIn("limit", self.m.depclean_plan([f"a/p{i}" for i in range(21)], c)[2])
        self.assertIsNone(self.m.depclean_plan([f"a/p{i}" for i in range(20)] + ["sys-kernel/gentoo-kernel"], c)[2])

    def test_reboot_reasons_kernel_and_microcode(self):
        r = self.m.reboot_reasons(["sys-firmware/intel-microcode", "app-editors/vim", "sys-firmware/intel-microcode"], self.conf(),
                                  running="6.12.1-gentoo", newest="6.12.9-gentoo")
        self.assertEqual(len(r), 2)
        self.assertIn("6.12.9", r[0])
        self.assertIn("sys-firmware/intel-microcode updated", r)
        self.assertEqual(self.m.reboot_reasons(["app-editors/vim"], self.conf(), running="6.12.1", newest="6.12.1"), [])

    def test_etc_update_never_defaults_to_overwrite(self):
        self.assertEqual(self.m.DEFAULTS["ETC_UPDATE"], "preen")
        self.assertEqual(self.m.DEFAULTS["SKIP_ON_BATTERY"], "yes")
        self.assertEqual(self.m.DEFAULTS["ENABLED"], "yes")


class Runs(Base):
    def run_it(self, **conf):
        self.m.root_zfs = conf.pop("zfs", lambda: None)
        return self.m.do_run(self.conf(**conf), "test", runner=self.fake_sh)

    def test_full_run_order_and_ok_status(self):
        self.out = {"@world --pretend": WORLD_PRETEND, "--pretend --update": WORLD_PRETEND, "--depclean --pretend": DEPCLEAN_PRETEND}
        with open(self.m.PRESERVED, "w") as f:
            f.write('{"cfg": 1}')
        self.m.shutil.which = lambda n: "/usr/bin/" + n if n in ("etc-update", "horcrux-be") else None
        st = self.run_it()
        cmds = [" ".join(c) for c in self.calls]
        order = [next(i for i, c in enumerate(cmds) if pat in c) for pat in
                 ("emaint sync -a", "@world", "--depclean --pretend", "--depclean", "@preserved-rebuild", "etc-update --preen")]
        self.assertEqual(order, sorted(order))
        self.assertEqual(st["result"], "ok")
        self.assertEqual(self.m.read_status()["result"], "ok")
        self.assertFalse(os.path.exists(os.path.join(self.m.STATE_DIR, "failed")))
        self.assertEqual(st["pending"], 2)
        self.assertEqual(st["steps"]["depclean"], "removed 2")
        self.assertEqual([c for c in self.calls if "--depclean" in c and "--pretend" not in c][0][-2:], ["dev-libs/foo", "app-misc/bar"])
        for c in self.calls:
            if "--depclean" in c:
                self.assertIn("--with-bdeps=y", c)

    def test_sync_failure_is_a_failure_but_update_still_runs(self):
        self.rc = {"emaint": 1}
        self.out = {"emaint": "sync failed: network"}
        st = self.run_it()
        self.assertEqual((st["result"], st["failed_step"]), ("failed", "sync"))
        self.assertTrue(any("@world" in " ".join(c) and "--pretend" not in c for c in self.calls))
        self.assertIn("network", open(os.path.join(self.m.STATE_DIR, "failed")).read())

    def test_update_failure_skips_depclean_and_etc_update(self):
        self.rc = {"--keep-going": 1}
        self.m.shutil.which = lambda n: "/usr/bin/" + n
        st = self.run_it()
        self.assertEqual(st["failed_step"], "update")
        cmds = [" ".join(c) for c in self.calls]
        self.assertFalse(any("--depclean" in c or "etc-update" in c or "preserved" in c for c in cmds))

    def test_depclean_never_removes_the_kernel(self):
        self.out = {"--depclean --pretend": "All selected packages: sys-kernel/gentoo-kernel-bin-6.12.1\nPackages installed:   9\n"}
        st = self.run_it()
        self.assertIn("nothing to remove", st["steps"]["depclean"])
        self.assertIn("kept protected: sys-kernel/gentoo-kernel-bin", st["steps"]["depclean"])
        self.assertFalse(any("--depclean" in c and "--pretend" not in c for c in self.calls))

    def test_depclean_removes_only_the_unprotected_packages_by_name(self):
        self.out = {"--depclean --pretend": "All selected packages: =sys-kernel/gentoo-kernel-6.18.62 =dev-libs/foo-1.0\n\nPackages installed:   9\n"}
        st = self.run_it()
        real = [c for c in self.calls if "--depclean" in c and "--pretend" not in c]
        self.assertEqual(len(real), 1)
        self.assertEqual(real[0][-1], "dev-libs/foo")
        self.assertNotIn("sys-kernel/gentoo-kernel", " ".join(real[0]))
        self.assertTrue(st["steps"]["depclean"].startswith("removed 1"))

    def test_depclean_off(self):
        st = self.run_it(DEPCLEAN="no")
        self.assertEqual(st["steps"]["depclean"], "disabled")
        self.assertFalse(any("--depclean" in c for c in self.calls))

    def test_snapshot_failure_blocks_the_update(self):
        self.m.shutil.which = lambda n: "/usr/bin/" + n if n == "horcrux-be" else None
        self.rc = {"horcrux-be snapshot": 1}
        st = self.run_it(zfs=lambda: "rpool/ROOT/horcrux")
        self.assertEqual(st["failed_step"], "snapshot")
        self.assertFalse(any("@world" in c for c in self.calls))

    def test_snapshot_taken_before_update_on_zfs(self):
        self.m.shutil.which = lambda n: "/usr/bin/" + n if n == "horcrux-be" else None
        self.run_it(zfs=lambda: "rpool/ROOT/horcrux")
        cmds = [" ".join(c) for c in self.calls]
        i = next(i for i, c in enumerate(cmds) if c.startswith("horcrux-be snapshot pre-upgrade-"))
        j = next(i for i, c in enumerate(cmds) if "@world" in c)
        self.assertLess(i, j)

    def test_reboot_recorded_never_executed(self):
        self.emerge_merges = ["sys-firmware/intel-microcode-20261110-r1", "sys-kernel/gentoo-kernel-bin-6.12.9"]
        st = self.run_it()
        self.assertTrue(st["reboot_required"])
        self.assertTrue(os.path.exists(os.path.join(self.m.STATE_DIR, "reboot-required")))
        self.assertTrue(os.path.exists(self.m.RUN_REBOOT_FLAG))
        self.assertIn("sys-firmware/intel-microcode updated", st["reboot_reasons"])
        exes = {c[0] for c in self.calls}
        self.assertFalse(exes & {"reboot", "shutdown", "systemctl", "kexec", "poweroff", "halt", "telinit", "openrc"})
        code, text = self.m.check_result(self.conf(), st)
        self.assertEqual(code, 0)
        self.assertIn("reboot required", text)

    def test_reboot_marker_cleared_by_a_later_boot(self):
        self.m.add_reboot_reasons(["x updated"])
        self.assertTrue(self.m.reboot_state()[0])
        self.m.boot_time = lambda: int(time.time()) + 100
        self.assertEqual(self.m.reboot_state(), (False, []))
        self.assertFalse(os.path.exists(os.path.join(self.m.STATE_DIR, "reboot-required")))

    def test_success_clears_failed_marker(self):
        self.rc = {"emaint": 1}
        self.run_it()
        self.assertTrue(os.path.exists(os.path.join(self.m.STATE_DIR, "failed")))
        self.rc = {}
        self.run_it()
        self.assertFalse(os.path.exists(os.path.join(self.m.STATE_DIR, "failed")))


# the gentoo part of a real run log: the rsync mirror served a tree whose Manifest did not
# verify; every overlay synced, the eix post-sync hook printed its noise, `emaint sync -a` exited 1
SYNC_MIRROR_MISMATCH = """\
>>> Syncing repository 'gentoo' into '/var/db/repos/gentoo'...
>>> Starting rsync with rsync://[2606:5480:1000::300:10]/gentoo-portage...
 * Manifest timestamp: 2026-11-17 09:37:58 UTC
 * Verifying /var/db/repos/gentoo/.tmp-unverified-download-quarantine ...!!! Manifest verification failed:
Manifest mismatch for www-apps/Manifest.gz
  __size__: expected: 13136, have: 13131
[ !! ]
>>> Syncing repository 'guru' into '/var/db/repos/guru'...
=== Sync completed for guru
cannot properly execute /var/db/repos/guru/x11-wm/oxwm/oxwm-9999.ebuild
Database contains 21999 packages in 185 categories
Action: sync for repo: gentoo, returned code = 1
Action: sync for repo: guru, returned code = 0
Action: sync for repo: horcrux, returned code = 0
Action: sync for repo: canutethegreat-overlay, returned code = 0
"""

SYNC_ALL_OK_HOOK_FAILED = """\
cannot properly execute /var/db/repos/myoverlay/app-misc/foo/foo-9999.ebuild
!!! eix-postsync: hook failed
Action: sync for repo: gentoo, returned code = 0
Action: sync for repo: guru, returned code = 0
"""


class Sync(Base):
    def setUp(self):
        super().setUp()
        self.slept = []
        self.m.SLEEP = self.slept.append
        self.m.GENTOO_REPO = os.path.join(self.t.name, "gentoo")
        os.makedirs(os.path.join(self.m.GENTOO_REPO, "metadata"))
        self.stamp(time.time() - 3600)

    def stamp(self, t):
        with open(os.path.join(self.m.GENTOO_REPO, "metadata", "timestamp.chk"), "w") as f:
            f.write(time.strftime("%a, %d %b %Y %H:%M:%S +0000", time.gmtime(t)) + "\n")

    def run_it(self, **conf):
        return self.m.do_run(self.conf(**conf), "test", runner=self.fake_sh)

    def test_parsers_on_the_real_log(self):
        lines = SYNC_MIRROR_MISMATCH.splitlines()
        self.assertEqual(self.m.parse_sync(lines), {"gentoo": 1, "guru": 0, "horcrux": 0, "canutethegreat-overlay": 0})
        errs = self.m.sync_errors(lines)
        self.assertTrue(any("Manifest mismatch for www-apps/Manifest.gz" in e for e in errs))
        self.assertFalse(any("cannot properly execute" in e or e.startswith("Action:") for e in errs))
        self.assertLess(self.m.tree_age_hours(self.m.GENTOO_REPO), 2)
        self.assertIsNone(self.m.tree_age_hours(os.path.join(self.t.name, "nope")))

    def test_bad_mirror_is_retried_on_its_own_and_the_run_is_ok(self):
        self.rc = {"emaint sync -a": 1}
        self.out = {"emaint sync -a": SYNC_MIRROR_MISMATCH, "emaint sync -r gentoo": "Action: sync for repo: gentoo, returned code = 0\n"}
        st = self.run_it()
        self.assertEqual(st["result"], "ok", st.get("error"))
        self.assertIn(["emaint", "sync", "-r", "gentoo"], self.calls)
        self.assertFalse(any(c[:3] == ["emaint", "sync", "-r"] and c[3] != "gentoo" for c in self.calls))  # only the failed repo
        self.assertEqual(self.slept, [120.0])
        self.assertIn("gentoo synced on retry 1", st["steps"]["sync"])
        self.assertIn("Manifest mismatch", st["steps"]["sync"])
        self.assertEqual(self.m.check_result(self.conf(), st)[0], 0)

    def test_persistent_failure_names_the_repo_and_the_reason(self):
        self.rc = {"emaint sync": 1}
        self.out = {"emaint sync -a": SYNC_MIRROR_MISMATCH,
                    "emaint sync -r gentoo": "!!! Manifest verification failed:\nManifest mismatch for www-apps/Manifest.gz\n"
                                             "Action: sync for repo: gentoo, returned code = 1\n"}
        st = self.run_it(SYNC_RETRIES="2", SYNC_RETRY_DELAY="5")
        self.assertEqual((st["result"], st["failed_step"]), ("failed", "sync"))
        self.assertEqual(len([c for c in self.calls if c[:4] == ["emaint", "sync", "-r", "gentoo"]]), 2)
        self.assertEqual(self.slept, [5.0, 5.0])
        self.assertIn("gentoo:", st["error"])
        self.assertIn("Manifest mismatch for www-apps/Manifest.gz", st["error"])
        self.assertNotIn("returned code = 0", st["error"])  # the old message quoted three healthy repos and never said which one failed
        code, text = self.m.check_result(self.conf(), st)
        self.assertEqual(code, 2)
        self.assertIn("gentoo", text)
        self.assertTrue(any("@world" in c and "--pretend" not in c for c in self.calls))  # the update still runs on the verified old tree

    def test_post_sync_hook_failure_is_a_warning_when_the_tree_is_fresh(self):
        self.rc = {"emaint sync -a": 1}
        self.out = {"emaint sync -a": SYNC_ALL_OK_HOOK_FAILED}
        st = self.run_it()
        self.assertEqual(st["result"], "ok")
        self.assertFalse(any(c[:3] == ["emaint", "sync", "-r"] for c in self.calls))
        self.assertIn("post-sync hook", st["warnings"][0])
        self.assertIn("eix-postsync", st["warnings"][0])
        code, text = self.m.check_result(self.conf(), st)
        self.assertEqual(code, 1)
        self.assertIn("post-sync hook", text)

    def test_post_sync_hook_failure_with_a_stale_tree_is_a_failure(self):
        self.stamp(time.time() - 5 * 86400)
        self.rc = {"emaint sync -a": 1}
        self.out = {"emaint sync -a": SYNC_ALL_OK_HOOK_FAILED}
        st = self.run_it()
        self.assertEqual((st["result"], st["failed_step"]), ("failed", "sync"))
        self.assertIn("old after the sync", st["error"])

    def test_emaint_crash_without_repo_lines_is_a_failure(self):
        self.rc = {"emaint sync -a": 1}
        self.out = {"emaint sync -a": "Traceback (most recent call last):\nOSError: boom\n"}
        st = self.run_it()
        self.assertEqual(st["failed_step"], "sync")
        self.assertIn("boom", st["error"])


class Scheduling(Base):
    def test_due_gate(self):
        c = self.conf()
        self.assertTrue(self.m.due(c, {}))
        self.assertFalse(self.m.due(c, {"started": int(time.time()) - 3600}))
        self.assertTrue(self.m.due(c, {"started": int(time.time()) - 25 * 3600}))

    def test_cron_skips_when_not_due_and_when_emerge_runs(self):
        self.m.write_status({"result": "ok", "started": int(time.time()) - 60, "last_success": int(time.time())})
        self.assertEqual(self.m.cmd_run(None, True), 0)
        self.assertEqual(self.calls, [])
        self.m.write_status({"result": "ok", "started": int(time.time()) - 90000})
        self.m.other_emerge_running = lambda: True
        self.m.on_battery = lambda: False
        self.assertEqual(self.m.cmd_run(None, True), 0)
        self.assertEqual(self.calls, [])  # waiting for the other emerge is not a failure, and the interval is not reset

    def test_battery_skip_and_disabled(self):
        self.m.write_status({"result": "ok", "started": 1})
        self.m.other_emerge_running = lambda: False
        self.m.on_battery = lambda: True
        self.assertEqual(self.m.cmd_run(None, True), 0)
        self.assertEqual(self.calls, [])
        with open(self.m.CONF, "w") as f:
            f.write("ENABLED=no\n")
        self.assertEqual(self.m.cmd_run(None, False), 0)
        self.assertEqual(self.m.read_status()["result"], "disabled")

    def test_lock_excludes_a_second_run(self):
        fd = self.m.take_lock(self.m.LOCK)
        try:
            with self.assertRaises(self.m.Locked):
                self.m.take_lock(self.m.LOCK)
        finally:
            os.close(fd)
        os.close(self.m.take_lock(self.m.LOCK))


class Check(Base):
    def test_check_levels(self):
        c, t = self.conf(), int(time.time())
        self.assertEqual(self.m.check_result(c, {})[0], 1)
        self.assertEqual(self.m.check_result(c, {"result": "ok", "last_success": t - 3600, "pending": 0})[0], 0)
        self.assertEqual(self.m.check_result(c, {"result": "ok", "last_success": t - 4 * 86400})[0], 1)
        self.assertEqual(self.m.check_result(c, {"result": "ok", "last_success": t - 7 * 86400})[0], 2)
        code, text = self.m.check_result(c, {"result": "failed", "failed_step": "update", "error": "boom", "last_success": t})
        self.assertEqual(code, 2)
        self.assertIn("update", text)
        self.assertEqual(self.m.check_result(c, {"result": "ok"})[0], 2)  # never succeeded
        self.assertEqual(self.m.check_result(self.conf(ENABLED="no"), {})[0], 0)
        old = {"result": "running", "started": t - 30 * 3600, "started_iso": "x", "last_success": t}
        self.assertEqual(self.m.check_result(c, old)[0], 2)  # a run that died (power loss) is critical, not "running" forever

    def test_status_json_roundtrip(self):
        self.m.write_status({"result": "ok", "pending": 3})
        self.assertEqual(json.load(open(os.path.join(self.m.STATE_DIR, "status.json")))["pending"], 3)


if __name__ == "__main__":
    unittest.main()
