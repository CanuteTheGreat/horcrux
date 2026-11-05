"""Regression tests for the 2026-11-05 lab findings (firstboot 'tree sync not done' loop, ctg-restic on an unprovisioned host, ...).

Run the real shell scripts of the Horcrux tree with stub binaries on PATH. Read the repo tree (gentoo/), so they skip in Patronus' vendored copy of common/.
"""
import os
import stat
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
GENTOO = os.path.join(REPO, "gentoo")
HAVE = os.path.isdir(os.path.join(GENTOO, "sys-apps", "horcrux-core"))


def stub(d, name, body):
    p = os.path.join(d, name)
    with open(p, "w") as f:
        f.write("#!/bin/sh\n" + body + "\n")
    os.chmod(p, 0o755)


def run(script, stubs, env=None, args=(), timeout=60):
    with tempfile.TemporaryDirectory() as d:
        bindir = os.path.join(d, "bin")
        os.mkdir(bindir)
        for k, v in stubs.items():
            stub(bindir, k, v)
        e = dict(os.environ, PATH=bindir + ":/usr/bin:/bin", STUBBIN=bindir, HOME=d, TD=d)
        e.update(env or {})
        r = subprocess.run(["/bin/sh", script, *args], env=e, capture_output=True, text=True, timeout=timeout)
        marks = {}
        for root, _, files in os.walk(d):
            for fn in files:
                if root != bindir:
                    marks[os.path.join(root, fn)[len(d):]] = open(os.path.join(root, fn), errors="replace").read()
        return r, marks


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class FirstbootSync(unittest.TestCase):
    SCRIPT = os.path.join(GENTOO, "sys-apps", "horcrux-firstboot", "files", "horcrux-firstboot")
    # the script writes /var/lib/horcrux and /run: point them into TD by running a patched copy
    def _run(self, webrsync, emaint, extra_env=None):
        with tempfile.TemporaryDirectory() as d:
            src = open(self.SCRIPT).read().replace("/var/lib/horcrux", d + "/state").replace("/run/horcrux-firstboot.pid", d + "/pid") \
                .replace("/usr/libexec/horcrux-firstboot.d/", d + "/hooks/") \
                .replace("PATH=/usr/local/sbin", "PATH=$STUBBIN:/usr/local/sbin", 1)
            sp = os.path.join(d, "fb.sh")
            open(sp, "w").write(src)
            stubs = {
                "ip": 'echo "default via 10.0.0.1 dev eth0"',
                "sleep": "echo sleep $1 >> $TD/sleeps",
                "emerge-webrsync": webrsync,
                "portageq": 'case "$1" in get_repos) echo gentoo; echo horcrux; echo horcrux-local ;; get_repo_path) echo "$TD/repos/$3" ;; repos_config) printf "[horcrux]\\nsync-uri = https://x\\nauto-sync = Yes\\n[horcrux-local]\\nlocation = /x\\nauto-sync = False\\n" ;; esac',
                "emaint": emaint,
                "git": "exit 1",
            }
            os.makedirs(d + "/repos/horcrux-local/keep")
            open(d + "/repos/horcrux-local/keep/file", "w").write("private overlay content")
            os.makedirs(d + "/repos/horcrux")
            env = {"TD": d, "FIRSTBOOT_SETTLE": "0", "FIRSTBOOT_POLL": "0"}
            env.update(extra_env or {})
            r, marks = run(sp, stubs, env, timeout=60)
            keep = os.path.exists(d + "/repos/horcrux-local/keep/file")
            done = os.path.exists(d + "/state/firstboot-done")
            doneTxt = open(d + "/state/firstboot-done").read() if done else ""
            sleeps = open(d + "/sleeps").read() if os.path.exists(d + "/sleeps") else ""
            return r, done, doneTxt, sleeps, keep

    def test_webrsync_timestamp_newer_and_sync_disabled_repo_complete(self):
        # the real gringotts/Surface failure: webrsync exits 1 (local timestamp newer), emaint exits 1 for an auto-sync=no overlay
        webrsync = 'echo " * The current local timestamp is newer than the timestamp"; exit 1'
        emaint = 'if [ "$2" = horcrux-local ]; then echo " * The specified repo(s) have sync disabled: horcrux-local"; exit 1; fi; echo synced; exit 0'
        r, done, txt, sleeps, keep = self._run(webrsync, emaint)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue(done, "first boot never completed:\n" + r.stdout)
        self.assertNotIn("tree sync not done", r.stdout)
        self.assertNotIn("partial", txt)
        self.assertTrue(keep, "the private local overlay was wiped by the first-sync cleanup")

    def test_real_failure_retries_then_marks_done_with_warning(self):
        r, done, txt, sleeps, keep = self._run("echo boom; exit 1", "exit 0", {"FIRSTBOOT_MAX_FAILS": "3", "FIRSTBOOT_RETRY": "0"})
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue(done)
        self.assertIn("partial", txt)
        self.assertGreaterEqual(r.stdout.count("tree sync not done"), 2)

    def test_clean_sync(self):
        r, done, txt, sleeps, keep = self._run("exit 0", "exit 0")
        self.assertTrue(done)
        self.assertNotIn("partial", txt)


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class CtgRestic(unittest.TestCase):
    SCRIPT = os.path.join(GENTOO, "sys-apps", "horcrux-backup", "files", "ctg-restic")

    def _go(self, args, conf_files=None):
        with tempfile.TemporaryDirectory() as d:
            conf = os.path.join(d, "conf")
            os.mkdir(conf)
            for k, v in (conf_files or {}).items():
                open(os.path.join(conf, k), "w").write(v)
            r = subprocess.run(["/bin/bash", self.SCRIPT, *args], env=dict(os.environ, PATH="/usr/bin:/bin", CTG_BACKUP_CONF=conf, CTG_BACKUP_STATE=d + "/state"),
                               capture_output=True, text=True, timeout=30)
            return r, os.path.exists(d + "/state/last-backup-ok")

    def test_unprovisioned_scheduled_commands_exit_zero_with_a_clear_message(self):
        for cmd in ("backup", "prune", "check", "content"):
            r, ok = self._go([cmd])
            self.assertEqual(r.returncode, 0, f"{cmd}: {r.stdout}{r.stderr}")
            self.assertIn("not provisioned", r.stdout + r.stderr)
            self.assertNotIn("No such file", r.stdout + r.stderr)
            self.assertFalse(ok, "an unprovisioned host must not record a successful backup")

    def test_unprovisioned_passthrough_is_a_config_error_not_a_stack_of_fatal(self):
        r, _ = self._go(["snapshots"])
        self.assertEqual(r.returncode, 78, r.stdout + r.stderr)
        self.assertIn("not provisioned", r.stdout + r.stderr)
        self.assertNotIn("No such file", r.stdout + r.stderr)


if __name__ == "__main__":
    unittest.main()
