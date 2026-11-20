#!/usr/bin/env python3
"""Host-side test harness for the Horcrux Windows guest agent.

It plays the node: listens on a TCP port, the agent connects (`horcrux-guest-agent.exe
--connect HOST:PORT`), and the harness speaks the guest-tools wire protocol
(crates/horcrux-guest-proto): Hello -> HostHello, StartVideo, Video packets, Input,
Clipboard, SetLayout/Layout. Standard library only; ffprobe/ffmpeg are used for the
bitstream checks when present (`analyze`).

  win_harness.py serve --port 9000 --out DIR [--www DIR] [--up DIR] [--tests a,b,c]
  win_harness.py analyze FILE.h264 --width W --height H [--min-frames N]
  win_harness.py analyze-audio audio.ogg [--tone-hz F]

Tests (`--tests`): hello, video, video-damage, input, clipboard, layout, video-native, audio,
audio-recover, virtual-display, video-virtual, gamepad. `audio` needs a render endpoint in the guest (e.g. QEMU ich9-intel-hda + hda-output and
the Windows Audio service running): it checks the 10 ms packet cadence while nothing plays, plays a
tone in the guest session (System.Media.SoundPlayer) and decodes the collected Opus packets (Ogg +
ffmpeg) to measure the tone's frequency and level; `audio-recover` stops and restarts the Windows
Audio service under a running stream. virtual-display, video-virtual and gamepad need the Horcrux
guest drivers: `--virtual-display` makes the HostHello ask for virtual monitors, as the node does when
the guest's GPU has no monitor or outputs; `--expect-gamepad` when hxpad is installed.
`input` and `clipboard` need a way to read guest state back. Probe scripts are written to
`--www` (served to the guest at http://10.0.2.2:8000/) and typed into the guest with the
agent's own keyboard injection (Win+R); they answer by setting the guest clipboard (which
the agent forwards) and by PUT to http://10.0.2.2:8001/<name> into `--up`.
Exit status 0 only if every selected test passed.
"""
import argparse, base64, json, os, queue, shutil, socket, struct, subprocess, sys, threading, time

K = dict(HELLO=1, HOST_HELLO=2, START_VIDEO=0x10, STOP_VIDEO=0x11, KEYFRAME=0x12, VIDEO=0x13,
         VIDEO_STATUS=0x14, CLIPBOARD=0x20, SET_LAYOUT=0x30, LAYOUT=0x31, INPUT=0x40, PING=0x60,
         PONG=0x61, LOG=0x7f, START_AUDIO=0x50, AUDIO=0x51, STOP_AUDIO=0x52)
KN = {v: k for k, v in K.items()}

# QEMU qnum (PC XT set 1; +0x80 = E0 extended) for a US layout.
SC = {}
for i, c in enumerate("1234567890"):
    SC[c] = 2 + i
for row, base in (("qwertyuiop", 0x10), ("asdfghjkl", 0x1e), ("zxcvbnm", 0x2c)):
    for i, c in enumerate(row):
        SC[c] = base + i
SC.update({"-": 0x0c, "=": 0x0d, "[": 0x1a, "]": 0x1b, ";": 0x27, "'": 0x28, "`": 0x29,
           "\\": 0x2b, ",": 0x33, ".": 0x34, "/": 0x35, " ": 0x39, "\n": 0x1c, "\t": 0x0f})
SHIFT = {"!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6", "&": "7", "*": "8", "(": "9",
         ")": "0", "_": "-", "+": "=", "{": "[", "}": "]", ":": ";", '"': "'", "|": "\\", "~": "`",
         "<": ",", ">": ".", "?": "/"}
LSHIFT, LCTRL, LALT, LWIN, ESC = 0x2a, 0x1d, 0x38, 0xdb, 0x01


def frame(kind, payload=b""):
    return struct.pack("<IB", len(payload) + 1, kind) + payload


def parse_video(p):
    return dict(keyframe=bool(p[0] & 1), monitor=struct.unpack_from("<H", p, 2)[0],
                capture_us=struct.unpack_from("<Q", p, 4)[0], encode_us=struct.unpack_from("<I", p, 12)[0],
                width=struct.unpack_from("<H", p, 16)[0], height=struct.unpack_from("<H", p, 18)[0],
                data=p[20:], t=time.time())


def nal_types(au):
    out, i = [], 0
    while i + 3 < len(au):
        if au[i] == 0 and au[i + 1] == 0 and au[i + 2] == 1:
            out.append(au[i + 3] & 0x1f)
            i += 3
        else:
            i += 1
    return out


