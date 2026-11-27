"""Owner finding 2026-11-27: with `mouse on`, drag-selecting in tmux (Terminator/GNOME Terminal = VTE, no OSC 52) copied only into tmux's own buffer and the
highlight vanished on release; no clipboard tool was installed. app-misc/horcrux-tmux-config ships /etc/tmux.conf + /usr/bin/horcrux-clip-copy, pulled by every
desktop edition meta (never the server core). Static checks on the repo tree, plus a REAL tmux run (skipped when tmux is absent): SGR mouse drag through a pty,
with stub wl-copy/xclip, must land the selected text in the clipboard tool, keep the selection, and leave `mouse` alone. Patronus' vendored common/ skips."""
import glob
import os
import pty
import re
import select
import shutil
import socket
import stat
import subprocess
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
PKG = os.path.join(REPO, "gentoo", "app-misc", "horcrux-tmux-config")
HAVE = os.path.isdir(PKG)
CONF = os.path.join(PKG, "files", "tmux.conf")
HELPER = os.path.join(PKG, "files", "horcrux-clip-copy")
TMUX = shutil.which("tmux")


def newest(pattern):
    files = sorted(glob.glob(os.path.join(REPO, "gentoo", pattern)), key=lambda p: int((re.search(r"-r(\d+)\.ebuild$", p) or [0, 0])[1]))
    return files[-1]


def rdepend(path):
    return re.search(r'^RDEPEND="(.*?)"', open(path).read(), re.S | re.M).group(1).split()


@unittest.skipUnless(HAVE, "needs the Horcrux repo tree")
class Static(unittest.TestCase):
    def test_desktop_metas_pull_it_server_core_does_not(self):
        atom = "app-misc/horcrux-tmux-config"
        self.assertIn(atom, rdepend(newest("gnome-extra/horcrux-gnome-defaults/*.ebuild")))
        self.assertIn(atom, rdepend(newest("app-misc/horcrux-surface-desktop/*.ebuild")))
        self.assertNotIn(atom, " ".join(rdepend(newest("sys-apps/horcrux-core/*.ebuild"))))

    def test_package_depends_on_both_clipboard_tools(self):
        dep = rdepend(newest("app-misc/horcrux-tmux-config/*.ebuild"))
        for a in ("gui-apps/wl-clipboard", "x11-misc/xclip", "app-misc/tmux"):
            self.assertIn(a, dep)

    def test_conf_does_not_force_mouse_and_keeps_osc52(self):
        txt = open(CONF).read()
        self.assertNotRegex(re.sub(r"(?m)^#.*$", "", txt), r"\bmouse\b")
        self.assertRegex(txt, r"(?m)^set -g set-clipboard external$")
        self.assertIn("copy-pipe-no-clear", txt)

    def test_helper_is_executable_posix_sh(self):
        self.assertTrue(os.stat(HELPER).st_mode & stat.S_IXUSR)
        self.assertEqual(open(HELPER).readline().strip(), "#!/bin/sh")
        subprocess.run(["sh", "-n", HELPER], check=True)


def sgr(btn, x, y, press=True):
    return ("\x1b[<%d;%d;%d%s" % (btn, x, y, "M" if press else "m")).encode()


@unittest.skipUnless(HAVE and TMUX, "needs the Horcrux repo tree and tmux")
class RealTmux(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp(prefix="hx-tmuxclip-")
        self.bin = os.path.join(self.d, "bin")
        os.mkdir(self.bin)
        self.out = os.path.join(self.d, "clip.out")
        for name, args in (("wl-copy", ""), ("xclip", "")):
            p = os.path.join(self.bin, name)
            open(p, "w").write('#!/bin/sh\necho %s >> "%s.who"\ncat > "%s"\n' % (name, self.out, self.out))
            os.chmod(p, 0o755)
        self.rt = os.path.join(self.d, "rt")
        os.mkdir(self.rt, 0o700)
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.bind(os.path.join(self.rt, "wayland-test"))
        self.conf = os.path.join(self.d, "tmux.conf")
        txt = open(CONF).read().replace("/usr/bin/horcrux-clip-copy", HELPER)
        open(self.conf, "w").write(txt)
        self.env = dict(os.environ, TERM="xterm-256color", PATH=self.bin + ":" + os.environ["PATH"], XDG_RUNTIME_DIR=self.rt,
                        WAYLAND_DISPLAY="wayland-test", TMUX_TMPDIR=self.d, HOME=self.d, LANG="C.UTF-8")
        self.env.pop("TMUX", None)
        self.env.pop("DISPLAY", None)
        self.sockname = "hx"

    def tearDown(self):
        subprocess.run([TMUX, "-L", self.sockname, "kill-server"], env=self.env, capture_output=True)
        self.sock.close()
        shutil.rmtree(self.d, ignore_errors=True)

    def t(self, *a):
        return subprocess.run([TMUX, "-L", self.sockname, "-f", self.conf, *a], env=self.env, capture_output=True, text=True)

    def start(self, mouse):
        self.t("new-session", "-d", "-x", "80", "-y", "24", "-s", "s", "sh -c 'echo hello-clipboard-world; exec cat'")
        if mouse is not None:
            self.t("set", "-g", "mouse", mouse)
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe(TMUX, [TMUX, "-L", self.sockname, "-f", self.conf, "attach", "-t", "s"], self.env)
        self.fd, self.pid = fd, pid
        self.drain(1.0)
        time.sleep(0.5)

    def drain(self, secs):
        end = time.time() + secs
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.1)
            if r:
                try:
                    os.read(self.fd, 65536)
                except OSError:
                    return

    def drag(self):
        # first row holds "hello-clipboard-world": press at col 1, drag to col 21, release
        for seq in (sgr(0, 1, 1), sgr(32, 8, 1), sgr(32, 22, 1), sgr(0, 22, 1, press=False)):
            os.write(self.fd, seq)
            self.drain(0.25)
        self.drain(0.8)

    def test_defaults_to_wayland_tool_and_keeps_selection_with_mouse_on(self):
        self.start("on")
        self.drag()
        self.assertTrue(os.path.exists(self.out), "nothing reached the clipboard tool")
        self.assertEqual(open(self.out).read().strip(), "hello-clipboard-world")
        self.assertEqual(open(self.out + ".who").read().split(), ["wl-copy"])
        # the highlight must survive the release (copy-pipe-no-clear): the pane is still in copy mode with a selection
        self.assertEqual(self.t("display", "-p", "-t", "s", "#{pane_in_mode}#{selection_present}").stdout.strip(), "11")

    def test_x11_session_uses_xclip(self):
        self.env["DISPLAY"] = ":99"
        self.env.pop("WAYLAND_DISPLAY")
        self.start("on")
        self.drag()
        self.assertEqual(open(self.out).read().strip(), "hello-clipboard-world")
        self.assertEqual(open(self.out + ".who").read().split(), ["xclip"])

    def test_no_display_is_silent_noop(self):
        self.env.pop("WAYLAND_DISPLAY")
        self.start("on")
        self.drag()
        self.assertFalse(os.path.exists(self.out))
        r = subprocess.run(["sh", HELPER], input=b"x", env=self.env, capture_output=True)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, b"", b""))

    def test_mouse_off_is_unaffected(self):
        self.start("off")
        self.assertEqual(self.t("show", "-gv", "mouse").stdout.strip(), "off")
        self.drag()
        self.assertFalse(os.path.exists(self.out))

    def test_conf_never_changes_mouse(self):
        self.start(None)
        self.assertEqual(self.t("show", "-gv", "mouse").stdout.strip(), "off")


if __name__ == "__main__":
    unittest.main()
