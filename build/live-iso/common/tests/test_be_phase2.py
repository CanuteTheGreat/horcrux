"""horcrux-be phase 2 (pure logic, fakes for zfs/findmnt/grub-editenv): auto-fallback state machine + grub.cfg, migrate-data, pre-upgrade."""
import os
import sys
import tempfile
import types
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import be as BE  # noqa: E402
from liveinst import befallback as FB  # noqa: E402
from liveinst import bemigrate as MG  # noqa: E402

ORIG_SE = FB.service_enabled


class FakeEnv:
    """in-memory grub-editenv"""
    def __init__(self):
        self.v = {}

    def __call__(self, path, *args):
        ok = types.SimpleNamespace(returncode=0, stdout="", stderr="")
        if args[0] in ("create", "set"):
            open(path, "a").close()
        if args[0] == "create":
            self.v = {}
        elif args[0] == "list":
            ok.stdout = "".join(f"{k}={v}\n" for k, v in self.v.items())
        elif args[0] == "set":
            for a in args[1:]:
                k, _, val = a.partition("=")
                self.v[k] = val
        elif args[0] == "unset":
            self.v.pop(args[1], None)
        return ok


def grub_boot(cfg_conf, env, active):
    """what GRUB does with the generated preamble for one boot: returns the menu id it boots (a faithful transcription of the cfg logic,
    kept honest by the string checks in test_preamble_text and by the QEMU test that runs the real GRUB)"""
    e = {k: env.get(k, "") for k in FB.VARS}
    default = f"be-{active}"
    if e["hx_trial"] == active:
        if e["hx_tries"] == "x" * FB.tries_max(cfg_conf) and e["hx_good"]:
            default = f"be-{e['hx_good']}"
            env["hx_failed"] = e["hx_trial"]
        else:
            env["hx_tries"] = e["hx_tries"] + "x"
    return default


