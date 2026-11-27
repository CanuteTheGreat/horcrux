#!/bin/bash
# Runs inside a throwaway ubuntu container: tmux 3.8 (built from the release tarball) + real wl-clipboard + weston headless (a real Wayland compositor with a clipboard).
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq sway weston wl-clipboard xclip xvfb python3 curl ca-certificates build-essential libevent-dev libncurses-dev bison pkg-config >/dev/null
curl -fsSL https://github.com/tmux/tmux/releases/download/3.6a/tmux-3.6a.tar.gz -o /tmp/t.tgz || true
TV=3.6a
if curl -fsSL -o /tmp/t8.tgz https://github.com/tmux/tmux/releases/download/3.8/tmux-3.8.tar.gz; then TV=3.8; mv /tmp/t8.tgz /tmp/t.tgz; fi
cd /tmp && tar xf t.tgz && cd tmux-$TV && ./configure -q && make -j4 >/dev/null && make install >/dev/null
tmux -V
useradd -m u; mkdir -p /run/u && chown u /run/u && chmod 700 /run/u
cp /w/gentoo/app-misc/horcrux-tmux-config/files/horcrux-clip-copy /usr/bin/horcrux-clip-copy; chmod 755 /usr/bin/horcrux-clip-copy
cp /w/gentoo/app-misc/horcrux-tmux-config/files/tmux.conf /etc/tmux.conf
cat >/tmp/run.sh <<'EOS'
set -e
export XDG_RUNTIME_DIR=/run/u
export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
sway >/tmp/sway.log 2>&1 &
for i in $(seq 75); do ls /run/u/wayland-* >/dev/null 2>&1 && break; sleep 0.2; done
export WAYLAND_DISPLAY=$(basename $(ls /run/u/wayland-*[0-9] | head -1))
echo compositor socket $WAYLAND_DISPLAY
echo hi | wl-copy; test "$(wl-paste)" = hi && echo "wl-copy/wl-paste round trip OK on real compositor"
cd /w/build/live-iso/common/tests
# the repo test, but with tmux.conf from /etc (installed path) and the REAL wl-copy: no stubs
python3 - <<'EOP'
import os, pty, select, subprocess, time
env = dict(os.environ, TERM="xterm-256color", TMUX_TMPDIR="/tmp", HOME="/home/u")
env.pop("TMUX", None)
def t(*a): return subprocess.run(["tmux","-L","p",*a],env=env,capture_output=True,text=True)
def attach_and_drag(mouse, label):
    t("kill-server")
    t("new-session","-d","-x","80","-y","24","-s","s","sh -c 'echo hello-clipboard-world; exec cat'")
    if mouse: t("set","-g","mouse",mouse)
    print(label, "set-clipboard=",t("show","-gv","set-clipboard").stdout.strip(),"copy-command=",t("show","-gv","copy-command").stdout.strip(),"mouse=",t("show","-gv","mouse").stdout.strip())
    subprocess.run(["wl-copy","--clear"],env=env)
    pid, fd = pty.fork()
    if pid == 0: os.execvpe("tmux",["tmux","-L","p","attach","-t","s"],env)
    def drain(s):
        e=time.time()+s
        while time.time()<e:
            if select.select([fd],[],[],0.1)[0]:
                try: os.read(fd,65536)
                except OSError: return
    drain(1.5)
    for seq in ("\x1b[<0;1;1M","\x1b[<32;8;1M","\x1b[<32;22;1M","\x1b[<0;22;1m"):
        os.write(fd,seq.encode()); drain(0.3)
    drain(1)
    r = subprocess.run(["wl-paste","-n"],env=env,capture_output=True,text=True)
    print(label, "wl-paste ->", repr(r.stdout), "| pane_in_mode/selection_present =", t("display","-p","-t","s","#{pane_in_mode}#{selection_present}").stdout.strip())
    return r.stdout
a = attach_and_drag("on","MOUSE ON :")
b = attach_and_drag("off","MOUSE OFF:")
t("kill-server")
assert a.strip()=="hello-clipboard-world", a
assert b.strip()=="", b
print("PROOF OK: mouse on -> clipboard has the selection; mouse off untouched")
EOP
EOS
su u -c 'bash /tmp/run.sh'
# no-display: helper is a silent no-op
env -u WAYLAND_DISPLAY -u DISPLAY sh -c 'echo x | /usr/bin/horcrux-clip-copy; echo "no-display exit=$?"'
# X11 session: xclip under Xvfb
su u -c 'Xvfb :7 & sleep 1; echo xdata | DISPLAY=:7 horcrux-clip-copy; sleep 0.5; DISPLAY=:7 xclip -o -selection clipboard; pkill Xvfb || true'