class Agent:
    """One agent connection with a reader thread."""

    def __init__(self, sock):
        self.s = sock
        self.q = queue.Queue()
        self.video = []          # parsed video packets (all, in order)
        self.audio = []          # (arrival time, capture_us, Opus packet)
        self.vlock = threading.Lock()
        self.logs = []
        self.alive = True
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        buf = b""
        try:
            while True:
                d = self.s.recv(1 << 20)
                if not d:
                    break
                buf += d
                while len(buf) >= 4:
                    n = struct.unpack_from("<I", buf)[0]
                    if len(buf) < 4 + n:
                        break
                    body, buf = buf[4:4 + n], buf[4 + n:]
                    self._msg(body[0], body[1:])
        except OSError:
            pass
        self.alive = False
        self.q.put(("closed", None))

    def _msg(self, k, p):
        if k == K["VIDEO"]:
            with self.vlock:
                self.video.append(parse_video(p))
        elif k == K["AUDIO"]:
            with self.vlock:
                self.audio.append((time.time(), struct.unpack_from("<Q", p)[0], p[8:]))
        elif k == K["LOG"]:
            self.logs.append(p.decode("utf-8", "replace"))
            print("  [agent log]", self.logs[-1], flush=True)
        elif k == K["PING"]:
            self.send(K["PONG"], p)
        elif k in (K["HELLO"], K["VIDEO_STATUS"], K["CLIPBOARD"], K["LAYOUT"]):
            self.q.put((KN[k], json.loads(p)))
        else:
            self.q.put((KN.get(k, hex(k)), p))

    def send(self, kind, payload=b""):
        self.s.sendall(frame(kind, payload))

    def sendj(self, kind, obj):
        self.send(kind, json.dumps(obj).encode())

    def wait(self, name, timeout=10.0, pred=None):
        end = time.time() + timeout
        while time.time() < end:
            try:
                k, v = self.q.get(timeout=max(0.05, end - time.time()))
            except queue.Empty:
                break
            if k == "closed":
                raise RuntimeError("agent disconnected")
            if k == name and (pred is None or pred(v)):
                return v
        return None

    def drain(self, name=None):
        keep = []
        while True:
            try:
                k, v = self.q.get_nowait()
            except queue.Empty:
                break
            if name is not None and k != name:
                keep.append((k, v))
        for it in keep:
            self.q.put(it)

    # --- input helpers -------------------------------------------------
    def key(self, qnum, down):
        self.send(K["INPUT"], struct.pack("<BHB", 1, qnum, 1 if down else 0))

    def tap(self, qnum, gap=0.02):
        self.key(qnum, True); time.sleep(gap); self.key(qnum, False); time.sleep(gap)

    def chord(self, *keys):
        for k in keys:
            self.key(k, True); time.sleep(0.03)
        for k in reversed(keys):
            self.key(k, False); time.sleep(0.03)

    def type(self, text, gap=0.03):
        for ch in text:
            if ch in SHIFT:
                self.chord(LSHIFT, SC[SHIFT[ch]])
            elif ch.isupper():
                self.chord(LSHIFT, SC[ch.lower()])
            else:
                self.tap(SC[ch])
            time.sleep(gap)

    def abs(self, x, y, monitor=0):
        self.send(K["INPUT"], struct.pack("<BIIH", 4, x, y, monitor))

    def rel(self, dx, dy):
        self.send(K["INPUT"], struct.pack("<Bii", 3, dx, dy))

    def button(self, b, down):
        self.send(K["INPUT"], struct.pack("<BBB", 2, b, 1 if down else 0))

    def wheel(self, dx, dy):
        self.send(K["INPUT"], struct.pack("<Bii", 5, dx, dy))

    def start_video(self, **kw):
        sv = dict(codec="h264", width=1280, height=720, fps=30, bitrate_kbps=4000, max_bitrate_kbps=8000,
                  keyframe_interval_frames=60, low_delay=True, fixed_rate=True, chroma="420", hdr=False,
                  intra_refresh=False, monitor=0)
        sv.update(kw)
        self.drain("VIDEO_STATUS")
        with self.vlock:
            self.video = []
        self.sendj(K["START_VIDEO"], sv)
        return sv, self.wait("VIDEO_STATUS", 20)

    def stop_video(self):
        self.send(K["STOP_VIDEO"])
        time.sleep(0.5)


class Ctx:
    def __init__(self, a, args):
        self.a, self.args = a, args
        self.out = args.out
        self.results = {}

    # The poller runs each probe script as a script block fetched from hx-cmd.ps1.
    POLLER = r"""
$ErrorActionPreference='Continue'
$last=''
Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/hx-poller-ready.txt -Body ready | Out-Null
while($true){
  try{
    $s=Invoke-RestMethod -Uri ("http://10.0.2.2:8000/hx-cmd.ps1?r="+(Get-Random))
    if($s -match '^#id (\S+)' -and $Matches[1] -ne $last){ $last=$Matches[1]; try{ & ([scriptblock]::Create($s)) }catch{ Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/hx-err.txt -Body ($_|Out-String) | Out-Null } }
  }catch{}
  Start-Sleep -Milliseconds 400
}
"""

    def _start_poller(self):
        """Type one `powershell ... (irm <url>)` line into Win+R through the agent (this itself
        exercises the keyboard path); the poller then runs the probe scripts, so later probes
        need no typing (the Run dialog's history popup eats keys of repeated command lines)."""
        a = self.a
        os.makedirs(self.args.www, exist_ok=True)
        os.makedirs(self.args.up, exist_ok=True)
        ready = os.path.join(self.args.up, "hx-poller-ready.txt")
        with open(os.path.join(self.args.www, "hx-poller.ps1"), "w") as f:
            f.write(self.POLLER)
        # The guest launcher may already have started it (then the file exists).
        end = time.time() + self.args.poller_wait
        while time.time() < end:
            if os.path.exists(ready):
                self.poller = True
                return True
            time.sleep(0.5)
        for attempt in range(4):
            a.tap(ESC)
            time.sleep(0.5)
            a.chord(LWIN, SC["r"])
            time.sleep(2.5)
            a.chord(LCTRL, SC["a"])
            a.type('powershell -w h -ep bypass -c "iex (irm http://10.0.2.2:8000/hx-poller.ps1)"', gap=0.02)
            time.sleep(0.5)
            a.tap(SC["\n"])
            end = time.time() + 25
            while time.time() < end:
                if os.path.exists(ready):
                    self.poller = True
                    return True
                time.sleep(0.5)
        return False

    def probe(self, name, script, wait=40):
        """Run a PowerShell probe in the guest; it answers by PUT of its output to
        http://10.0.2.2:8001/hx-<name>.txt (file in --up). Returns that text or None."""
        if not getattr(self, "poller", False) and not self._start_poller():
            return None
        upf = os.path.join(self.args.up, f"hx-{name}.txt")
        if os.path.exists(upf):
            os.remove(upf)
        with open(os.path.join(self.args.www, "hx-cmd.ps1"), "w") as f:
            f.write(f"#id {name}-{time.time()}\n" + script + "\n")
        end = time.time() + wait
        while time.time() < end:
            if os.path.exists(upf) and os.path.getsize(upf) > 0:
                time.sleep(0.3)
                return open(upf, "rb").read().decode("utf-8", "replace")
            time.sleep(0.4)
        return None


def check(ctx, name, ok, detail):
    print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}", flush=True)
    ctx.results[name] = dict(ok=bool(ok), detail=detail)
    return ok


# --- tests ---------------------------------------------------------------
def t_hello(ctx):
    a = ctx.a
    h = ctx.hello
    ctx.results["hello_json"] = h
    c = h.get("caps", {})
    check(ctx, "hello.os", h.get("os") == "windows" and h.get("v") == 1, f"os={h.get('os')} ver={h.get('os_version')} agent={h.get('agent_version')}")
    check(ctx, "hello.caps", c.get("clipboard") and c.get("resize") and c.get("relative_mouse")
          and bool(c.get("gamepad")) == ctx.args.expect_gamepad
          and (not ctx.args.virtual_display or bool(c.get("virtual_display"))),
          json.dumps(c))
    return True


