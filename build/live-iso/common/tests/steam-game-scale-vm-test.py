#!/usr/bin/env python3
"""Steam game scaling VM test: a fixed-size X11 "game" window launched the way Steam launches a game must cover the size the scaled
desktop implies when it runs through horcrux-game-scale (games-util/horcrux-game-scale), and must NOT without it (control).

Why: mutter >= 50 always presents Xwayland surfaces at buffer scale ceil(monitor scale). X11 desktop apps learn the factor from
XSETTINGS (gnome-session-openrc's gsd-xsettings); games draw their own surface and never do, so on a 133 % desktop a windowed
1280x720 game covers 853x480 panel pixels (owner-ws 2026-11-23: GDK_SCALE fixed Steam's own window, not the game). The fix runs
the game in a nested, fullscreen gamescope that mutter sizes to the monitor.

Attach mode, same interface as gnome-session-lifecycle-test.py (it reuses that test's VM/QMP/login helpers): the VM runs an installed
GNOME edition with games-util/horcrux-game-scale and gui-wm/gamescope installed and a Vulkan driver gamescope can use (a VM without a
GPU needs mesa VIDEO_CARDS=lavapipe), a QMP socket (screendumps are read from the QEMU side's filesystem: run this test where QEMU
runs, or pass --vm-out), the desktop user listed at the greeter. Per scale in 1.25/1.333/1.5/2.0 (mutter picks the closest scale it
supports for the mode):

  * horcrux-steam-launch-options (the real tool) stamps a synthetic Steam library's localconfig.vdf; the stamped LaunchOptions are
    expanded the way Steam does (leading VAR=value words -> environment, %command% -> the compatibility-tool chain) inside
    reaper SteamLaunch AppId=N -- steam-launch-wrapper -- ... with a Steam Linux Runtime entry point and a "proton" stand-in
    (no Steam login exists for VMs: these are shell stand-ins with Steam's real argument shape; Proton itself is not run);
  * control: the same chain without the wrapper -> the game's window covers ~1/ceil(scale) of the expected size;
  * wrapped: the game's pixels cover at least logical size x scale (1280x720 x 1.333 = 1707x960 physical) and, gamescope
    being fullscreen, fill the panel's width (aspect-fit);
  * horcrux-game-scale --explain reports the monitor's physical mode from mutter (not a hard-coded size), and at scale 1 it runs
    the game directly.

    steam-game-scale-vm-test.py --qmp /w/qmp.sock --port 22790 --key k [--user owner] [--out DIR] [--scales 1.25,1.333,1.5,2.0]

Exit 0 = PASS, 1 = FAIL. JSON summary in OUT/result.json, screendumps (PPM) of every measurement in OUT/.
"""
import argparse
import importlib.machinery
import importlib.util
import json
import math
import os
import re
import shlex
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_l = importlib.machinery.SourceFileLoader("gsl", os.path.join(HERE, "gnome-session-lifecycle-test.py"))
_s = importlib.util.spec_from_loader("gsl", _l)
gsl = importlib.util.module_from_spec(_s)
_l.exec_module(gsl)
shq = shlex.quote

GAME_W, GAME_H = 1280, 720
READY_RE = re.compile(r"^(\S+) (\d+) root=(\d+)x(\d+) win=(-?\d+),(-?\d+)\+(\d+)x(\d+)$")
MAGENTA = (255, 0, 255)
APPID = "4242420"

