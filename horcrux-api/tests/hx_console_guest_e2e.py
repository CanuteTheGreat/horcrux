#!/usr/bin/env python3
"""Horcrux Console C2b end to end: guest tools in real Windows and Linux VMs,
viewed through the node's WebRTC client in headless Chromium. Driven by
hx_console_guest_e2e.sh (API server, images, playwright).

Windows (HXG_WIN_IMAGE: Windows 11 with virtio-win + QEMU guest agent, no
Horcrux tools; ci/windows-test-image.py):
  * VM created through the API (q35, OVMF Secure Boot, TPM, guest agent,
    HDA sound), its disk imported with POST /vms/:id/import-disk;
  * the guest tools CD is attached by the node; opening a viewer makes the
    node install the MSI through the guest agent (automatic install) and the
    running session switches to the tools without reconnecting;
  * the browser shows the guest-encoded stream (session stats video_source
    "guest", guest_encoder), through the real virtio-serial port;
  * absolute pointer + buttons (rubber-band selection on the desktop),
    raw relative motion through the tools, keyboard into Notepad with the
    keystroke-to-picture latency, clipboard guest -> browser and browser ->
    guest, resize through the tools, audio (Opus track packets) when the
    tools report audio.
Linux (HXG_LINUX_IMAGE: Debian with FFmpeg + qemu-guest-agent; ci/linux-test-image.py):
  * POST /vms/:id/console/guest-tools/install (the UI button's action):
    the node installs the .deb from the tools CD with apt and starts the
    systemd service; RBAC (a view-only user gets 403) and audit;
  * the browser shows the guest-encoded stream; a keystroke reaches the
    guest and its picture comes back (latency); the UI's guest tools bar.
Results: `ok - ...` / `FAIL ...`, `LATENCY {...}` lines, OUT/guest-e2e.json.
"""
import json
import os
import ssl
import statistics
import sys
import time
import urllib.error
import urllib.request

BASE = os.environ.get("HXC_BASE", "https://127.0.0.1:8006")
OUT = os.environ.get("HXC_OUT", "/tmp/hxg-e2e")
IMAGES = os.environ["HXG_IMAGES"]  # <data>/images
WIN = os.environ.get("HXG_WIN_IMAGE", "")
LIN = os.environ.get("HXG_LINUX_IMAGE", "")
TOOLS_ISO = os.environ["HXG_TOOLS_ISO"]
ONLY = os.environ.get("HXG_ONLY", "")  # windows | linux | "" (both)
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
RESULTS = {}
FAILS = []
os.makedirs(OUT, exist_ok=True)