def analyze(path, width, height, min_frames=1, expect_keyframes=None):
    """ffprobe/ffmpeg checks of a raw Annex B file. Returns (ok, info dict)."""
    info = {}
    if not shutil.which("ffprobe"):
        return None, {"error": "ffprobe not installed here; run `win_harness.py analyze` elsewhere"}
    r = subprocess.run(["ffprobe", "-v", "error", "-f", "h264", "-count_frames", "-select_streams", "v:0",
                        "-show_entries", "stream=codec_name,profile,width,height,nb_read_frames",
                        "-of", "json", path], capture_output=True, text=True)
    st = (json.loads(r.stdout or "{}").get("streams") or [{}])[0]
    info["stream"] = st
    r = subprocess.run(["ffprobe", "-v", "error", "-f", "h264", "-select_streams", "v:0", "-show_entries",
                        "frame=key_frame,pict_type", "-of", "csv=p=0", path], capture_output=True, text=True)
    frames = [l.split(",") for l in r.stdout.split() if l]
    kf = [i for i, f in enumerate(frames) if f and f[0] == "1"]
    info["frames"] = len(frames)
    info["keyframe_indices"] = kf
    info["pict_types"] = "".join(f[1] if len(f) > 1 else "?" for f in frames[:300])
    d = subprocess.run(["ffmpeg", "-v", "error", "-f", "h264", "-i", path, "-f", "null", "-"],
                       capture_output=True, text=True)
    info["decode_rc"] = d.returncode
    info["decode_errors"] = d.stderr.strip()[:500]
    ok = (st.get("codec_name") == "h264" and st.get("width") == width and st.get("height") == height
          and len(frames) >= min_frames and d.returncode == 0 and not d.stderr.strip())
    return ok, info


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(len(v) * p / 100))] if v else 0


def collect_video(ctx, name, secs, **kw):
    a = ctx.a
    sv, st = a.start_video(**kw)
    ok = bool(st and st.get("running") and not st.get("error"))
    check(ctx, f"{name}.status", ok, json.dumps(st))
    if not ok:
        return None
    time.sleep(secs / 2)
    t_req = time.time()
    req_us = None
    a.send(K["KEYFRAME"])
    time.sleep(secs / 2)
    a.stop_video()
    with a.vlock:
        pk = list(a.video)
    path = os.path.join(ctx.out, f"{name}.h264")
    with open(path, "wb") as f:
        for p in pk:
            f.write(p["data"])
    return sv, st, pk, path, t_req