# the "game": a plain X11 window of a fixed size in X pixels, one solid colour, no decorations - what a game's window is
# (GDK_SCALE/GDK_DPI_SCALE pinned to 1 so XSETTINGS cannot resize it, as no game reads XSETTINGS)
GAME_PY = r'''
import os, sys, gi
os.environ["GDK_BACKEND"] = "x11"; os.environ["GDK_SCALE"] = "1"; os.environ["GDK_DPI_SCALE"] = "1"
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GLib
w = Gtk.Window(title="hx-scale-game")
w.set_decorated(False)
w.set_default_size(%d, %d)
w.set_size_request(%d, %d)
w.set_resizable(False)
css = Gtk.CssProvider(); css.load_from_data(b"window { background-color: #ff00ff; }")
Gtk.StyleContext.add_provider_for_screen(Gdk.Screen.get_default(), css, 800)
w.connect("destroy", Gtk.main_quit)
w.show_all()


def report():
    # what the game sees, in X pixels: the X screen (root window) and its own window
    gw, root = w.get_window(), w.get_screen().get_root_window()
    _r, ox, oy = gw.get_origin()
    g = gw.get_geometry()
    with open(os.environ.get("HX_GAME_READY", "/dev/null"), "w") as f:
        f.write("%%s %%s root=%%dx%%d win=%%d,%%d+%%dx%%d\n" %% (os.environ.get("DISPLAY"), os.getpid(), root.get_width(), root.get_height(),
                                                            ox, oy, g.width, g.height))
    return False


GLib.timeout_add(1500, report)
Gtk.main()
''' % (GAME_W, GAME_H, GAME_W, GAME_H)

# Steam's launch chain, as stand-ins with Steam's argument shape (ubuntu12_32/reaper, steam-launch-wrapper, the Steam Linux Runtime
# entry point, Proton's `proton VERB EXE`)
STANDINS = {
    "ubuntu12_32/reaper": 'while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done; shift; exec "$@"',
    "ubuntu12_32/steam-launch-wrapper": '[ "$1" = -- ] && shift; exec "$@"',
    "steamapps/common/SteamLinuxRuntime_sniper/_v2-entry-point": 'while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done; shift; exec "$@"',
    "steamapps/common/Proton - Experimental/proton": 'verb=$1; shift; exec "$@"',
}


def vnc_ppm(addr, path, timeout=30):
    """Full-frame capture from QEMU's VNC server (RFB 3.8, no auth, raw encoding) into a binary PPM. Needed with a GL virtual
    GPU (virtio-vga-gl + egl-headless): QMP screendump answers "no surface" for a GL scanout, VNC gets the read-back frame."""
    import socket
    import struct
    host, port = addr.rsplit(":", 1)
    s = socket.create_connection((host, int(port)), timeout=timeout)

    def rd(n):
        b = b""
        while len(b) < n:
            c = s.recv(n - len(b))
            if not c:
                raise OSError("vnc: connection closed")
            b += c
        return b
    rd(12)
    s.sendall(b"RFB 003.008\n")
    n = rd(1)[0]
    types = rd(n)
    if 1 not in types:
        raise OSError(f"vnc: no 'None' security type offered: {list(types)}")
    s.sendall(b"\x01")
    if struct.unpack(">I", rd(4))[0] != 0:
        raise OSError("vnc: security handshake failed")
    s.sendall(b"\x01")  # shared
    w, h = struct.unpack(">HH", rd(4))
    rd(16)
    rd(struct.unpack(">I", rd(4))[0])
    # 32 bpp, depth 24, little endian, true colour, r/g/b max 255 at shifts 16/8/0
    s.sendall(struct.pack(">BxxxBBBBHHHBBBxxx", 0, 32, 24, 0, 1, 255, 255, 255, 16, 8, 0))
    s.sendall(struct.pack(">BxHi", 2, 1, 0))  # encodings: raw
    s.sendall(struct.pack(">BBHHHH", 3, 0, 0, 0, w, h))
    img = bytearray(w * h * 3)
    covered = 0
    while covered < w * h:
        t = rd(1)[0]
        if t != 0:  # bell / cut text / colour map: skip what we can
            if t == 2:
                continue
            if t == 3:
                rd(3)
                rd(struct.unpack(">I", rd(4))[0])
                continue
            raise OSError(f"vnc: unexpected message {t}")
        rd(1)
        for _ in range(struct.unpack(">H", rd(2))[0]):
            x, y, rw, rh, enc = struct.unpack(">HHHHi", rd(12))
            if enc != 0:
                raise OSError(f"vnc: unexpected encoding {enc}")
            data = rd(rw * rh * 4)
            for r in range(rh):
                row = data[r * rw * 4:(r + 1) * rw * 4]
                o = ((y + r) * w + x) * 3
                img[o:o + rw * 3:3] = row[2::4]
                img[o + 1:o + rw * 3:3] = row[1::4]
                img[o + 2:o + rw * 3:3] = row[0::4]
            covered += rw * rh
    s.close()
    with open(path, "wb") as f:
        f.write(b"P6\n%d %d\n255\n" % (w, h) + bytes(img))


