"""Tests for files/50-horcrux-md5-cache, the repo.postsync.d hook (no Portage, no root):
python3 -m unittest discover -s gentoo/sys-apps/horcrux-autoupdate/tests"""
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
HOOK = os.path.join(HERE, "..", "files", "50-horcrux-md5-cache")


@unittest.skipUnless(shutil.which("git") and shutil.which("sh"), "needs git and sh")
class Md5CacheHook(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.TemporaryDirectory()
        d = self.t.name
        self.bin = os.path.join(d, "bin")
        os.makedirs(self.bin)
        self.calls = os.path.join(d, "egencache-calls")
        self.fake("egencache", f'echo "$@" >> {self.calls}\n[ -e {d}/egencache-fail ] && exit 1\n'
                               'for a; do case $a in --repo=*) r=${a#--repo=};; esac; done\nmkdir -p "$REPO_PATH/metadata/md5-cache/cat"\n'
                               'echo x > "$REPO_PATH/metadata/md5-cache/cat/pkg-1"\nexit 0\n')
        self.env = dict(os.environ, PATH=self.bin + os.pathsep + os.environ["PATH"])

    def tearDown(self):
        self.t.cleanup()

    def fake(self, name, body):
        p = os.path.join(self.bin, name)
        with open(p, "w") as f:
            f.write("#!/bin/sh\n" + body)
        os.chmod(p, 0o755)

    def repo(self, name, tracked_cache=False):
        p = os.path.join(self.t.name, name)
        os.makedirs(os.path.join(p, "cat", "pkg"))
        with open(os.path.join(p, "cat", "pkg", "pkg-1.ebuild"), "w") as f:
            f.write("EAPI=8\n")
        if tracked_cache:
            os.makedirs(os.path.join(p, "metadata", "md5-cache", "cat"))
            with open(os.path.join(p, "metadata", "md5-cache", "cat", "pkg-1"), "w") as f:
                f.write("upstream\n")
        g = ["git", "-c", "user.email=t@t", "-c", "user.name=t", "-C", p]
        subprocess.run(g + ["init", "-q"], check=True)
        subprocess.run(g + ["add", "-A"], check=True)
        subprocess.run(g + ["commit", "-qm", "init"], check=True)
        return p

    def hook(self, name, path):
        env = dict(self.env, REPO_PATH=path)
        return subprocess.run(["sh", HOOK, name, "git://x", path], env=env, capture_output=True, text=True)

    def called(self):
        try:
            return open(self.calls).read().split("\n")[:-1]
        except OSError:
            return []

    def test_overlay_without_cache_gets_one_out_of_git_status(self):
        p = self.repo("private-overlay")
        r = self.hook("private-overlay", p)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(len(self.called()), 1)
        self.assertIn("--update --repo=private-overlay", self.called()[0])
        self.assertTrue(os.path.exists(os.path.join(p, "metadata", "md5-cache", "cat", "pkg-1")))
        st = subprocess.run(["git", "-C", p, "status", "--porcelain"], capture_output=True, text=True).stdout
        self.assertEqual(st, "")  # the next `git pull` sees a clean tree
        self.hook("private-overlay", p)
        with open(os.path.join(p, ".git", "info", "exclude")) as f:
            self.assertEqual(f.read().count("/metadata/md5-cache/"), 1)

    def test_tracked_upstream_cache_and_gentoo_are_left_alone(self):
        p = self.repo("guru", tracked_cache=True)
        self.assertEqual(self.hook("guru", p).returncode, 0)
        g = self.repo("gentoo")
        self.assertEqual(self.hook("gentoo", g).returncode, 0)
        self.assertEqual(self.called(), [])
        with open(os.path.join(p, "metadata", "md5-cache", "cat", "pkg-1")) as f:
            self.assertEqual(f.read(), "upstream\n")

    def test_a_failing_egencache_never_fails_the_sync(self):
        open(os.path.join(self.t.name, "egencache-fail"), "w").close()
        p = self.repo("steam-overlay")
        r = self.hook("steam-overlay", p)
        self.assertEqual(r.returncode, 0)
        self.assertIn("egencache", r.stderr)

    def test_non_git_and_missing_args_are_ignored(self):
        p = os.path.join(self.t.name, "plain")
        os.makedirs(p)
        self.assertEqual(self.hook("plain", p).returncode, 0)
        self.assertEqual(subprocess.run(["sh", HOOK], env=self.env).returncode, 0)
        self.assertEqual(self.called(), [])


if __name__ == "__main__":
    unittest.main()