def t_video_common(ctx, name, secs, expect_w, expect_h, fixed, **kw):
    r = collect_video(ctx, name, secs, fixed_rate=fixed, **kw)
    if not r:
        return False
    sv, st, pk, path, t_req = r
    n = len(pk)
    enc = [p["encode_us"] for p in pk]
    kf = [i for i, p in enumerate(pk) if p["keyframe"]]
    ctx.results[name + ".stats"] = dict(packets=n, keyframes=kf, bytes=sum(len(p["data"]) for p in pk),
                                        encode_us_median=pct(enc, 50), encode_us_p95=pct(enc, 95),
                                        encode_us_max=max(enc) if enc else 0, status=st)
    s = ctx.results[name + ".stats"]
    print(f"  {name}: {n} packets, keyframes at {kf[:12]}, encode_us median {s['encode_us_median']} p95 {s['encode_us_p95']} max {s['encode_us_max']}, "
          f"{s['bytes']} bytes, encoder={st.get('encoder')} capture={st.get('capture')}", flush=True)
    check(ctx, f"{name}.first-keyframe", n > 0 and pk[0]["keyframe"] and {7, 8, 5} <= set(nal_types(pk[0]["data"])),
          f"first packet keyframe={pk[0]['keyframe'] if n else None} nal types={nal_types(pk[0]['data']) if n else None}")
    check(ctx, f"{name}.size", n > 0 and all((p["width"], p["height"]) == (expect_w, expect_h) for p in pk),
          f"packets {expect_w}x{expect_h}; status {st.get('width')}x{st.get('height')}")
    if fixed:
        exp = secs * sv["fps"]
        check(ctx, f"{name}.frame-count", 0.8 * exp <= n <= 1.1 * exp, f"{n} packets for {secs}s at {sv['fps']} fps (expected ~{exp})")
        # periodic GOP: keyframe_interval_frames=60 -> a keyframe about every 60 frames, plus the requested one
        gop = sv["keyframe_interval_frames"]
        regular = [k for k in kf if k % gop == 0 or k == 0]
        check(ctx, f"{name}.gop", len(kf) >= 2, f"keyframes at packet indices {kf} (interval {gop})")
    # keyframe requested at t_req: an IDR must follow within 1 s
    after = [p for p in pk if p["t"] >= t_req and p["keyframe"]]
    delay = (after[0]["t"] - t_req) if after else None
    check(ctx, f"{name}.keyframe-request", bool(after) and delay < 1.0,
          f"IDR {'%.3f s' % delay if delay is not None else 'never'} after Keyframe request")
    ok, info = analyze(path, expect_w, expect_h, min_frames=max(1, n // 2) if n else 1)
    ctx.results[name + ".ffprobe"] = info
    if ok is None:
        print("  (no ffprobe here: analyze later)", info, flush=True)
    else:
        print("  ffprobe:", json.dumps(info.get("stream")), "frames", info["frames"], "keyframes", info["keyframe_indices"][:12],
              "pict_types", info["pict_types"][:60], "decode_rc", info["decode_rc"], info["decode_errors"], flush=True)
        check(ctx, f"{name}.decodes", ok and info["frames"] == n,
              f"ffprobe {info['frames']} frames vs {n} packets, {expect_w}x{expect_h}, decode rc={info['decode_rc']}")
        check(ctx, f"{name}.no-bframes", "B" not in info["pict_types"], f"pict types {info['pict_types'][:40]}")
    return True


def t_video(ctx):
    w, h = ctx.args.size
    return t_video_common(ctx, "video", ctx.args.secs, w, h, True, width=w, height=h, fps=ctx.args.fps)


def t_video_damage(ctx):
    """fixed_rate=false: few packets while idle, a burst while the screen changes."""
    a = ctx.a
    w, h = ctx.args.size
    sv, st = a.start_video(width=w, height=h, fps=ctx.args.fps, fixed_rate=False)
    check(ctx, "video-damage.status", bool(st and st.get("running")), json.dumps(st))
    time.sleep(1.0)
    with a.vlock:
        a.video.clear()
    time.sleep(3.0)
    with a.vlock:
        idle = len(a.video)
        a.video.clear()
    a.chord(LWIN, SC["r"])
    time.sleep(1.5)
    for i in range(20):
        a.rel(10, 0) if False else None
        a.type("abcdef"[i % 6], gap=0.05)
    time.sleep(0.5)
    with a.vlock:
        busy = len(a.video)
    a.tap(ESC)
    time.sleep(0.5)
    a.stop_video()
    check(ctx, "video-damage.on-change-only", idle <= 3 and busy >= 5, f"{idle} packets in 3 s idle, {busy} while the Run dialog redraws")
    return True


def t_video_native(ctx):
    lay = ctx.results.get("layout.now")
    w, h = (lay["width"], lay["height"]) if lay else (0, 0)
    r = collect_video(ctx, "video-native", 3, width=0, height=0, fps=ctx.args.fps, fixed_rate=True)
    if not r:
        return False
    sv, st, pk, path, _ = r
    check(ctx, "video-native.size", bool(pk) and (not w or (st["width"], st["height"]) == (w & ~1, h & ~1)) and
          (pk[0]["width"], pk[0]["height"]) == (st["width"], st["height"]),
          f"monitor {w}x{h} -> encoded {st['width']}x{st['height']}")
    ok, info = analyze(path, st["width"], st["height"], 10)
    if ok is not None:
        check(ctx, "video-native.decodes", ok, json.dumps(info.get("stream")) + f" frames={info['frames']}")
    return True


CUR = r"""
Add-Type -A System.Windows.Forms
$p=[Windows.Forms.Cursor]::Position
$t="X=$($p.X) Y=$($p.Y)"
Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/hx-%NAME%.txt -Body $t | Out-Null
"""

CLIPGET = r"""
Add-Type -A System.Windows.Forms
$t=[Windows.Forms.Clipboard]::GetText()
$b=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($t))
Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/hx-%NAME%.txt -Body $b | Out-Null
"""

CLIPSET = r"""
Add-Type -A System.Windows.Forms
[Windows.Forms.Clipboard]::SetText("from the guest: $([char]0xfc)$([char]0x20ac)$([char]0x2713) line1`r`nline2")
Start-Sleep 2
Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/hx-%NAME%.txt -Body done | Out-Null
"""


def t_input(ctx):
    a = ctx.a
    a.drain("CLIPBOARD")
    # 1. keyboard: Win+R -> notepad -> type -> select all, copy -> the agent reports the clipboard.
    text = "Hello Horcrux 123 ok"
    for attempt in range(3):
        a.tap(ESC)
        a.chord(LWIN, SC["r"])
        time.sleep(2.0)
        a.type("notepad")
        a.tap(SC["\n"])
        time.sleep(4.0)
        a.type(text, gap=0.05)
        time.sleep(0.5)
        a.chord(LCTRL, SC["a"])
        a.chord(LCTRL, SC["c"])
        c = a.wait("CLIPBOARD", 5, lambda v: v.get("text", "").strip() == text)
        if c:
            break
        a.chord(LALT, 0x3e)  # alt+F4 (E0-less F4 = 0x3e), then do not save
        time.sleep(1); a.type("n"); time.sleep(1)
    check(ctx, "input.keyboard", bool(c), f"typed {text!r} into Notepad via Win+R, read back via Ctrl+A/Ctrl+C -> agent clipboard {c and c.get('text')!r}")
    # extended key: Home/End (E0 47 / E0 4f): type X at start with Home
    a.tap(0xc7)  # Home (E0 extended)
    a.type("X")
    a.chord(LCTRL, SC["a"]); a.chord(LCTRL, SC["c"])
    c = a.wait("CLIPBOARD", 5, lambda v: v.get("text", "").strip() == "X" + text)
    check(ctx, "input.extended-key", bool(c), f"Home (E0) then X -> {c and c.get('text')!r}")
    a.chord(LALT, 0x3e); time.sleep(1); a.type("n"); time.sleep(1)
    # 2. pointer
    a.abs(300, 200)
    time.sleep(0.5)
    r1 = ctx.probe("cur1", CUR.replace("%NAME%", "cur1"))
    check(ctx, "input.abs-pointer", r1 is not None and r1.strip() in ("X=300 Y=200",) or (r1 and abs(int(r1.split()[0][2:]) - 300) <= 1 and abs(int(r1.split()[1][2:]) - 200) <= 1), f"Abs(300,200) -> guest cursor {r1!r}")
    a.abs(300, 200)
    time.sleep(0.2)
    for _ in range(5):
        a.rel(20, 10)
        time.sleep(0.02)
    time.sleep(0.3)
    # the Run dialog of the probe moves nothing; read position first through a probe
    r2 = ctx.probe("cur2", CUR.replace("%NAME%", "cur2"))
    try:
        x, y = [int(t[2:]) for t in r2.split()]
    except Exception:
        x = y = None
    check(ctx, "input.rel-pointer", x is not None and x > 300 and y > 200, f"Abs(300,200) then 5x Rel(20,10) -> guest cursor {r2!r} (raw relative; pointer acceleration may scale it)")
    return True


def t_clipboard(ctx):
    a = ctx.a
    a.drain("CLIPBOARD")
    sent = "h\u00e9llo \u2713 w\u00f6rld \u20ac\nline2 \U0001F600"
    a.sendj(K["CLIPBOARD"], dict(mime="text/plain", text=sent))
    time.sleep(0.8)
    r = ctx.probe("clipget", CLIPGET.replace("%NAME%", "clipget"))
    got = base64.b64decode(r).decode("utf-8") if r else None
    check(ctx, "clipboard.set", got is not None and got.replace("\r\n", "\n") == sent, f"host->guest {sent!r} read back {got!r}")
    # echo suppression: the text we set must not come back as a guest clipboard event
    echo = a.wait("CLIPBOARD", 2.5, lambda v: v.get("text", "").replace("\r\n", "\n") == sent)
    check(ctx, "clipboard.no-echo", echo is None, "text set by the node was not sent back")
    a.drain("CLIPBOARD")
    ctx.probe("clipset", CLIPSET.replace("%NAME%", "clipset"))
    c = a.wait("CLIPBOARD", 8, lambda v: v.get("text", "").startswith("from the guest"))
    want = "from the guest: \u00fc\u20ac\u2713 line1\nline2"
    check(ctx, "clipboard.watch", bool(c) and c["text"] == want and c.get("mime") == "text/plain", f"guest->host {c and c['text']!r}")
    return True


def t_layout(ctx):
    a = ctx.a
    a.drain("LAYOUT")
    a.sendj(K["SET_LAYOUT"], dict(monitors=[]))   # empty request: the agent reports nothing it cannot change
    first = a.wait("LAYOUT", 5)
    check(ctx, "layout.report", bool(first and first.get("monitors")), json.dumps(first))
    if not first or not first["monitors"]:
        return False
    orig = first["monitors"][0]
    ctx.results["layout.orig"] = orig
    w, h = ctx.args.layout
    want = dict(x=0, y=0, width=w, height=h, refresh_hz=orig.get("refresh_hz", 60), primary=True)
    a.drain("LAYOUT")
    a.sendj(K["SET_LAYOUT"], dict(monitors=[want]))
    now = a.wait("LAYOUT", 15)
    m = now and now["monitors"][0]
    ctx.results["layout.now"] = m
    check(ctx, "layout.resize", bool(m) and (m["width"], m["height"]) == (w, h), f"asked {w}x{h}, guest reports {m}")
    return True


def t_layout_restore(ctx):
    a = ctx.a
    o = ctx.results.get("layout.orig")
    if o:
        a.drain("LAYOUT")
        a.sendj(K["SET_LAYOUT"], dict(monitors=[o]))
        now = a.wait("LAYOUT", 15)
        check(ctx, "layout.restore", bool(now) and (now["monitors"][0]["width"], now["monitors"][0]["height"]) == (o["width"], o["height"]), json.dumps(now))
    return True


# --- audio ---------------------------------------------------------------
def ogg_crc(data):
    crc = 0
    for b in data:
        crc ^= b << 24
        for _ in range(8):
            crc = ((crc << 1) ^ 0x04C11DB7) & 0xffffffff if crc & 0x80000000 else (crc << 1) & 0xffffffff
    return crc


def ogg_opus(packets, channels=2, frame_samples=480):
    """Raw Opus packets -> an Ogg Opus file (RFC 7845), one packet per page."""
    out, seq, gran = [], 0, 0

    def page(payload, granule, flags):
        nonlocal seq
        segs, n = [], len(payload)
        while n >= 255:
            segs.append(255); n -= 255
        segs.append(n)
        hdr = struct.pack("<4sBBqIIIB", b"OggS", 0, flags, granule, 0x48584155, seq, 0, len(segs)) + bytes(segs)
        crc = ogg_crc(hdr + payload)
        hdr = hdr[:22] + struct.pack("<I", crc) + hdr[26:]
        seq += 1
        out.append(hdr + payload)

    page(b"OpusHead" + struct.pack("<BBHIhB", 1, channels, 0, 48000, 0, 0), 0, 2)
    page(b"OpusTags" + struct.pack("<I", 7) + b"harness" + struct.pack("<I", 0), 0, 0)
    for i, p in enumerate(packets):
        gran += frame_samples
        page(p, gran, 4 if i == len(packets) - 1 else 0)
    return b"".join(out)


def write_tone(path, freq, amp, secs, rate=48000):
    import math, wave
    with wave.open(path, "wb") as w:
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(rate)
        fr = bytearray()
        for i in range(int(secs * rate)):
            s = int(amp * 32767 * math.sin(2 * math.pi * freq * i / rate))
            fr += struct.pack("<hh", s, s)
        w.writeframes(bytes(fr))


def goertzel(x, rate, freq):
    import math
    w = 2 * math.pi * freq / rate
    c = 2 * math.cos(w)
    s1 = s2 = 0.0
    for v in x:
        s1, s2 = v + c * s1 - s2, s1
    return 2 * math.sqrt(max(0.0, s1 * s1 + s2 * s2 - c * s1 * s2)) / len(x)


def analyze_audio(path, tone_hz, rate=48000):
    """Decode an Ogg Opus file with ffmpeg; find the tone and measure it. Returns (ok|None, info)."""
    import array, math
    if not shutil.which("ffmpeg"):
        return None, {"error": "ffmpeg not installed here; run `win_harness.py analyze-audio` elsewhere"}
    r = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-f", "f32le", "-ac", "2", "-ar", str(rate), "-"],
                       capture_output=True)
    pcm = array.array("f")
    pcm.frombytes(r.stdout[: len(r.stdout) // 8 * 8])
    left, right = pcm[0::2], pcm[1::2]
    info = dict(decode_rc=r.returncode, decode_errors=r.stderr.decode()[:300], seconds=len(left) / rate)
    blk = rate // 10   # 100 ms blocks
    rms = [math.sqrt(sum(v * v for v in left[i:i + blk]) / blk) for i in range(0, len(left) - blk + 1, blk)]
    info["rms_dbfs_per_100ms"] = [round(20 * math.log10(max(v, 1e-9)), 1) for v in rms]
    loud = [i for i, v in enumerate(rms) if v > 0.01]
    if not loud:
        return False, info
    a, b = loud[0] + 3, loud[-1] - 2          # inside the tone, away from its edges
    seg_l = left[a * blk:(b + 1) * blk]
    seg_r = right[a * blk:(b + 1) * blk]
    zc = sum(1 for i in range(1, len(seg_l)) if seg_l[i - 1] < 0 <= seg_l[i])
    freq = zc * rate / len(seg_l) if seg_l else 0.0
    # refine around the zero-crossing estimate
    best = max((goertzel(seg_l[:rate], rate, f / 10), f / 10) for f in range(int(freq * 10) - 30, int(freq * 10) + 31))
    lvl = goertzel(seg_l, rate, best[1])
    total = math.sqrt(sum(v * v for v in seg_l) / len(seg_l)) * math.sqrt(2)
    quiet = [v for i, v in enumerate(rms) if i < loud[0] - 2]
    info.update(tone_seconds=round(len(seg_l) / rate, 2), freq_hz=round(best[1], 1), freq_zc_hz=round(freq, 2),
                tone_amplitude=round(lvl, 4), tone_dbfs=round(20 * math.log10(max(lvl, 1e-9)), 2),
                tone_share_of_energy=round(lvl / total, 4) if total else 0,
                right_equals_left_dbfs=round(20 * math.log10(max(goertzel(seg_r, rate, best[1]), 1e-9)), 2),
                silence_before_dbfs=round(20 * math.log10(max(max(quiet) if quiet else 0, 1e-9)), 1))
    ok = (r.returncode == 0 and abs(best[1] - tone_hz) <= 2 and info["tone_share_of_energy"] > 0.95
          and info["silence_before_dbfs"] < -60)
    return ok, info


TONE_PLAY = r"""
$ErrorActionPreference='Stop'
New-Item -ItemType Directory -Force C:\hxgt | Out-Null
Invoke-WebRequest http://10.0.2.2:8000/hx-tone.wav -OutFile C:\hxgt\hx-tone.wav
$p=New-Object System.Media.SoundPlayer 'C:\hxgt\hx-tone.wav'
$p.Load()
$t0=Get-Date
$p.PlaySync()
$t="played $(((Get-Date)-$t0).TotalSeconds) s"
Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/hx-%NAME%.txt -Body $t | Out-Null
"""

SVC = r"""
%CMD% audiosrv
Start-Sleep 1
Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/hx-%NAME%.txt -Body ((Get-Service audiosrv).Status | Out-String) | Out-Null
"""


def audio_window(a, t0, t1):
    with a.vlock:
        return [p for p in a.audio if t0 <= p[0] < t1]


def cadence(ctx, name, pk, frame_us, secs):
    caps = [p[1] for p in pk]
    d_cap = sorted(set(caps[i] - caps[i - 1] for i in range(1, len(caps))))
    arr = [round((pk[i][0] - pk[i - 1][0]) * 1000, 2) for i in range(1, len(pk))]
    exp = secs * 1e6 / frame_us
    st = dict(packets=len(pk), expected=exp, capture_us_steps=d_cap[:10], arrival_ms_median=pct(arr, 50),
              arrival_ms_p95=pct(arr, 95), arrival_ms_max=max(arr) if arr else None,
              sizes=sorted(set(len(p[2]) for p in pk))[:10])
    ctx.results[name + ".stats"] = st
    print(f"  {name}: {st}", flush=True)
    check(ctx, name + ".cadence", 0.95 * exp <= len(pk) <= 1.05 * exp and d_cap == [frame_us],
          f"{len(pk)} packets in {secs} s (expected {exp:.0f}), capture_us steps {d_cap[:5]}, "
          f"arrival gap median {st['arrival_ms_median']} ms p95 {st['arrival_ms_p95']} ms")
    return st


def t_audio(ctx):
    a = ctx.a
    check(ctx, "audio.caps", ctx.hello.get("caps", {}).get("audio") is True, "Hello caps.audio (render endpoint probed)")
    tone_hz, amp, tone_secs = ctx.args.tone_hz, 0.5, 4
    write_tone(os.path.join(ctx.args.www, "hx-tone.wav"), tone_hz, amp, tone_secs)
    with a.vlock:
        a.audio = []
    nlogs = len(a.logs)
    a.sendj(K["START_AUDIO"], dict(codec="opus", sample_rate=48000, channels=2, bitrate_kbps=128, frame_ms=10))
    t_start = time.time()
    end = t_start + 5
    while time.time() < end and not a.audio:
        time.sleep(0.005)
    first_ms = (a.audio[0][0] - t_start) * 1000 if a.audio else None
    ctx.results["audio.first_packet_ms"] = first_ms
    check(ctx, "audio.starts", first_ms is not None and first_ms < 1000,
          f"first packet {first_ms and round(first_ms)} ms after StartAudio (opens the endpoint first)")
    time.sleep(0.5)
    # 1. silent endpoint: loopback delivers nothing, the agent must keep the 10 ms cadence
    t0 = time.time()
    time.sleep(3.0)
    silent = audio_window(a, t0, time.time())
    cadence(ctx, "audio.silent", silent, 10000, 3.0)
    sizes = set(len(p[2]) for p in silent)
    toc = set(p[2][0] for p in silent if p[2])
    check(ctx, "audio.cbr-opus", sizes == {160} and all((t >> 3) >= 16 and (t & 4) for t in toc),
          f"packet sizes {sorted(sizes)} (128 kbit/s CBR 10 ms = 160 bytes), TOC bytes {[hex(t) for t in sorted(toc)]} "
          f"(CELT, stereo)")
    # 2. play a tone in the guest's interactive session
    t_play = time.time()
    r = ctx.probe("tone", TONE_PLAY.replace("%NAME%", "tone"), wait=tone_secs + 40)
    t_played = time.time()
    check(ctx, "audio.guest-played", bool(r and r.startswith("played")), f"guest SoundPlayer: {r!r}")
    time.sleep(1.0)
    during = audio_window(a, t_play, time.time())
    cadence(ctx, "audio.playing", during, 10000, time.time() - t_play)
    # 3. stop
    a.send(K["STOP_AUDIO"])
    time.sleep(0.5)
    n_stop = len(a.audio)
    time.sleep(1.0)
    check(ctx, "audio.stop", len(a.audio) == n_stop, f"{len(a.audio) - n_stop} packets after StopAudio + 0.5 s")
    with a.vlock:
        pk = list(a.audio)
    with open(os.path.join(ctx.out, "audio-packets.json"), "w") as f:
        json.dump([dict(t=round(p[0] - t_start, 4), capture_us=p[1], bytes=len(p[2])) for p in pk], f)
    path = os.path.join(ctx.out, "audio.ogg")
    with open(path, "wb") as f:
        f.write(ogg_opus([p[2] for p in pk]))
    ctx.results["audio.timeline"] = dict(play_start_s=round(t_play - t_start, 3), play_end_s=round(t_played - t_start, 3))
    ctx.results["audio.agent_logs"] = a.logs[nlogs:]
    ok, info = analyze_audio(path, tone_hz)
    ctx.results["audio.decode"] = info
    if ok is None:
        print("  (no ffmpeg here: run analyze-audio on", path, ")", flush=True)
    else:
        info_s = {k: v for k, v in info.items() if k != "rms_dbfs_per_100ms"}
        check(ctx, "audio.tone", ok, f"decoded: {info_s}")
    # 4. 20 ms frames at 64 kbit/s
    with a.vlock:
        a.audio = []
    a.sendj(K["START_AUDIO"], dict(codec="opus", sample_rate=48000, channels=2, bitrate_kbps=64, frame_ms=20))
    time.sleep(1.0)
    t0 = time.time()
    time.sleep(2.0)
    p20 = audio_window(a, t0, time.time())
    a.send(K["STOP_AUDIO"])
    cadence(ctx, "audio.20ms", p20, 20000, 2.0)
    check(ctx, "audio.20ms-cbr", set(len(p[2]) for p in p20) == {160}, f"sizes {sorted(set(len(p[2]) for p in p20))} (64 kbit/s x 20 ms = 160 bytes)")
    time.sleep(0.5)
    return True


def t_audio_recover(ctx):
    """The endpoint goes away (Windows Audio service stopped) and comes back: the stream keeps its
    cadence with silence, the agent says so in a Log message and reopens the endpoint."""
    a = ctx.a
    with a.vlock:
        a.audio = []
    nlogs = len(a.logs)
    a.sendj(K["START_AUDIO"], dict(codec="opus", sample_rate=48000, channels=2, bitrate_kbps=128, frame_ms=10))
    time.sleep(1.5)
    r = ctx.probe("svcstop", SVC.replace("%CMD%", "Stop-Service -Force").replace("%NAME%", "svcstop"))
    t0 = time.time()
    time.sleep(3.0)
    gone = audio_window(a, t0, time.time())
    logs_gone = a.logs[nlogs:]
    check(ctx, "audio-recover.reported", any("audio:" in l and ("stopped" in l or "retrying" in l) for l in logs_gone),
          f"service {r and r.strip()!r}; agent logs {logs_gone}")
    cadence(ctx, "audio-recover.gone", gone, 10000, 3.0)
    n2 = len(a.logs)
    r = ctx.probe("svcstart", SVC.replace("%CMD%", "Start-Service").replace("%NAME%", "svcstart"))
    end = time.time() + 10
    while time.time() < end and not any("capturing" in l for l in a.logs[n2:]):
        time.sleep(0.2)
    check(ctx, "audio-recover.reopened", any("capturing" in l for l in a.logs[n2:]),
          f"service {r and r.strip()!r}; agent logs {a.logs[n2:]}")
    a.send(K["STOP_AUDIO"])
    time.sleep(0.5)


def t_virtual_display(ctx):
    """SetLayout at the virtual size/rate: the agent creates a Horcrux virtual monitor in that
    mode (the guest GPU's own outputs are left alone) and reports it in Layout."""
    a = ctx.a
    w, h, hz = ctx.args.virtual_mode
    a.drain("LAYOUT")
    a.sendj(K["SET_LAYOUT"], dict(monitors=[dict(x=0, y=0, width=w, height=h, refresh_hz=hz, primary=True)]))
    now = a.wait("LAYOUT", 30)
    mons = (now or {}).get("monitors", [])
    v = [i for i, m in enumerate(mons) if m.get("virtual_display")]
    ctx.results["virtual.layout"] = now
    hit = [i for i in v if (mons[i]["width"], mons[i]["height"], mons[i]["refresh_hz"]) == (w, h, hz)]
    check(ctx, "virtual-display.mode", bool(hit), f"asked {w}x{h}@{hz}, guest reports {json.dumps(mons)}")
    if hit:
        ctx.virtual_index = hit[0]
        check(ctx, "virtual-display.primary", mons[hit[0]].get("primary") is True, json.dumps(mons[hit[0]]))
    return bool(hit)


def t_video_virtual(ctx):
    """Capture + encode of the virtual monitor at its native size."""
    i = getattr(ctx, "virtual_index", None)
    if i is None:
        check(ctx, "video-virtual.monitor", False, "no virtual monitor (run virtual-display first)")
        return False
    w, h, hz = ctx.args.virtual_mode
    fps = min(hz, ctx.args.virtual_fps)
    ok = t_video_common(ctx, "video-virtual", ctx.args.secs, w, h, True, width=w, height=h, fps=fps,
                        monitor=i, bitrate_kbps=20000, max_bitrate_kbps=40000)
    st = (ctx.results.get("video-virtual.stats") or {}).get("status") or {}
    if ctx.args.expect_capture:
        check(ctx, "video-virtual.capture", st.get("capture") == ctx.args.expect_capture,
              f"capture={st.get('capture')} (expected {ctx.args.expect_capture})")
    return ok


GAMEPAD_PROBE = r"""
if (-not ('HxJoy' -as [type])) { Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class HxJoy {
  [StructLayout(LayoutKind.Sequential)] public struct JI { public int Size, Flags, X, Y, Z, R, U, V, Buttons, ButtonNumber, POV, R1, R2; }
  [DllImport("winmm.dll")] public static extern int joyGetPosEx(int id, ref JI ji);
}
"@ }
$seen = @{}
$t0 = Get-Date
while (((Get-Date) - $t0).TotalSeconds -lt __SECS__) {
  for ($i = 0; $i -lt 4; $i++) {
    $j = New-Object HxJoy+JI; $j.Size = [Runtime.InteropServices.Marshal]::SizeOf($j); $j.Flags = 0xff
    if ([HxJoy]::joyGetPosEx($i, [ref]$j) -eq 0) { $seen["$i $($j.X) $($j.Y) $($j.Z) $($j.R) $($j.U) $($j.V) $($j.Buttons) $($j.POV)"] = 1 }
  }
  Start-Sleep -Milliseconds 50
}
Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/hx-gamepad.txt -Body (($seen.Keys | Sort-Object) -join "`n") | Out-Null
"""


def t_gamepad(ctx):
    """Gamepad states sent as Msg::Input arrive at the guest's game-controller API (winmm
    joyGetPosEx, what joy.cpl and DirectInput games read)."""
    a = ctx.a
    secs = 6
    pad = lambda buttons=0, lx=0, ly=0, rx=0, ry=0, lt=0, rt=0: struct.pack("<BBIhhhhBB", 6, 0, buttons, lx, ly, rx, ry, lt, rt)
    states = [pad(buttons=1 | 1 << 3, lx=32767, ry=-32768, lt=255),      # A + Y, left stick right, right stick up, LT
              pad(buttons=1 << 12 | 1 << 15, ly=32767, rx=32767, rt=128)]  # d-pad up+right, left stick down, right stick right, half RT
    # winmm axis names for the pad's HID usages: X/Y = left stick, Z = left trigger,
    # R (rudder) = right trigger, U = right stick Y, V = right stick X
    expect = [dict(X=65535, Y=32767, Z=65535, R=0, U=0, V=32767, buttons=0x1 | 0x8, pov=65535),
              dict(X=32767, Y=65535, Z=0, R=32896, U=32767, V=65535, buttons=0, pov=4500)]
    done = threading.Event()

    def feed():
        k = 0
        while not done.is_set():
            a.send(K["INPUT"], states[(k // 25) % 2])   # 0.5 s each, 50 Hz
            k += 1
            time.sleep(0.02)
        a.send(K["INPUT"], pad())

    th = threading.Thread(target=feed, daemon=True)
    th.start()
    out = ctx.probe("gamepad", GAMEPAD_PROBE.replace("__SECS__", str(secs)), wait=secs + 40)
    done.set()
    th.join(2)
    seen = []
    for line in (out or "").splitlines():
        f = line.split()
        if len(f) == 9:
            seen.append(dict(id=int(f[0]), X=int(f[1]), Y=int(f[2]), Z=int(f[3]), R=int(f[4]), U=int(f[5]), V=int(f[6]),
                             buttons=int(f[7]), pov=int(f[8])))
    ctx.results["gamepad.seen"] = seen

    def near(a_, b_):
        return abs(a_ - b_) <= 300

    for n, e in enumerate(expect):
        hit = [s_ for s_ in seen if all(near(s_[k], v) for k, v in e.items() if k not in ("buttons", "pov"))
               and s_["buttons"] == e["buttons"] and (e["pov"] is None or s_["pov"] == e["pov"])]
        check(ctx, f"gamepad.state{n}", bool(hit), f"expected {e}; joyGetPosEx saw {seen}")
    return True


TESTS = [("hello", t_hello), ("video", t_video), ("video-damage", t_video_damage), ("input", t_input),
         ("clipboard", t_clipboard), ("layout", t_layout), ("video-native", t_video_native),
         ("layout-restore", t_layout_restore), ("audio", t_audio), ("audio-recover", t_audio_recover),
         ("virtual-display", t_virtual_display), ("video-virtual", t_video_virtual), ("gamepad", t_gamepad)]


def serve(args):
    os.makedirs(args.out, exist_ok=True)
    os.makedirs(args.www, exist_ok=True)
    with open(os.path.join(args.www, "hx-poller.ps1"), "w") as f:
        f.write(Ctx.POLLER)
    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((args.bind, args.port))
    srv.listen(1)
    print(f"listening on {args.bind}:{args.port}", flush=True)
    srv.settimeout(args.wait)
    conn, addr = srv.accept()
    conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    print("agent connected from", addr, flush=True)
    a = Agent(conn)
    ctx = Ctx(a, args)
    h = a.wait("HELLO", 15)
    if not h:
        print("FAIL no Hello", flush=True)
        return 1
    ctx.hello = h
    a.sendj(K["HOST_HELLO"], dict(v=1, clipboard_max_bytes=1 << 20, virtual_display=args.virtual_display))
    sel = [t for t in args.tests.split(",") if t]
    if {"input", "clipboard", "audio", "audio-recover", "gamepad"} & set(sel):
        print("--- starting the guest-side probe poller (typed through the agent)", flush=True)
        check(ctx, "poller.start", ctx._start_poller(), "typed a PowerShell launcher into Win+R through the agent's keyboard injection")
    for name, fn in TESTS:
        if name in sel or (name == "layout-restore" and "layout" in sel):
            print(f"--- {name}", flush=True)
            try:
                fn(ctx)
            except Exception as e:  # keep going: later tests may still say something
                check(ctx, name + ".exception", False, repr(e))
                if not a.alive:
                    break
    with open(os.path.join(args.out, "results.json"), "w") as f:
        json.dump(ctx.results, f, indent=1, default=str)
    bad = [k for k, v in ctx.results.items() if isinstance(v, dict) and v.get("ok") is False]
    print(f"SUMMARY: {sum(1 for v in ctx.results.values() if isinstance(v, dict) and v.get('ok'))} passed, {len(bad)} failed {bad}", flush=True)
    return 1 if bad else 0


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("serve")
    s.add_argument("--port", type=int, default=9000)
    s.add_argument("--bind", default="0.0.0.0")
    s.add_argument("--out", required=True)
    s.add_argument("--www", default="/vm/www")
    s.add_argument("--up", default="/vm/up")
    s.add_argument("--tests", default="hello,video,video-damage,input,clipboard,layout,video-native")
    s.add_argument("--size", default="1280x720")
    s.add_argument("--layout", default="1024x768")
    s.add_argument("--fps", type=int, default=30)
    s.add_argument("--virtual-display", action="store_true", help="HostHello asks for Horcrux virtual monitors")
    s.add_argument("--virtual-mode", default="2560x1440@240", help="virtual-display test mode WxH@HZ")
    s.add_argument("--virtual-fps", type=int, default=60, help="video-virtual encode rate (capped at the mode's HZ)")
    s.add_argument("--expect-capture", default="", help="video-virtual: capture source the agent must report")
    s.add_argument("--expect-gamepad", action="store_true", help="hello: the agent reports a virtual gamepad")
    s.add_argument("--secs", type=int, default=6)
    s.add_argument("--wait", type=float, default=900)
    s.add_argument("--poller-wait", type=float, default=15.0, help="seconds to wait for a launcher-started poller before typing one")
    s.add_argument("--probe-settle", type=float, default=4.0, help="seconds to wait before typing a probe into the guest")
    s.add_argument("--tone-hz", type=float, default=1000.0, help="audio test: tone the guest plays")
    an = sub.add_parser("analyze")
    an.add_argument("file")
    an.add_argument("--width", type=int, required=True)
    an.add_argument("--height", type=int, required=True)
    an.add_argument("--min-frames", type=int, default=1)
    aa = sub.add_parser("analyze-audio")
    aa.add_argument("file", help="audio.ogg written by the audio test")
    aa.add_argument("--tone-hz", type=float, default=1000.0)
    args = ap.parse_args()
    if args.cmd == "analyze":
        ok, info = analyze(args.file, args.width, args.height, args.min_frames)
        print(json.dumps(info, indent=1))
        sys.exit(0 if ok else 1)
    if args.cmd == "analyze-audio":
        ok, info = analyze_audio(args.file, args.tone_hz)
        print(json.dumps(info, indent=1))
        print("PASS" if ok else "FAIL", "audio.tone")
        sys.exit(0 if ok else 1)
    args.size = tuple(int(x) for x in args.size.split("x"))
    args.layout = tuple(int(x) for x in args.layout.split("x"))
    wh, hz = args.virtual_mode.split("@")
    args.virtual_mode = tuple(int(x) for x in wh.split("x")) + (int(hz),)
    sys.exit(serve(args))


if __name__ == "__main__":
    main()