def ppm_bbox(path, rgb, tol=40):
    """(x0, y0, x1, y1, count) of the pixels within tol of rgb in a binary PPM (P6), or None."""
    with open(path, "rb") as f:
        data = f.read()
    parts, pos = [], 0
    while len(parts) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        if data[pos:pos + 1] == b"#":
            pos = data.index(b"\n", pos) + 1
            continue
        end = pos
        while not data[end:end + 1].isspace():
            end += 1
        parts.append(data[pos:end])
        pos = end
    pos += 1
    w, h = int(parts[1]), int(parts[2])
    px = data[pos:pos + w * h * 3]
    r0, g0, b0 = rgb
    xs, ys, n = [], [], 0
    for y in range(0, h, 2):  # every 2nd row/column: plenty for a bounding box, 4x faster
        row = px[y * w * 3:(y + 1) * w * 3]
        hit = [x for x in range(0, w, 2) if abs(row[3 * x] - r0) <= tol and abs(row[3 * x + 1] - g0) <= tol and abs(row[3 * x + 2] - b0) <= tol]
        if hit:
            xs += (hit[0], hit[-1])
            ys.append(y)
            n += len(hit)
    if not ys:
        return None
    return min(xs), min(ys), max(xs) + 1, max(ys) + 1, n * 4, (w, h)


def wl_presented(log_text, buf_w, buf_h):
    """From a WAYLAND_DEBUG=client log of gamescope's wayland backend: (fullscreen logical size from mutter's last non-zero
    xdg_toplevel.configure, preferred fractional scale, [(dest_w, dest_h)] of every wp_viewport whose source is the game's
    buf_w x buf_h buffer) - the size mutter is asked to present the game at, in logical px of the monitor."""
    conf = re.findall(r"xdg_toplevel#\d+\.configure\((\d+), (\d+), array", log_text)
    conf = [(int(a), int(b)) for a, b in conf if int(a) > 0]
    sc = re.findall(r"wp_fractional_scale_v1#\d+\.preferred_scale\((\d+)\)", log_text)
    vp = {}
    dests = []
    for m in re.finditer(r"-> wp_viewport#(\d+)\.set_(source|destination)\(([^)]*)\)", log_text):
        vid, kind, args = m.group(1), m.group(2), [float(x) for x in m.group(3).split(",")]
        if kind == "source":
            vp[vid] = (round(args[2]), round(args[3]))
        elif vp.get(vid) == (buf_w, buf_h):
            dests.append((round(args[0]), round(args[1])))
    return (conf[-1] if conf else None), (int(sc[-1]) / 120 if sc else None), dests


