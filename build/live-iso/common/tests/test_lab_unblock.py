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


def _rd(*parts):
    with open(os.path.join(REPO, *parts)) as f:
        return f.read()


@unittest.skipUnless(HAVE, "Horcrux repo tree only")
class ProductBlockers(unittest.TestCase):
    """The 2026-11-05 lab blockers that are package/image definitions (the lab re-proves them on real hardware)."""

    def test_manager_is_a_keyworded_package_installed_by_the_core_layer(self):
        import glob
        eb = _rd("gentoo", "app-emulation", "horcrux-bin", "horcrux-bin-0.1.0.ebuild")
        kw = [l for l in eb.splitlines() if l.startswith("KEYWORDS=")][0]
        self.assertNotIn("~", kw, "the manager must not be masked by a testing keyword (the original blocker)")
        for arch in ("amd64", "arm64"):
            self.assertIn(arch, kw)
        self.assertIn("app-emulation/qemu[spice", eb)
        self.assertIn("net-fs/nfs-utils", eb)                           # shared (NFS) storage for a cluster: lab 2026-11-05 had to install it by hand
        self.assertIn("!app-emulation/horcrux", eb)                     # same files as the source build: never both
        core = _rd("build", "live-iso", "project", "variants", "core", "Dockerfile")
        self.assertIn("app-emulation/horcrux-bin", core)
        self.assertIn("--select sys-apps/horcrux-core app-emulation/horcrux-bin", core)
        self.assertIn("GATE: qemu built without SPICE", core)
        # desktop editions are built FROM the core image: the manager reaches gnome-light and every machine layer
        gl = _rd("build", "live-iso", "project", "variants", "gnome-light", "Dockerfile")
        self.assertIn("BASE", gl)

    def test_tarball_contract_between_image_build_and_ebuild(self):
        app = _rd("build", "live-iso", "project", "Dockerfile.app")
        eb = _rd("gentoo", "app-emulation", "horcrux-bin", "horcrux-bin-0.1.0.ebuild")
        self.assertIn("horcrux-manager-$(uname -m).tar.zst", app)
        self.assertIn("horcrux-manager-$(uname -m).tar.zst", eb.replace('horcrux-manager-$(uname -m)', 'horcrux-manager-$(uname -m)'))
        for member in ("usr/bin/horcrux-api", "usr/bin/horcrux ", "usr/share/horcrux/ui", "etc/init.d/horcrux-api"):
            self.assertIn(member, app)
        ign = _rd("build", "live-iso", "project", "Dockerfile.app.dockerignore")
        self.assertIn("!build/live-iso/project/overlay/etc/init.d/horcrux-api", ign)   # build/live-iso is ignored otherwise: COPY would fail

    def test_qemu_has_spice_in_image_and_on_machines(self):
        for path in (("build", "live-iso", "project", "portage", "package.use", "horcrux-virt"), ("gentoo", "app-emulation", "horcrux-bin", "files", "package.use")):
            line = [l for l in _rd(*path).splitlines() if l.startswith("app-emulation/qemu")][0].split()
            for flag in ("spice", "usbredir", "virtfs", "vnc"):
                self.assertIn(flag, line[1:], path[-1])
        use = _rd("gentoo", "sys-apps", "horcrux-repo-config", "files", "horcrux-roles.use")
        self.assertRegex(use, r"(?m)^app-emulation/qemu .*\bspice\b")      # first install of the manager on an existing machine resolves without autounmask
        # a global -spice / USE="-spice" in the image's own config would silently undo it
        for root, _, files in os.walk(os.path.join(REPO, "build", "live-iso")):
            for fn in files:
                if root.endswith("portage") or "package.use" in root or fn == "make.conf":
                    t = open(os.path.join(root, fn), errors="replace").read()
                    self.assertNotRegex(t, r"(?m)^[^#\n]*\s-spice\b", os.path.join(root, fn))

    def test_container_role_package(self):
        eb = _rd("gentoo", "sys-apps", "horcrux-role-containers", "horcrux-role-containers-1.ebuild")
        for atom in ("app-containers/incus", "app-containers/podman", "app-containers/buildah"):
            self.assertIn(atom, eb)
        kws = _rd("gentoo", "sys-apps", "horcrux-repo-config", "files", "horcrux-roles.keywords")
        for atom in ("app-containers/incus", "app-containers/podman", "app-containers/buildah"):
            self.assertRegex(kws, r"(?m)^" + atom + r" ~amd64")        # keywords arrive with repo-config: before the role package resolves

    def test_changed_packages_got_a_revision_so_installed_machines_update(self):
        for cat, pn, want in (("sys-apps", "horcrux-firstboot", "horcrux-firstboot-1-r1.ebuild"), ("sys-apps", "horcrux-backup", "horcrux-backup-1-r5.ebuild"),
                              ("sys-apps", "horcrux-repo-config", "horcrux-repo-config-1-r2.ebuild")):
            self.assertIn(want, os.listdir(os.path.join(GENTOO, cat, pn)))


if __name__ == "__main__":
    unittest.main()