class Fallback(unittest.TestCase):
    def setUp(self):
        self.env = FakeEnv()
        self.tmp = tempfile.TemporaryDirectory()
        self.boot = self.tmp.name
        self.ev = os.path.join(self.boot, "events.jsonl")
        self.p = [mock.patch.object(FB, "_editenv", self.env), mock.patch.object(FB, "service_enabled", lambda n, root="/": True if n == "horcrux-be-health" else ORIG_SE(n, root)), mock.patch.object(FB, "EVENTS", self.ev)]
        for p in self.p:
            p.start()
        self.c = BE.conf("/nonexistent")
        self.pool = []

    def tearDown(self):
        for p in self.p:
            p.stop()
        self.tmp.cleanup()

    def settle(self, running):
        return FB.settle(self.c, running, self.boot, set_bootfs=lambda be: self.pool.append(("bootfs", be)),
                         mark_failed=lambda be: self.pool.append(("failed", be)), regen=lambda: self.pool.append(("regen",)),)

    def test_preamble_text(self):
        t = "\n".join(FB.grub_preamble(self.c, "new"))
        self.assertIn('load_env -f "${hx_env}" hx_trial hx_tries hx_good hx_failed', t)
        self.assertIn('set default="be-new"', t)
        self.assertIn('[ "${hx_tries}" = "xxx" -a -n "${hx_good}" ]', t)
        self.assertIn('set default="be-${hx_good}"', t)
        self.c["FALLBACK_TRIES"] = "5"
        self.assertIn('"xxxxx"', "\n".join(FB.grub_preamble(self.c, "new")))
        self.c["FALLBACK"] = "no"
        self.assertEqual(FB.grub_preamble(self.c, "new"), ['set default="be-new"'])

    def test_render_has_preamble_after_root_search_and_panic(self):
        t = BE.render_grub(["old", "new"], "new", "rpool", "u-u-i-d", "", 5, None, fallback=self.c)
        self.assertLess(t.index("search --no-floppy"), t.index("load_env"))
        self.assertLess(t.index("load_env"), t.index("menuentry"))
        self.assertIn("root=ZFS=rpool/ROOT/new zfsforce=1 ro panic=10", t)
        off = dict(self.c, FALLBACK="no")
        self.assertNotIn("load_env", BE.render_grub(["old", "new"], "new", "rpool", "u", "", 5, None, fallback=None))
        self.assertNotIn("panic=", BE.render_grub(["new"], "new", "rpool", "u", "", 5, None, fallback=None))
        del off

    def test_broken_new_environment_falls_back_after_n_boots_then_stays(self):
        self.assertTrue(FB.arm(self.c, "new", "old", self.boot))
        boots = [grub_boot(self.c, self.env.v, "new") for _ in range(6)]
        self.assertEqual(boots, ["be-new"] * 3 + ["be-old"] * 3)
        # the old environment comes up healthy
        self.assertEqual(self.settle("old"), "fallback")
        self.assertIn(("bootfs", "old"), self.pool)
        self.assertIn(("failed", "new"), self.pool)
        ev = FB.read_events(path=self.ev)
        self.assertEqual([e["kind"] for e in ev], ["armed", "fallback"])
        self.assertIn("new", ev[-1]["detail"])
        # state is clean: nothing on trial, and with bootfs=old GRUB just boots old
        self.assertEqual(FB.state(self.c, self.boot)["trial"], None)
        self.assertEqual(grub_boot(self.c, self.env.v, "old"), "be-old")

    def test_good_new_environment_ends_the_trial(self):
        FB.arm(self.c, "new", "old", self.boot)
        self.assertEqual(grub_boot(self.c, self.env.v, "new"), "be-new")
        self.assertEqual(self.settle("new"), "boot-ok")
        for _ in range(5):
            self.assertEqual(grub_boot(self.c, self.env.v, "new"), "be-new")
        self.assertEqual(self.env.v.get("hx_good"), "new")

    def test_unhealthy_boot_keeps_counting_and_manual_menu_pick_does_not_end_trial(self):
        FB.arm(self.c, "new", "old", self.boot)
        grub_boot(self.c, self.env.v, "new")
        self.assertIsNone(self.settle("old"))          # user picked old by hand in the menu: no event, trial stays
        self.assertEqual(FB.state(self.c, self.boot)["trial"], "new")
        self.assertEqual(FB.state(self.c, self.boot)["tries"], 1)

    def test_fallback_survives_a_reboot_before_the_good_environment_settles(self):
        FB.arm(self.c, "new", "old", self.boot)
        for _ in range(3):
            grub_boot(self.c, self.env.v, "new")
        self.assertEqual(grub_boot(self.c, self.env.v, "new"), "be-old")
        self.assertEqual(grub_boot(self.c, self.env.v, "new"), "be-old")     # reboot again before the health service ran: still old
        self.assertEqual(self.env.v["hx_failed"], "new")

    def test_no_health_service_means_no_trial(self):
        self.assertFalse(FB.arm(self.c, "new", "old", self.boot, healthy_service=lambda: False))
        self.assertNotIn("hx_trial", self.env.v)

    def test_arm_rules(self):
        self.assertFalse(FB.arm(self.c, "same", "same", self.boot))
        self.assertFalse(FB.arm(self.c, "x", None, self.boot))
        self.assertFalse(FB.arm(self.c, "x", "old", self.boot, has_kernel=lambda b: False))
        self.c["FALLBACK"] = "no"
        self.assertFalse(FB.arm(self.c, "x", "old", self.boot))
        self.assertNotIn("hx_trial", self.env.v)
        self.c["FALLBACK"] = "yes"
        FB.arm(self.c, "x", "old", self.boot)
        FB.arm(self.c, "old", "old", self.boot)          # activating the running BE itself clears the trial
        self.assertNotIn("hx_trial", self.env.v)

    def test_health_waits_then_gives_up(self):
        calls = iter(["api down", "api down", None])
        ok, why = FB.wait_healthy(self.c, timeout=100, pause=0, sleep=lambda s: None, problem=lambda: next(calls))
        self.assertTrue(ok)
        ok, why = FB.wait_healthy(self.c, timeout=0, pause=0, sleep=lambda s: None, problem=lambda: "horcrux-api is enabled but not answering")
        self.assertFalse(ok)
        self.assertIn("not answering", why)

    def test_health_problem_checks_api_only_when_enabled(self):
        zp = lambda *a, **k: types.SimpleNamespace(stdout="ONLINE\n", returncode=0)
        with tempfile.TemporaryDirectory() as root:
            self.assertIsNone(FB.health_problem(self.c, root, api=lambda: False, run=zp))
            os.makedirs(f"{root}/etc/runlevels/default")
            open(f"{root}/etc/runlevels/default/horcrux-api", "w").close()
            self.assertIn("horcrux-api", FB.health_problem(self.c, root, api=lambda: False, run=zp))
            self.assertIsNone(FB.health_problem(self.c, root, api=lambda: True, run=zp))
        with tempfile.TemporaryDirectory() as root:         # systemd flavour of "enabled"
            os.makedirs(f"{root}/etc/systemd/system/multi-user.target.wants")
            open(f"{root}/etc/systemd/system/multi-user.target.wants/horcrux-api.service", "w").close()
            self.assertIn("horcrux-api", FB.health_problem(self.c, root, api=lambda: False, run=zp))

    def test_activate_arms_trial_and_reports_it(self):
        ds = ["horcrux", "new"]
        os.makedirs(os.path.join(self.boot, "be", "horcrux"))
        os.makedirs(os.path.join(self.boot, "be", "new"))
        open(os.path.join(self.boot, "be", "horcrux", "vmlinuz"), "w").close()
        open(os.path.join(self.boot, "be", "new", "vmlinuz"), "w").close()
        with mock.patch.object(BE, "be_datasets", lambda pool: ds), mock.patch.object(BE, "running_be", lambda: "horcrux"), \
                mock.patch.object(BE, "sh", lambda *a, **k: ""), mock.patch.object(BE, "grubcfg"):
            BE.cmd_activate(self.c, "new", self.boot)
        st = FB.state(self.c, self.boot)
        self.assertEqual((st["trial"], st["good"], st["tries"]), ("new", "horcrux", 0))

    def test_mark_good_cli_path_closes_trial(self):
        FB.arm(self.c, "new", "old", self.boot)
        with mock.patch.object(BE, "running_be", lambda: "new"), mock.patch.object(BE, "grubcfg"), mock.patch.object(BE, "sh", lambda *a, **k: ""), \
                mock.patch.object(BE, "BOOT", self.boot):
            self.assertEqual(BE.cmd_mark_good(self.c, wait=False, boot=self.boot), 0)
        self.assertIsNone(FB.state(self.c, self.boot)["trial"])