def call(method, path, body=None, token=None, ok=(200, 201, 202, 204), timeout=600):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + "/api" + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=timeout) as r:
            status, text = r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if ok is not None and status not in ok:
        raise SystemExit(f"FAIL {method} {path} -> {status}: {text[:500]}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def check(cond, what, fatal=True):
    if not cond:
        FAILS.append(what)
        print("FAIL " + what, flush=True)
        if fatal:
            dump()
            raise SystemExit(1)
        return False
    print("ok -", what, flush=True)
    return True


def dump():
    RESULTS["fails"] = FAILS
    with open(f"{OUT}/guest-e2e.json", "w") as f:
        json.dump(RESULTS, f, indent=1)


def login(user, pw):
    _, r = call("POST", "/auth/login", {"username": user, "password": pw}, ok=(200,))
    return r.get("ticket") or r.get("token")


def latency_summary(name, values):
    v = [x for x in values if x is not None]
    if not v:
        return None
    v.sort()
    d = {"samples": len(v), "median_ms": round(statistics.median(v), 1),
         "p90_ms": round(v[min(len(v) - 1, int(len(v) * 0.9))], 1),
         "min_ms": round(min(v), 1), "max_ms": round(max(v), 1)}
    print("LATENCY " + json.dumps({name: d}), flush=True)
    RESULTS[name] = d
    return d


ADMIN = login("admin", "admin")

# ---------------------------------------------------------------- settings
_, s = call("GET", "/console/settings", token=ADMIN)
s.update(guest_tools=True, guest_tools_auto_install=True, guest_tools_iso=TOOLS_ISO,
         guest_tools_retry_secs=3, guest_tools_wait_ms=2000, audio=True)
s["desktop"] = dict(s["desktop"], guest_capture=True)
_, s = call("PUT", "/console/settings", s, token=ADMIN)
check(s["guest_tools_iso"] == TOOLS_ISO and s["guest_tools_retry_secs"] == 3,
      "console settings: guest tools CD, automatic install, in-session retry (3 s)")


def make_vm(vm_id, image, hw, memory, cpus, sound="none", video="std"):
    call("POST", f"/vms/{vm_id}/stop", token=ADMIN, ok=None)
    call("DELETE", f"/vms/{vm_id}", token=ADMIN, ok=None)
    call("POST", "/vms", {
        "id": vm_id, "name": vm_id, "hypervisor": "qemu", "architecture": "x86_64",
        "cpus": cpus, "memory": memory, "disk_size": hw.pop("disk_size"), "status": "stopped",
        "display": {"protocol": "vnc", "video": video, "sound": sound},
        "hardware": hw,
    }, token=ADMIN)
    t = time.time()
    _, vm = call("POST", f"/vms/{vm_id}/import-disk", {"source": image}, token=ADMIN, timeout=1800)
    check(vm.get("id") == vm_id, f"{vm_id}: disk imported through the API ({time.time() - t:.0f} s)")
    call("POST", f"/vms/{vm_id}/start", token=ADMIN, ok=(200, 202))


def tools_status(vm_id, token=None):
    return call("GET", f"/vms/{vm_id}/console/guest-tools", token=token or ADMIN)[1]


def wait_agent(vm_id, secs):
    end = time.time() + secs
    st = {}
    while time.time() < end:
        st = tools_status(vm_id)
        if st.get("guest_agent"):
            return st
        time.sleep(10)
    return st


def session_stats(vm_id):
    _, ss = call("GET", f"/console/{vm_id}/hxc/sessions", token=ADMIN)
    return ss[0]["stats"] if ss else {}


from playwright.sync_api import sync_playwright  # noqa: E402


def login_context(browser, user, pw):
    ctx = browser.new_context(ignore_https_errors=True, viewport={"width": 1280, "height": 900},
                              permissions=["clipboard-read", "clipboard-write"])
    r = ctx.request.post(BASE + "/api/auth/login", data={"username": user, "password": pw})
    assert r.ok, f"browser login {user}: {r.status}"
    return ctx


def open_console(ctx, vm_id, name):
    page = ctx.new_page()
    page.on("pageerror", lambda e: print(f"[{name} pageerror] {e}", flush=True))
    page.goto(f"{BASE}/api/console/{vm_id}/hxc")
    page.wait_for_function("window.hxc && (window.hxc.state === 'connected' || window.hxc.state === 'failed')",
                           timeout=90000)
    if page.evaluate("window.hxc.state") != "connected":
        page.screenshot(path=f"{OUT}/{name}-failed.png")
        check(False, f"{name}: console connects ({page.evaluate('window.hxc.lastError')})")
    page.wait_for_function("window.hxc.framesDecoded > 3", timeout=90000)
    return page


def wait_guest_video(page, vm_id, name, secs):
    end = time.time() + secs
    last = None
    while time.time() < end:
        src = page.evaluate("window.hxc.videoSource || null")
        gt = page.evaluate("window.hxc.guestTools || null")
        if gt != last:
            print(f"   {name}: guest tools {gt}", flush=True)
            last = gt
        if src == "guest":
            return True
        time.sleep(3)
    page.screenshot(path=f"{OUT}/{name}-no-guest-video.png")
    return False


def region_trial(page, x, y, action, timeout=10, size=64):
    if x is None:
        page.evaluate("window.hxc.watchRegion(0,0,window.hxc.guest.width,window.hxc.guest.height)")
    else:
        page.evaluate(f"window.hxc.watchRegion({x - size // 2},{y - size // 2},{size},{size})")
    page.evaluate(action)
    end = time.time() + timeout
    while time.time() < end:
        v = page.evaluate("window.hxc.regionChangeMs")
        if v is not None:
            return v
        time.sleep(0.03)
    page.evaluate("window.hxc.watch = null")
    return None


def key(code):
    return f"window.hxc.sendKey('{code}', true); window.hxc.sendKey('{code}', false);"


def combo(*codes):
    down = "".join(f"window.hxc.sendKey('{c}', true);" for c in codes)
    up = "".join(f"window.hxc.sendKey('{c}', false);" for c in reversed(codes))
    return down + up


def quiet(page, ms=1500, secs=30):
    end = time.time() + secs
    while time.time() < end:
        if page.evaluate("performance.now() - (window.hxc.lastChangeAt || 0)") > ms:
            return True
        time.sleep(0.2)
    return False


KEYCODE = {c: "Key" + c.upper() for c in "abcdefghijklmnopqrstuvwxyz"}
KEYCODE.update({str(d): "Digit" + str(d) for d in range(10)})
KEYCODE.update({" ": "Space", "-": "Minus", ".": "Period", "/": "Slash", "\n": "Enter"})


def type_text(page, text, measure=None, gap=0.12):
    for ch in text:
        act = key(KEYCODE[ch])
        if measure is not None and ch not in " \n":
            measure.append(region_trial(page, None, None, act, timeout=5))
        else:
            page.evaluate(act)
        time.sleep(gap)


def audio_stats(page):
    return page.evaluate("""async () => {
        const st = await window.hxc.pc.getStats(); let r = null;
        st.forEach(s => { if (s.type === 'inbound-rtp' && s.kind === 'audio')
            r = {packets: s.packetsReceived || 0, energy: s.totalAudioEnergy || 0, level: s.audioLevel || 0}; });
        return r; }""")


def video_stats(page):
    return page.evaluate("""async () => {
        const st = await window.hxc.pc.getStats(); let r = null;
        st.forEach(s => { if (s.type === 'inbound-rtp' && s.kind === 'video')
            r = {frames: s.framesDecoded, w: s.frameWidth, h: s.frameHeight, codec: s.codecId}; });
        return r; }""")


# ------------------------------------------------------------------ Windows
def windows(browser):
    vm = "hxg-win"
    make_vm(vm, WIN, {
        "disk_size": 64, "nets": [], "boot": ["disk"], "machine": "q35", "firmware": "ovmf",
        "secure_boot": True, "tpm": True, "guest_agent": True, "cpu_type": "host",
    }, memory=4096, cpus=4, sound="ich9-intel-hda", video="virtio")
    t0 = time.time()
    st = wait_agent(vm, 600)
    check(st.get("guest_agent"), f"windows: guest agent answers {time.time() - t0:.0f} s after start ({st.get('guest_os')})")
    check(st.get("tools_cd") and st.get("tools_port") and not st.get("in_use"),
          "windows: the node attached the guest tools CD and the guest-tools port; no tools yet")
    time.sleep(45)  # autologon + desktop
    admin = login_context(browser, "admin", "admin")
    page = open_console(admin, vm, "windows")
    t1 = time.time()
    got = wait_guest_video(page, vm, "windows", 600)
    page.screenshot(path=f"{OUT}/windows-guest-video.png")
    gts = page.evaluate("window.hxc.guestTools || null")
    check(got, f"windows: the node installed the guest tools through the guest agent and the open session "
               f"switched to the guest-encoded stream after {time.time() - t1:.0f} s ({gts})")
    st = tools_status(vm)
    check((st.get("last_install") or {}).get("state") == "installed" and st["last_install"]["by"] == "auto",
          f"windows: install recorded as automatic ({(st.get('last_install') or {}).get('report', {}).get('message')})")
    time.sleep(4)
    stats = session_stats(vm)
    RESULTS["windows_session"] = stats
    check(stats.get("video_source") == "guest" and stats.get("guest_encoder"),
          f"windows: session stats video_source=guest, guest_encoder={stats.get('guest_encoder')}, "
          f"tools={stats.get('guest_tools')}, guest encode {stats.get('guest_encode_ms')} ms")
    hello = page.evaluate("window.hxc.hello")
    check(hello.get("guest_tools") and hello.get("clipboard") and hello.get("resize") and hello.get("relative"),
          f"windows: the session re-announced the tools' channels (clipboard/resize/relative) to the client")
    g = page.evaluate("window.hxc.guest")
    vs = video_stats(page)
    RESULTS["windows_video"] = {"guest": g, "rtp": vs}
    print(f"   windows video {g} {vs}", flush=True)

    # keyboard: Notepad via Win+R, then typed text with latency
    page.evaluate(combo("MetaLeft", "KeyR"))
    time.sleep(2.5)
    type_text(page, "notepad\n")
    time.sleep(6)
    quiet(page, 1500, 20)
    lat = []
    typed = "hxc2b windows keys 42"
    type_text(page, typed, measure=lat, gap=0.25)
    page.screenshot(path=f"{OUT}/windows-notepad.png")
    check(len([x for x in lat if x is not None]) >= len(typed.replace(" ", "")) // 2,
          f"windows: keystrokes reach Notepad and come back in the guest-encoded picture "
          f"({len([x for x in lat if x is not None])}/{len(typed.replace(' ', ''))})", fatal=False)
    latency_summary("windows_guest_encoded_key_to_picture", lat)
    # clipboard guest -> browser: select all + copy in Notepad
    page.evaluate("window.hxc.guestClipboard = null")
    page.evaluate(combo("ControlLeft", "KeyA"))
    time.sleep(0.5)
    page.evaluate(combo("ControlLeft", "KeyC"))
    end = time.time() + 15
    while time.time() < end and not page.evaluate("window.hxc.guestClipboard"):
        time.sleep(0.3)
    gc = page.evaluate("window.hxc.guestClipboard")
    check(gc is not None and typed in gc, f"windows: guest clipboard reaches the browser ({gc!r})", fatal=False)
    # browser -> guest: paste over the selection, copy it back
    # (the tools never echo what the browser set, so the text cut back is
    # the pasted text plus typed characters)
    sent = "from the browser 7"
    page.evaluate(f"window.hxc.sendClipboard({json.dumps(sent)})")
    time.sleep(1.5)
    page.evaluate(combo("ControlLeft", "KeyA"))
    time.sleep(0.3)
    page.evaluate(combo("ControlLeft", "KeyV"))
    time.sleep(1)
    type_text(page, " ok")
    time.sleep(0.5)
    page.evaluate("window.hxc.guestClipboard = null")
    page.evaluate(combo("ControlLeft", "KeyA"))
    time.sleep(0.3)
    page.evaluate(combo("ControlLeft", "KeyX"))
    end = time.time() + 15
    while time.time() < end and not page.evaluate("window.hxc.guestClipboard"):
        time.sleep(0.3)
    back = page.evaluate("window.hxc.guestClipboard")
    check(back is not None and sent + " ok" in back,
          f"windows: browser clipboard reaches the guest (pasted into Notepad, cut back: {back!r})", fatal=False)
    stats = session_stats(vm)
    RESULTS["windows_clipboard"] = {"in": stats.get("clipboard_in"), "out": stats.get("clipboard_out")}

    # absolute pointer + buttons: a rubber band on the desktop
    # (Notepad closed first: Win+D shows the desktop)
    page.evaluate(combo("MetaLeft", "KeyD"))
    time.sleep(2)
    quiet(page, 1500, 20)
    abs_ms = None
    for _ in range(3):
        page.evaluate("window.hxc.move(300, 300)")
        time.sleep(0.5)
        page.evaluate("window.hxc.press(300, 300, 0)")
        time.sleep(0.3)
        abs_ms = region_trial(page, 420, 420, "window.hxc.move(520, 520)", timeout=15, size=200)
        page.evaluate("window.hxc.release(0)")
        if abs_ms is not None:
            break
        quiet(page, 1500, 10)
    check(abs_ms is not None, f"windows: absolute pointer + left button reach Windows (selection rectangle drawn, "
                              f"{abs_ms} ms to the picture)", fatal=False)
    # raw relative motion through the tools while the button is held
    quiet(page)
    page.evaluate("window.hxc.move(200, 200); window.hxc.press(200, 200, 0)")
    time.sleep(0.3)
    rel = []
    for _ in range(4):
        rel.append(region_trial(page, None, None, "window.hxc.relMove(60, 40)", timeout=5))
        time.sleep(0.3)
    page.evaluate("window.hxc.release(0)")
    stats = session_stats(vm)
    check(any(r is not None for r in rel) and stats.get("pointer_in", 0) > 0,
          f"windows: raw relative motion through the guest tools grows the selection ({rel})", fatal=False)
    page.evaluate(key("Escape"))

    # resize through the tools
    for (w, h) in ((1280, 720), (1280, 1024), (1024, 768)):
        page.evaluate("window.hxc.guestLayout = null")
        page.evaluate(f"window.hxc.resize({w}, {h})")
        end = time.time() + 20
        lay = None
        while time.time() < end:
            lay = page.evaluate("window.hxc.guestLayout")
            if lay:
                break
            time.sleep(0.5)
        time.sleep(3)
        g = page.evaluate("window.hxc.guest")
        print(f"   windows resize {w}x{h}: layout {lay} picture {g}", flush=True)
        if g.get("width") == w and g.get("height") == h:
            check(True, f"windows: resize through the guest tools: the guest switched to {w}x{h} and the "
                        f"guest-encoded stream follows ({lay})")
            RESULTS["windows_resize"] = {"asked": [w, h], "layout": lay, "picture": g}
            break
    else:
        check(False, "windows: resize through the guest tools", fatal=False)

    # audio (when the tools report it)
    if hello.get("audio"):
        a0 = audio_stats(page)
        time.sleep(5)
        a1 = audio_stats(page)
        RESULTS["windows_audio"] = {"before": a0, "after": a1}
        check(a1 and a1["packets"] > (a0 or {}).get("packets", 0) and a1["packets"] > 100,
              f"windows: the browser receives the guest's Opus audio track ({a0} -> {a1})", fatal=False)
    else:
        check(False, "windows: the guest tools report audio (WASAPI loopback)", fatal=False)
    stats = session_stats(vm)
    RESULTS["windows_session_end"] = stats
    print("   windows session " + json.dumps(stats), flush=True)
    page.close()
    admin.close()
    call("POST", f"/vms/{vm}/stop", token=ADMIN, ok=None)


# -------------------------------------------------------------------- Linux
def linux(browser):
    vm = "hxg-deb"
    make_vm(vm, LIN, {"disk_size": 6, "nets": [], "boot": ["disk"], "guest_agent": True}, memory=1536, cpus=2)
    t0 = time.time()
    st = wait_agent(vm, 300)
    check(st.get("guest_agent"), f"linux: guest agent answers {time.time() - t0:.0f} s after start ({st.get('guest_os')})")
    # RBAC: a view-only user may look but not install
    call("POST", "/roles", {"name": "HxgViewer", "description": "console view only",
                            "privileges": ["VM.Audit", "VM.Console"]}, token=ADMIN, ok=None)
    call("POST", "/users", {"username": "hxgview", "password": "Hxg-e2e-pass-1",
                            "email": "hxgview@errant.solutions"}, token=ADMIN, ok=None)
    call("POST", "/acl", {"path": "/vms", "subject_type": "user", "subject": "hxgview", "role": "HxgViewer"},
         token=ADMIN, ok=(200, 201, 409))
    viewer = login("hxgview", "Hxg-e2e-pass-1")
    stv = tools_status(vm, viewer)
    code, _ = call("POST", f"/vms/{vm}/console/guest-tools/install", {}, token=viewer, ok=None)
    check(stv.get("vm") == vm and code == 403,
          f"linux: a VM.Console-only user sees the guest tools state but the install is refused ({code})")
    t1 = time.time()
    code, st = call("POST", f"/vms/{vm}/console/guest-tools/install", {}, token=ADMIN, ok=(202,))
    check(st["last_install"]["state"] == "installing", "linux: install requested through the API (202, installing)")
    end = time.time() + 600
    while time.time() < end:
        st = tools_status(vm)
        if st["last_install"]["state"] != "installing":
            break
        time.sleep(5)
    li = st["last_install"]
    RESULTS["linux_install"] = li
    check(li["state"] == "installed",
          f"linux: the node installed the .deb from the tools CD with apt and started the service in "
          f"{time.time() - t1:.0f} s ({li.get('report', {}).get('message')}; "
          f"{(li.get('report') or {}).get('output', '')[-200:]!r})")
    _, audit = call("GET", "/audit/events?limit=200", token=ADMIN, ok=None)
    text = json.dumps(audit)
    check("GuestToolsInstall" in text and "guest tools installed" in text,
          "linux: the install is in the audit log (requested + result)", fatal=False)
    admin = login_context(browser, "admin", "admin")
    page = open_console(admin, vm, "linux")
    got = wait_guest_video(page, vm, "linux", 120)
    page.screenshot(path=f"{OUT}/linux-guest-video.png")
    stats = session_stats(vm)
    RESULTS["linux_session"] = stats
    check(got and stats.get("video_source") == "guest",
          f"linux: the browser shows the guest-encoded stream (video_source={stats.get('video_source')}, "
          f"guest_encoder={stats.get('guest_encoder')}, tools={stats.get('guest_tools')})")
    quiet(page)
    type_text(page, "root\n")
    time.sleep(3)
    quiet(page)
    lat = []
    type_text(page, "echo hxc2b linux 42", measure=lat, gap=0.25)
    page.evaluate(key("Enter"))
    page.screenshot(path=f"{OUT}/linux-typed.png")
    check(len([x for x in lat if x is not None]) >= 8,
          f"linux: keystrokes reach the guest and its reaction comes back in the guest-encoded picture "
          f"({len([x for x in lat if x is not None])}/{len(lat)})", fatal=False)
    latency_summary("linux_guest_encoded_key_to_picture", lat)
    page.close()
    # the UI's guest tools bar on the VM console page
    ui = admin.new_page()
    ui.goto(f"{BASE}/vms/{vm}/console")
    try:
        ui.wait_for_selector("#guest-tools-state", timeout=30000)
        ui.wait_for_function("!document.querySelector('#guest-tools-state').innerText.includes('checking')",
                             timeout=30000)
        time.sleep(2)
        txt = ui.inner_text("#guest-tools-state")
        note = ui.query_selector("#guest-tools-note")
        if note:
            txt += " / " + note.inner_text()
        btn = ui.query_selector("#install-guest-tools")
        check(btn is not None and ("in use" in txt or "installed" in txt),
              f"linux: the console page shows the guest tools bar ({txt!r}) with the Install button", fatal=False)
    except Exception as e:  # noqa: BLE001
        check(False, f"linux: console page guest tools bar ({e})", fatal=False)
    ui.screenshot(path=f"{OUT}/linux-console-page.png")
    admin.close()
    call("POST", f"/vms/{vm}/stop", token=ADMIN, ok=None)


with sync_playwright() as pw:
    browser = pw.chromium.launch(args=["--autoplay-policy=no-user-gesture-required"])
    if LIN and ONLY in ("", "linux"):
        linux(browser)
    if WIN and ONLY in ("", "windows"):
        windows(browser)
    browser.close()

dump()
print("RESULTS " + json.dumps({k: v for k, v in RESULTS.items() if "latency" in k or k.endswith("_ms")}), flush=True)
if FAILS:
    print(f"FAIL {len(FAILS)} check(s): {FAILS}", flush=True)
    sys.exit(1)
print("PASS Horcrux Console guest tools e2e", flush=True)