class Run(gsl.Run):
    # method 1 = temporary: gnome-shell asks "Keep these display settings?" for a persistent (2) change and reverts it after
    # ~20 s, which is shorter than one control + wrapped measurement
    SCALE_PROBE = gsl.Run.SCALE_PROBE.replace("(serial, 2, lm_out, {})", "(serial, 1, lm_out, {})")
    assert SCALE_PROBE != gsl.Run.SCALE_PROBE
    _cur = 'if mp.get("is-current"):'
    assert _cur in SCALE_PROBE and "want = float(sys.argv[1])" in SCALE_PROBE
    SCALE_PROBE = SCALE_PROBE.replace("want = float(sys.argv[1])", "want = float(sys.argv[1]); want_mode = sys.argv[2] if len(sys.argv) > 2 else ''")
    SCALE_PROBE = SCALE_PROBE.replace(_cur, 'if ((f"{w}x{h}" == want_mode and (conn not in mode_of or mp.get("is-current"))) if want_mode '
                                            'else mp.get("is-current")):')
    def __init__(self, a):
        super().__init__(a)
        self.cycle_no = 1

    def env_line(self):
        uid = self.uid
        disp = self.disp
        auth = self.vm.out(f"tr '\\0' '\\n' </proc/$(pgrep -u {uid} -x Xwayland | head -n1)/cmdline | sed -n '/^-auth$/{{n;p;q}}'")
        return (f"XDG_RUNTIME_DIR=/run/user/{uid} DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus WAYLAND_DISPLAY=wayland-0 "
                f"XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=GNOME DISPLAY={disp} XAUTHORITY={auth}"
                + "".join(" " + shq(e) for e in self.a.env))

    def xdisplay(self):
        """the session's Xwayland display; mutter starts Xwayland on demand, so start an X11 client first when it isn't up"""
        hp = "/usr/libexec/gnome-session-openrc-helper"
        d = self.as_user(f"{hp} x-display 2>/dev/null") if self.vm.sh(f"test -x {hp}")[0] == 0 else ""
        if not d.startswith(":"):
            self.as_user("WAYLAND_DISPLAY=wayland-0 GDK_BACKEND=x11 timeout 5 python3 -c 'import gi; gi.require_version(\"Gtk\",\"3.0\"); "
                         "from gi.repository import Gtk' >/dev/null 2>&1; true")
            pid = self.vm.out(f"pgrep -u {self.uid} -x Xwayland | head -n1")
            d = self.vm.out(f"tr '\\0' '\\n' </proc/{pid}/cmdline | grep -m1 '^:[0-9]'") if pid else ""
        return d.strip()

    def steam_tree(self):
        """a synthetic Steam installation of the user: stand-ins, one installed game, its localconfig.vdf; then the REAL
        horcrux-steam-launch-options stamps it"""
        vm, u = self.vm, self.a.user
        root = f"~{u}/.local/share/Steam"
        script = [f"set -e; R={root}; mkdir -p $R/userdata/7/config $R/steamapps/common/HxScaleGame $R/ubuntu12_32 "
                  f"$R/steamapps/common/SteamLinuxRuntime_sniper \"$R/steamapps/common/Proton - Experimental\""]
        for rel, body in STANDINS.items():
            script.append(f"printf '%s\\n' '#!/bin/sh' {shq(body)} > \"$R/{rel}\"; chmod 755 \"$R/{rel}\"")
        script.append(f"printf '%s' {shq(GAME_PY)} > $R/steamapps/common/HxScaleGame/game.py")
        script.append("printf '%s\\n' '#!/bin/sh' 'exec python3 \"$(dirname \"$0\")/game.py\"' > $R/steamapps/common/HxScaleGame/game.exe; "
                      "chmod 755 $R/steamapps/common/HxScaleGame/game.exe")
        script.append("printf '\"manifest\" { \"commandline\" \"/proton %%verb%%\" }' > \"$R/steamapps/common/Proton - Experimental/toolmanifest.vdf\"")
        acf = f'"AppState"\n{{\n\t"appid"\t\t"{APPID}"\n\t"name"\t\t"Hx Scale Game"\n\t"installdir"\t\t"HxScaleGame"\n}}\n'
        script.append(f"printf '%s' {shq(acf)} > $R/steamapps/appmanifest_{APPID}.acf")
        lc = ('"UserLocalConfigStore"\n{\n\t"Software"\n\t{\n\t\t"Valve"\n\t\t{\n\t\t\t"Steam"\n\t\t\t{\n\t\t\t\t"apps"\n\t\t\t\t{\n\t\t\t\t\t"'
              + APPID + '"\n\t\t\t\t\t{\n\t\t\t\t\t\t"LaunchOptions"\t\t"HX_TEST_MARK=1 %command%"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n')
        script.append(f"printf '%s' {shq(lc)} > $R/userdata/7/config/localconfig.vdf; chown -R {u}: ~{u}/.local")
        rc, o = vm.sh("\n".join(script))
        if not self.check(0, "steam-library-staged", rc == 0, o[-300:]):
            return None
        out = self.as_user("horcrux-steam-launch-options apply 2>&1; horcrux-steam-launch-options status 2>&1")
        opt = vm.out(f"grep -o '\"LaunchOptions\"[[:space:]]*\"[^\"]*\"' {root}/userdata/7/config/localconfig.vdf | sed 's/.*\"\\([^\"]*\\)\"$/\\1/'")
        self.check(0, "launch-options-stamped", opt == "HX_TEST_MARK=1 horcrux-game-scale %command%", f"LaunchOptions={opt!r}; tool: {out[-200:]!r}")
        return opt

    def chain(self, launch_option, ready):
        """the command Steam runs for a Proton game with this launch option, as one shell line"""
        u = self.a.user
        root = f"/home/{u}/.local/share/Steam"
        compat = [f"{root}/steamapps/common/SteamLinuxRuntime_sniper/_v2-entry-point", "--verb=waitforexitandrun", "--",
                  f"{root}/steamapps/common/Proton - Experimental/proton", "waitforexitandrun", f"{root}/steamapps/common/HxScaleGame/game.exe"]
        words = shlex.split(launch_option)
        env = []
        while words and "=" in words[0] and not words[0].startswith("%"):
            env.append(words.pop(0))
        i = words.index("%command%")
        argv = [f"{root}/ubuntu12_32/reaper", "SteamLaunch", f"AppId={APPID}", "--", f"{root}/ubuntu12_32/steam-launch-wrapper", "--"] + \
            words[:i] + compat + words[i + 1:]
        env += [f"SteamAppId={APPID}", f"SteamGameId={APPID}", f"STEAM_COMPAT_DATA_PATH={root}/steamapps/compatdata/{APPID}", f"HX_GAME_READY={ready}"]
        return " ".join(shq(e) for e in env) + " " + " ".join(shq(x) for x in argv)

    def launch(self, line, tag):
        log = f"/tmp/hx-scale-{tag}.log"
        self.vm.sh(f"su - {self.a.user} -c {shq('setsid env ' + self.env_line() + ' ' + line + f' >{log} 2>&1 </dev/null &')}", timeout=30)
        return log

    def stop_game(self):
        self.vm.sh(f"pkill -u {self.uid} -f 'HxScaleGame/[g]ame.py'; sleep 2; pkill -u {self.uid} -x gamescope; "
                   f"pkill -u {self.uid} -f '[g]amescope-wl'; sleep 2; pkill -9 -u {self.uid} -x gamescope; true")

    def measure(self, tag):
        self.vm.key("shift")  # keep the output awake
        time.sleep(0.5)
        path = os.path.join(self.a.vm_out or self.a.out, f"{tag}.ppm")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        if self.a.vnc:
            vnc_ppm(self.a.vnc, os.path.join(self.a.out, f"{tag}.ppm"))
        else:
            self.vm.qmp("screendump", filename=path)
            time.sleep(1)
        return ppm_bbox(os.path.join(self.a.out, f"{tag}.ppm"), MAGENTA)

    def ready(self, path, pred, timeout=45):
        """the game's own report (display, X screen size, window geometry) once it satisfies pred"""
        end, got = time.time() + timeout, None
        while time.time() < end:
            m = READY_RE.match(self.vm.out(f"cat {path} 2>/dev/null").strip())
            if m:
                got = (m.group(1),) + tuple(int(x) for x in m.groups()[1:])
                if pred(got):
                    return got
            time.sleep(2)
        return got

    def wait_bbox(self, tag, pred, timeout=45):
        end, bb = time.time() + timeout, None
        while time.time() < end:
            bb = self.measure(tag)
            if bb and pred(bb):
                return bb
            time.sleep(3)
        return bb

    def main(self):
        a = self.a
        self.held = []
        self.setup()
        vm = self.vm
        vers = vm.out("qlist -Iv games-util/horcrux-game-scale gui-wm/gamescope x11-wm/mutter media-libs/mesa 2>/dev/null")
        print("versions:\n  " + vers.replace("\n", "\n  "), flush=True)
        if not self.check(0, "packages-installed", "horcrux-game-scale" in vers and "gamescope" in vers, vers.replace("\n", ", ")):
            return self.finish()
        # an earlier test of the same gate may have left the user logged in at the greeter: use that session
        act = gsl.active_session(vm)
        sid = next((s[0] for s in gsl.seat_sessions(vm, self.uid) if s[0] == act and s[1] == "active" and s[2] == "user"), None)
        if sid:
            print(f"  using the user's active session {sid}", flush=True)
        else:
            sid = self.login(1)
            if not sid:
                return self.finish()
            time.sleep(20)
        # no screen blanking during the measurements (a blanked virtio-vga output dumps as "Display output is not active")
        self.as_user("gsettings set org.gnome.desktop.session idle-delay 0; gsettings set org.gnome.settings-daemon.plugins.power "
                     "idle-dim false; gsettings set org.gnome.desktop.screensaver lock-enabled false")
        vm.key("shift")
        time.sleep(2)
        vm.key("esc")  # leave the overview GNOME opens at login
        time.sleep(2)
        venv = " ".join(shq(e) for e in a.env)
        vk = vm.out(f"env {venv} vulkaninfo --summary 2>/dev/null | grep -E 'deviceName|driverName' | head -4")
        self.check(0, "vulkan-device", "deviceName" in vk, vk.replace("\n", "; ")[:200])
        self.disp = self.xdisplay()
        if not self.check(0, "xwayland-display", self.disp.startswith(":"), f"Xwayland display {self.disp!r}"):
            return self.finish()
        vm.sh("cat > /tmp/gsl-scale-probe.py; chmod 644 /tmp/gsl-scale-probe.py", inp=self.SCALE_PROBE)
        opt = self.steam_tree()
        if not opt:
            return self.finish()
        orig = None
        try:
            base_mode = ""  # the monitor's mode before the test: scales without @WxH use it, and it is restored at the end
            for want, want_mode in a.scales:
                o = self.as_user(f"python3 /tmp/gsl-scale-probe.py {want} {want_mode or base_mode}")
                try:
                    st = json.loads(o.strip().splitlines()[-1])
                except (ValueError, IndexError):
                    self.check(0, f"scale-{want}-apply", False, f"ApplyMonitorsConfig failed: {o[-300:]}")
                    continue
                orig = st["was"] if orig is None else orig
                if not base_mode and not want_mode:
                    base_mode = "%dx%d" % tuple(st["mode"])
                s = st["applied"][0]
                pw, ph = st["mode"]
                f = math.ceil(s - 1e-6)
                exp_w, exp_h = GAME_W * s, GAME_H * s  # the size the scaled desktop implies for a 1280x720 window
                time.sleep(5)
                ex = self.as_user(f"{self.env_line()} SteamAppId={APPID} horcrux-game-scale --explain -- true")
                try:
                    exj = json.loads(ex[ex.index("{"):])
                    argv = exj["argv"]
                    got = (argv[argv.index("-W") + 1], argv[argv.index("-H") + 1], argv[argv.index("-w") + 1], argv[argv.index("-h") + 1])
                    ok = exj["wrap"] and got == (str(pw), str(ph), str(pw), str(ph))
                except (ValueError, KeyError, IndexError):
                    ok, got = False, ex[-200:]
                self.check(0, f"scale-{want}-explain-uses-mutter-mode", ok, f"scale {s:.4f}, mode {pw}x{ph}: gamescope -W -H -w -h = {got}")
                # control: Steam's chain without the wrapper
                self.stop_game()
                self.vm.sh("rm -f /tmp/hx-ready-ctl /tmp/hx-ready-wrap")
                line = self.chain("HX_TEST_MARK=1 %command%", "/tmp/hx-ready-ctl")
                self.launch(line, f"ctl-{want}")
                if a.measure == "protocol":
                    # the game's X window in mutter's Xwayland, whose X screen is ceil(scale) x the logical size: it covers
                    # win * panel / X screen physical px
                    r = self.ready("/tmp/hx-ready-ctl", lambda g: g[3] > 0)
                    bb = (0, 0, round(r[6] * pw / r[2]), round(r[7] * ph / r[3]), 1) if r and r[2] else None
                    print(f"  control: game reports {r}; mutter X screen {r[2] if r else '?'}x{r[3] if r else '?'} for a {pw}x{ph} panel", flush=True)
                else:
                    bb = self.wait_bbox(f"scale-{want}-control", lambda b: b[4] > 1000, 40)
                cw, ch = (bb[2] - bb[0], bb[3] - bb[1]) if bb else (0, 0)
                small_w, small_h = GAME_W / f * s, GAME_H / f * s
                self.check(0, f"scale-{want}-control-without-wrapper-is-small",
                           bb is not None and cw < exp_w * 0.9 and abs(cw - small_w) <= 0.06 * small_w + 6 and abs(ch - small_h) <= 0.06 * small_h + 6,
                           f"game covers {cw}x{ch} px (expected without the fix ~{small_w:.0f}x{small_h:.0f} = 1/{f} of {exp_w:.0f}x{exp_h:.0f})")
                self.stop_game()
                time.sleep(2)
                # wrapped: the stamped launch option, expanded like Steam
                line = self.chain(("WAYLAND_DEBUG=client " if a.measure == "protocol" else "") + opt, "/tmp/hx-ready-wrap")
                log = self.launch(line, f"wrap-{want}")
                if a.measure == "protocol":
                    # inside gamescope the game sees an X screen of the panel's physical size; gamescope (a Wayland client of
                    # mutter) asks mutter to present the game's buffer at a viewport destination in the monitor's logical px
                    r = self.ready("/tmp/hx-ready-wrap", lambda g: g[0] != self.disp, 60)
                    time.sleep(8)  # gamescope must still be up (no fallback) after the game showed
                    alive = vm.out(f"pgrep -u {self.uid} -f '^/[^ ]*gamescope --backend' | head -n1")  # comm is gamescope-wl
                    conf, fs, dests = wl_presented(vm.out(f"grep -a -E 'configure\\(|preferred_scale|wp_viewport' {log}"), GAME_W, GAME_H)
                    print(f"  wrapped: game reports {r}; gamescope pid {alive or '-'}; mutter configure {conf}, preferred scale {fs}, "
                          f"game viewport destinations {dests[-3:]}", flush=True)
                    self.check(0, f"scale-{want}-wrapped-game-inside-gamescope",
                               bool(r) and r[0] != self.disp and (r[2], r[3]) == (pw, ph) and bool(alive),
                               f"game on X display {r[0] if r else '?'} (session Xwayland {self.disp}), X screen "
                               f"{r[2] if r else '?'}x{r[3] if r else '?'} (panel {pw}x{ph}), gamescope running: {bool(alive)}")
                    self.check(0, f"scale-{want}-wrapped-gamescope-fullscreen-on-monitor",
                               conf is not None and fs is not None and abs(conf[0] * fs - pw) <= 2 and abs(conf[1] * fs - ph) <= 2,
                               f"mutter sized gamescope {conf} logical px at preferred scale {fs} = "
                               f"{(round(conf[0] * fs), round(conf[1] * fs)) if conf and fs else '?'} physical (panel {pw}x{ph})")
                    bb = (0, 0, round(dests[-1][0] * s), round(dests[-1][1] * s), 1) if dests else None
                else:
                    bb = self.wait_bbox(f"scale-{want}-wrapped", lambda b: b[2] - b[0] >= exp_w, 60)
                ww, wh = (bb[2] - bb[0], bb[3] - bb[1]) if bb else (0, 0)
                fit = min(pw / GAME_W, ph / GAME_H)
                gsps = vm.out(f"pgrep -u {self.uid} -a gamescope | head -2")
                self.check(0, f"scale-{want}-wrapped-covers-expected-size",
                           bb is not None and ww >= exp_w - 4 and wh >= exp_h - 4,
                           f"game covers {ww}x{wh} px, needs >= {exp_w:.0f}x{exp_h:.0f}; gamescope: {gsps[:160]!r}")
                self.check(0, f"scale-{want}-wrapped-fills-monitor-aspect-fit",
                           bb is not None and abs(ww - GAME_W * fit) <= 8 and abs(wh - GAME_H * fit) <= 8,
                           f"{ww}x{wh} vs aspect-fit {GAME_W * fit:.0f}x{GAME_H * fit:.0f} of the {pw}x{ph} panel")
                if not bb or ww < exp_w - 4:
                    print("  wrapper log:\n    " + vm.out(f"tail -n 30 {log}; tail -n 5 ~{a.user}/.local/state/horcrux/game-scale.log").replace("\n", "\n    "), flush=True)
                self.stop_game()
            # scale 1: nothing to fix, the game runs directly
            o = self.as_user(f"python3 /tmp/gsl-scale-probe.py 1.0 {base_mode}")
            time.sleep(4)
            ex = self.as_user(f"{self.env_line()} horcrux-game-scale --explain -- true")
            try:
                exj = json.loads(ex[ex.index("{"):])
                self.check(0, "scale-1-runs-game-directly", not exj["wrap"] and exj["argv"] == ["true"], exj["reason"])
            except ValueError:
                self.check(0, "scale-1-runs-game-directly", False, ex[-200:])
        finally:
            self.stop_game()
            if orig is not None:
                self.as_user(f"python3 /tmp/gsl-scale-probe.py {orig} {base_mode}")
        return self.finish()

    def finish(self):
        os.makedirs(self.a.out, exist_ok=True)
        summary = {"result": "FAIL" if self.failed else "PASS", "checks": self.results}
        with open(os.path.join(self.a.out, "result.json"), "w") as f:
            json.dump(summary, f, indent=1)
        nfail = sum(not r["ok"] for r in self.results)
        print(f"RESULT {summary['result']}: {len(self.results) - nfail} passed, {nfail} failed", flush=True)
        return 1 if self.failed or not self.results else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--qmp", required=True)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=22)
    ap.add_argument("--key", required=True)
    ap.add_argument("--user", default="owner")
    ap.add_argument("--out", default="gss-out")
    ap.add_argument("--vm-out", default=None)
    ap.add_argument("--scales", default="1.25,1.333,1.5@1920x1200,2.0",
                    help="comma list; SCALE@WxH also switches the monitor to that mode (mutter offers 1.5 only where the logical "
                         "size stays whole)")
    ap.add_argument("--vnc", default=None, metavar="HOST:PORT",
                    help="capture frames from QEMU's VNC server instead of QMP screendump (needed with a GL virtual GPU)")
    ap.add_argument("--env", action="append", default=[], metavar="VAR=VALUE",
                    help="extra environment for the game launch, e.g. VK_DRIVER_FILES=... to pick the guest's Vulkan driver")
    ap.add_argument("--measure", choices=("pixels", "protocol"), default="pixels",
                    help="pixels: screendump/VNC bounding box of the game's colour; protocol: the game's own X geometry plus the "
                         "size gamescope asks mutter to present it at (WAYLAND_DEBUG of gamescope's wayland backend), for virtual GPUs "
                         "whose frames cannot be captured (virgl/venus on a host without a GPU)")
    ap.add_argument("--cycles", type=int, default=1, help="ignored (gnome-vm-gate passes it to every test)")
    a = ap.parse_args()
    a.scales = [(float(x.split("@")[0]), (x.split("@") + [""])[1]) for x in a.scales.split(",") if x]
    a.out = os.path.abspath(a.out)
    return Run(a).main()


if __name__ == "__main__":
    sys.exit(main())