class FakeZ:
    """zfs/findmnt/rsync-less fake for migrate_one: datasets mount by recording their mountpoint"""
    def __init__(self, pool="rpool"):
        self.ds, self.mounted, self.calls = {}, {}, []
        self.pool = pool

    def run(self, cmd, **kw):
        self.calls.append(cmd)
        ok = types.SimpleNamespace(returncode=0, stdout="", stderr="")
        if cmd[0] == "findmnt":
            if "-T" in cmd:
                p = cmd[-1]
                while p and p != "/":
                    if p in self.mounted:
                        ok.stdout = self.mounted[p] + "\n"
                        return ok
                    p = os.path.dirname(p)
                ok.stdout = "rpool/ROOT/horcrux\n"
            else:
                p = cmd[-1]
                if p in self.mounted:
                    ok.stdout = self.mounted[p] + "\n"
                else:
                    ok.returncode = 1
            return ok
        if cmd[:2] == ["zfs", "list"] and "avail" in cmd:
            ok.stdout = "100000000000\n"
            return ok
        if cmd[:2] == ["zfs", "list"]:
            ok.returncode = 0 if cmd[-1] in self.ds or cmd[-1] == self.pool else 1
            return ok
        if cmd[:2] == ["zfs", "get"]:
            d = self.ds.get(cmd[-1])
            if d is None:
                ok.returncode = 1
            else:
                ok.stdout = f"{MG.MIGRATING_PROP}\t{d.get('prop', '-')}\nmounted\tno\nmountpoint\t{d.get('mp', '-')}\n"
            return ok
        if cmd[:2] == ["zfs", "create"]:
            props = {cmd[i + 1].split("=")[0]: cmd[i + 1].split("=", 1)[1] for i, a in enumerate(cmd) if a == "-o"}
            self.ds[cmd[-1]] = {"mp": props.get("mountpoint", ""), "prop": props.get(MG.MIGRATING_PROP, "-"), "canmount": props.get("canmount", "on")}
            return ok
        if cmd[:2] == ["zfs", "mount"]:
            d = self.ds[cmd[-1]]
            os.makedirs(d["mp"], exist_ok=True)
            self.mounted[d["mp"]] = cmd[-1]
            return ok
        if cmd[:2] == ["zfs", "unmount"]:
            d = self.ds[cmd[-1]]
            self.mounted.pop(d["mp"], None)
            return ok
        if cmd[:2] == ["zfs", "set"]:
            k, _, v = cmd[2].partition("=")
            d = self.ds[cmd[-1]]
            if k == "mountpoint":
                d["mp"] = v
            elif k == "canmount":
                d["canmount"] = v
                if v == "on":
                    os.makedirs(d["mp"], exist_ok=True)
                    self.mounted[d["mp"]] = cmd[-1]
            return ok
        if cmd[:2] == ["zfs", "inherit"]:
            self.ds[cmd[-1]]["prop"] = "-"
            return ok
        if cmd[0] in ("rsync", "cp"):
            return subprocess_real(cmd)
        raise AssertionError(cmd)


def subprocess_real(cmd):
    import subprocess
    return subprocess.run(cmd, capture_output=True, text=True)


class Migrate(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.vms = os.path.join(self.root, "var/lib/horcrux/vms")
        os.makedirs(self.vms)
        with open(os.path.join(self.vms, "a.qcow2"), "wb") as f:
            f.write(os.urandom(4096))
        os.makedirs(os.path.join(self.vms, "sub"))
        with open(os.path.join(self.vms, "sub", "b.xml"), "w") as f:
            f.write("<vm/>")
        os.symlink("a.qcow2", os.path.join(self.vms, "link"))
        self.z = FakeZ()
        self.c = BE.conf("/nonexistent")
        self.c["DATA_PATHS"] = self.vms
        self.tr = mock.patch.object(MG, "TMPROOT", os.path.join(self.root, "run"))
        self.tr.start()
        self.op = mock.patch.object(MG, "open_files", lambda p, proc="/proc": self.opens)
        self.opens = []
        self.op.start()
        self.dsn = f"rpool/{self.vms.strip('/')}"

    def tearDown(self):
        self.tr.stop()
        self.op.stop()
        self.tmp.cleanup()

    def test_plan_dry_run_changes_nothing(self):
        out = []
        rc = MG.migrate(self.c, dry_run=True, run=self.z.run, say=out.append)
        self.assertEqual(rc, 0)
        self.assertTrue(any(l.startswith("MOVE") for l in out), out)
        self.assertEqual([c for c in self.z.calls if c[:2] in (["zfs", "create"], ["zfs", "set"])], [])
        self.assertTrue(os.path.exists(os.path.join(self.vms, "a.qcow2")))

    def test_open_file_blocks_everything(self):
        self.opens = [("4242", "qemu-system-x86")]
        out = []
        self.assertEqual(MG.migrate(self.c, run=self.z.run, say=out.append, snapshot_cmd=lambda n: self.fail("must not snapshot when blocked")), 3)
        self.assertTrue(any("pid 4242" in l for l in out))
        self.assertEqual([c for c in self.z.calls if c[:2] == ["zfs", "create"]], [])

    def test_migrates_verifies_swaps_and_is_idempotent(self):
        snaps = []
        out = []
        ev = os.path.join(self.root, "ev.jsonl")
        with mock.patch.object(FB, "EVENTS", ev):
            rc = MG.migrate(self.c, run=self.z.run, say=out.append, snapshot_cmd=lambda n: snaps.append(n) or f"horcrux@{n}")
        self.assertEqual(rc, 0, out)
        self.assertEqual(len(snaps), 1)
        self.assertTrue(snaps[0].startswith("migrate-data-"))
        self.assertEqual(self.z.mounted.get(self.vms), self.dsn)
        self.assertEqual(self.z.ds[self.dsn]["prop"], "-")                 # migrating marker removed
        self.assertEqual(self.z.ds[self.dsn]["canmount"], "on")
        self.assertEqual(self.z.ds[self.dsn]["mp"], self.vms)
        self.assertTrue(any(c[:3] == ["zfs", "create", "-o"] and c[-1] == "rpool/" + "/".join(self.vms.strip("/").split("/")[:-1]) for c in self.z.calls))
        # parents are containers, never mountable
        for c in self.z.calls:
            if c[:2] == ["zfs", "create"] and c[-1] != self.dsn:
                self.assertIn("canmount=off", c)
        self.assertFalse(os.path.exists(self.vms + ".hx-migrating"))
        with open(ev) as f:
            self.assertTrue(any("migrated" in l for l in f))
        # the data a "VM" had is in the dataset's mount (here: the temp copy dir was the dataset's backing dir)
        # run again: path is now on its own dataset
        out2 = []
        self.assertEqual(MG.migrate(self.c, run=self.z.run, say=out2.append, snapshot_cmd=lambda n: self.fail("idempotent")), 0)
        self.assertTrue(any("already on its own dataset" in l for l in out2), out2)

    def test_copy_mismatch_leaves_original_alone(self):
        real = MG.copy_tree

        def bad_copy(src, dst, run=None):
            real(src, dst)
            os.truncate(os.path.join(dst, "a.qcow2"), 10)
        with mock.patch.object(MG, "copy_tree", bad_copy):
            with self.assertRaises(RuntimeError) as cm:
                MG.migrate(self.c, run=self.z.run, say=lambda m: None, snapshot_cmd=lambda n: n)
        self.assertIn("does not match", str(cm.exception))
        self.assertEqual(os.path.getsize(os.path.join(self.vms, "a.qcow2")), 4096)
        self.assertNotIn(self.vms, self.z.mounted)

    def test_existing_unrelated_dataset_is_never_touched(self):
        self.z.ds[self.dsn] = {"mp": "/x", "prop": "-"}
        out = []
        self.assertEqual(MG.migrate(self.c, run=self.z.run, say=out.append), 3)
        self.assertTrue(any("not touching it" in l for l in out))

    def test_half_finished_run_resumes(self):
        self.z.ds[self.dsn] = {"mp": "/x", "prop": "yes", "canmount": "noauto"}
        rows = MG.plan(self.c, run=self.z.run)
        self.assertEqual(rows[0]["action"], "resume")

    def test_empty_absent_and_foreign_paths_are_skipped(self):
        empty = os.path.join(self.root, "empty")
        os.makedirs(empty)
        rows = MG.plan(dict(self.c, DATA_PATHS=f"{empty} {self.root}/nope"), run=self.z.run)
        self.assertEqual([r["action"] for r in rows], ["skip", "skip"])
        z2 = FakeZ()
        z2.mounted[self.vms] = "/dev/md0"
        self.assertIn("not on the ZFS", MG.plan(self.c, run=z2.run)[0]["why"])

    def test_verify_copy_detects_differences(self):
        with tempfile.TemporaryDirectory() as d:
            MG.copy_tree(self.vms, d)
            self.assertEqual(MG.verify_copy(self.vms, d), [])
            os.unlink(os.path.join(d, "sub", "b.xml"))
            self.assertEqual(MG.verify_copy(self.vms, d), [("sub/b.xml", "missing in the copy")])


class PreUpgrade(unittest.TestCase):
    def test_always_snapshots_unless_disabled(self):
        c = BE.conf("/nonexistent")
        with mock.patch.object(BE, "cmd_snapshot", lambda c, n, boot=None: f"horcrux@{n}"), mock.patch.object(BE.shutil, "which", lambda x: "/sbin/zfs"), \
                mock.patch.object(BE, "running_be", lambda: "horcrux"), mock.patch.object(FB, "record_event", lambda *a, **k: None):
            o = BE.cmd_pre_upgrade(c)
            self.assertTrue(o["snapshot"].startswith("horcrux@pre-upgrade-"))
            self.assertTrue(BE.AUTO_SNAP.fullmatch(o["snapshot"].split("@")[1]))   # the Packages banner and `rollback` recognise it
            c["AUTO_SNAPSHOT"] = "no"
            self.assertIn("switched off", BE.cmd_pre_upgrade(c)["skipped"])


class InitParity(unittest.TestCase):
    """both inits get the ZFS boot services and the health service; both flavours run the same command"""
    OV = os.path.join(HERE, "..", "overlay")

    def test_enable_commands_for_both_inits(self):
        from liveinst import initsys
        self.assertEqual(initsys.enable_cmds("openrc", "horcrux-be-health", "default"), [["rc-update", "add", "horcrux-be-health", "default"]])
        self.assertEqual(initsys.enable_cmds("systemd", "horcrux-be-health", "default"), [["systemctl", "enable", "horcrux-be-health.service"]])
        self.assertEqual(initsys.enable_cmds("systemd", "zfs-mount", "boot"), [["systemctl", "enable", "zfs-mount.service", "zfs.target"]])
        self.assertEqual(initsys.enable_cmds("systemd", "zfs-import", "boot"), [["systemctl", "enable", "zfs-import-cache.service", "zfs-import.target"]])

    def test_installer_enables_the_health_service_through_initsys(self):
        src = open(os.path.join(self.OV, "usr/lib/live-installer/liveinst/zfsroot.py")).read()
        self.assertIn('initsys.enable(inst, "horcrux-be-health", "default")', src)
        self.assertIn('for svc in ("zfs-import", "zfs-mount", "zfs-zed")', src)

    def test_both_flavours_run_the_same_command(self):
        rc = open(os.path.join(self.OV, "etc/init.d/horcrux-be-health")).read()
        unit = open(os.path.join(self.OV, "usr/lib/systemd/system/horcrux-be-health.service")).read()
        cmd = "/usr/bin/horcrux-be mark-good --wait --detach"
        self.assertIn(cmd, rc)
        self.assertIn(cmd, unit)
        self.assertTrue(os.access(os.path.join(self.OV, "etc/init.d/horcrux-be-health"), os.X_OK))

    def test_systemd_unit_is_a_boot_service_not_a_blocker(self):
        unit = open(os.path.join(self.OV, "usr/lib/systemd/system/horcrux-be-health.service")).read()
        self.assertIn("WantedBy=multi-user.target", unit)
        self.assertIn("Type=oneshot", unit)       # the wait itself is forked (--detach): multi-user.target is never held for minutes


if __name__ == "__main__":
    unittest.main()
