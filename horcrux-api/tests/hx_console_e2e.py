#!/usr/bin/env python3
"""Horcrux Console (phase C1) end to end, driven by hx_console_e2e.sh.

Real QEMU guests started through the API with the D-Bus display, viewed in
a real Chromium through the node's WebRTC client page:

* Linux desktop VM (Tiny Core Linux ISO, no guest tools): the picture
  arrives before any OS runs (ISOLINUX boot prompt), keyboard input echoes
  there, then the X desktop answers a mouse click (menu opens);
* optional Windows VM (HXC_WIN_ISO, no tools): Windows Setup renders and
  answers keyboard input;
* RBAC: a view-only user (VM.Console without VM.Console.Input) sees the
  picture but its input is refused and changes nothing; a user without
  VM.Console gets no ticket (403); a ticket only works on the signalling
  connection and for the user it was issued to, once;
* the classic console (noVNC page) still works next to it, the settings
  API validates and persists, sessions are listed and audited.

Latency: input-to-picture (client sends input -> changed frame decoded in
the browser) and data-channel RTT, printed as `LATENCY {...}` lines.
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
ISO = os.environ["HXC_LINUX_ISO"]
WIN_ISO = os.environ.get("HXC_WIN_ISO", "")
OUT = os.environ.get("HXC_OUT", "/tmp/hxc-e2e")
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
RESULTS = {}


def call(method, path, body=None, token=None, ok=(200, 201, 202, 204)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + "/api" + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=180) as r:
            status, text = r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if ok is not None and status not in ok:
        raise SystemExit(f"FAIL {method} {path} -> {status}: {text[:500]}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL " + what)
    print("ok -", what, flush=True)


def login(user, pw):
    _, r = call("POST", "/auth/login", {"username": user, "password": pw}, ok=(200,))
    return r.get("ticket") or r.get("token")


ADMIN = login("admin", "admin")


def make_vm(vm_id, iso, memory, cpus=2, extra_hw=None):
    call("POST", f"/vms/{vm_id}/stop", token=ADMIN, ok=None)
    call("DELETE", f"/vms/{vm_id}", token=ADMIN, ok=None)
    hw = {"nets": [], "cdrom": iso, "boot": ["cdrom", "disk"]}
    hw.update(extra_hw or {})
    call("POST", "/vms", {
        "id": vm_id, "name": vm_id, "hypervisor": "qemu", "architecture": "x86_64",
        "cpus": cpus, "memory": memory, "disk_size": 1, "status": "stopped",
        "display": {"protocol": "vnc", "video": "std", "sound": "none"},
        "hardware": hw,
    }, token=ADMIN)
    call("POST", f"/vms/{vm_id}/start", token=ADMIN, ok=(200, 202))


def qemu_cmdline(vm_id):
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            argv = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")
        except OSError:
            continue
        if argv and b"qemu-system" in argv[0] and any(vm_id.encode() in a for a in argv):
            return [a.decode(errors="replace") for a in argv]
    return []


# ---------------------------------------------------------------- settings
_, s = call("GET", "/console/settings", token=ADMIN)
check(s["enabled"] and s["ticket_lifetime_secs"] == 60 and s["codecs"] == ["h264", "vp8"],
      "console settings have their documented defaults")
st, err = call("PUT", "/console/settings", dict(s, codecs=["mpeg2"]), token=ADMIN, ok=None)
check(st in (400, 422) and "mpeg2" in json.dumps(err), "invalid settings are refused with the reason")
s2 = dict(s, desktop=dict(s["desktop"], max_fps=30), session_idle_timeout_secs=45)
_, saved = call("PUT", "/console/settings", s2, token=ADMIN)
_, again = call("GET", "/console/settings", token=ADMIN)
check(again["session_idle_timeout_secs"] == 45, "settings changes persist (GET returns them)")

# ---------------------------------------------------------------- users
call("POST", "/roles", {"name": "HxcViewer", "description": "console view only",
                        "privileges": ["VM.Audit", "VM.Console"]}, token=ADMIN, ok=None)
for u in ("hxcview", "hxcnone"):
    call("POST", "/users", {"username": u, "password": "Hxc-e2e-pass-1",
                            "email": f"{u}@errant.solutions"}, token=ADMIN, ok=None)
call("POST", "/acl", {"path": "/vms", "subject_type": "user", "subject": "hxcview", "role": "HxcViewer"},
     token=ADMIN, ok=(200, 201, 409))
_, roles = call("GET", "/roles", token=ADMIN)
check(any(r.get("name") == "HxcViewer" for r in roles), "a view-only role exists for the RBAC checks")

# ---------------------------------------------------------------- Linux VM
VM = "hxc-linux"
make_vm(VM, ISO, 768)
argv = " ".join(qemu_cmdline(VM))
check("-display dbus,p2p=yes" in argv, "the VM runs with QEMU's D-Bus display")
check("-vnc 127.0.0.1:" in argv, "the classic VNC console is still configured")
_, avail = call("GET", f"/console/{VM}/hxc/status", token=ADMIN)
check(avail["available"], f"console status: available ({avail['encoder']})")
RESULTS["encoder_caps"] = avail["encoder"]

from playwright.sync_api import sync_playwright  # noqa: E402

os.makedirs(OUT, exist_ok=True)


def login_context(browser, user, pw):
    ctx = browser.new_context(ignore_https_errors=True, viewport={"width": 1280, "height": 860})
    r = ctx.request.post(BASE + "/api/auth/login", data={"username": user, "password": pw})
    assert r.ok, f"browser login {user}: {r.status}"
    return ctx


def open_console(ctx, vm_id, name):
    page = ctx.new_page()
    page.on("console", lambda m: print(f"[{name} console] {m.type}: {m.text}", flush=True))
    page.on("pageerror", lambda e: print(f"[{name} pageerror] {e}", flush=True))
    page.goto(f"{BASE}/api/console/{vm_id}/hxc")
    try:
        page.wait_for_function("window.hxc && (window.hxc.state === 'connected' || window.hxc.state === 'failed')",
                               timeout=60000)
    except Exception:
        page.screenshot(path=f"{OUT}/{name}-timeout.png")
        print("DEBUG hxc", page.evaluate("""() => JSON.stringify({state: window.hxc && window.hxc.state,
            err: window.hxc && window.hxc.lastError, conn: window.hxc && window.hxc.connId,
            session: window.hxc && window.hxc.session, frames: window.hxc && window.hxc.framesDecoded,
            ice: window.hxc && window.hxc.pc && window.hxc.pc.iceConnectionState,
            pcs: window.hxc && window.hxc.pc && window.hxc.pc.connectionState,
            body: document.body.innerText.slice(0, 400)})"""), flush=True)
        raise
    state = page.evaluate("window.hxc.state")
    if state != "connected":
        page.screenshot(path=f"{OUT}/{name}-failed.png")
        raise SystemExit(f"FAIL {name}: console did not connect: {page.evaluate('window.hxc.lastError')}")
    page.wait_for_function("window.hxc.framesDecoded > 3", timeout=60000)
    return page


def wait_static(page, quiet_ms=1500, timeout=120):
    """Until no frame change for quiet_ms (the guest stopped drawing)."""
    end = time.time() + timeout
    while time.time() < end:
        q = page.evaluate("performance.now() - (window.hxc.lastChangeAt || 0)")
        if q > quiet_ms:
            return True
        time.sleep(0.2)
    return False


def trial(page, action, timeout=10):
    """Run `action` (JS) and return the input-to-picture latency in ms, or None."""
    n = page.evaluate("window.hxc.inputToFrameMs.length")
    page.evaluate(action)
    end = time.time() + timeout
    while time.time() < end:
        if page.evaluate("window.hxc.inputToFrameMs.length") > n:
            return page.evaluate("window.hxc.inputToFrameMs[window.hxc.inputToFrameMs.length-1]")
        time.sleep(0.05)
    return None


def key(code):
    return f"window.hxc.sendKey('{code}', true); window.hxc.sendKey('{code}', false);"


def latency_summary(name, values):
    v = [x for x in values if x is not None]
    if not v:
        return None
    d = {"samples": len(v), "median_ms": round(statistics.median(v), 1),
         "min_ms": round(min(v), 1), "max_ms": round(max(v), 1)}
    print("LATENCY " + json.dumps({name: d}), flush=True)
    RESULTS[name] = d
    return d


with sync_playwright() as pw:
    browser = pw.chromium.launch(args=["--autoplay-policy=no-user-gesture-required"])
    admin = login_context(browser, "admin", "admin")
    page = open_console(admin, VM, "linux")
    hello = page.evaluate("window.hxc.hello")
    check(hello["input"] is True and hello["codec"] in ("h264", "vp8"),
          f"admin session: input allowed, codec {hello['codec']} via {hello['encoder']}")
    RESULTS["codec"], RESULTS["encoder"] = hello["codec"], hello["encoder"]
    page.screenshot(path=f"{OUT}/linux-preos.png")
    check(page.evaluate("window.hxc.guest.width") >= 640, "pre-OS screen (boot loader) is visible: "
          + str(page.evaluate("window.hxc.guest")))

    # Pre-OS keyboard: type at the ISOLINUX "boot:" prompt, it echoes.
    wait_static(page)
    pre = []
    for c in ["KeyA", "Backspace", "KeyB", "Backspace", "KeyC", "Backspace", "KeyD", "Backspace"]:
        pre.append(trial(page, key(c)))
        time.sleep(0.3)
    got = [x for x in pre if x is not None]
    check(len(got) >= 6, f"keyboard input reaches the guest before any OS runs ({len(got)}/8 keystrokes changed the screen)")
    latency_summary("preos_key_input_to_picture", pre)

    # Boot Tiny Core and wait for its X desktop.
    page.evaluate(key("Enter"))
    time.sleep(20)
    if not wait_static(page, quiet_ms=6000, timeout=240):
        print("note: desktop kept changing; continuing", flush=True)
    page.screenshot(path=f"{OUT}/linux-desktop.png")
    g = page.evaluate("window.hxc.guest")
    # Pointer motion: the guest draws its own (software) cursor into the
    # framebuffer, so every move to a new place changes the picture.
    moves = []
    for i, (fx, fy) in enumerate([(0.2, 0.2), (0.8, 0.25), (0.75, 0.7), (0.25, 0.75), (0.5, 0.3), (0.3, 0.5)]):
        x, y = int(g["width"] * fx), int(g["height"] * fy)
        moves.append(trial(page, f"window.hxc.move({x},{y})"))
        wait_static(page, quiet_ms=600, timeout=10)
    got = len([m for m in moves if m is not None])
    check(got >= 4, f"pointer motion reaches the desktop ({got}/6 moves moved the guest's cursor)")
    latency_summary("desktop_pointer_input_to_picture", moves)
    page.screenshot(path=f"{OUT}/linux-moved.png")

    # Click: the dock's last icon (bottom centre) opens a terminal window.
    clicks = []
    for i, dx in enumerate([102, 100, 104]):
        x, y = g["width"] // 2 + dx, g["height"] - 22
        page.evaluate(f"window.hxc.move({x},{y})")
        wait_static(page, quiet_ms=1200, timeout=15)  # the dock's hover zoom settles
        lat = trial(page, f"window.hxc.press({x},{y},0)", timeout=2)
        time.sleep(0.15)
        lat2 = trial(page, "window.hxc.release(0)", timeout=8)
        clicks.append(lat if lat is not None else lat2)
        if clicks[-1] is not None:
            break
    page.screenshot(path=f"{OUT}/linux-click.png")
    check(any(c is not None for c in clicks), f"a mouse click reaches the desktop (dock icon opened a window: {clicks})")
    latency_summary("desktop_click_input_to_picture", clicks)
    time.sleep(2.5)
    rtt = page.evaluate("window.hxc.rttMs")
    server = page.evaluate("window.hxc.server")
    check(rtt is not None and server and server["frames"] > 10, f"stats: rtt {rtt:.1f} ms, server {server}")
    RESULTS["datachannel_rtt_ms"] = round(rtt, 2)
    RESULTS["encode_ms_avg"] = round(server["encode_ms"], 2)
    RESULTS["guest"] = g

    # Sessions are listed with live numbers.
    _, sess = call("GET", f"/console/{VM}/hxc/sessions", token=ADMIN)
    check(len(sess) == 1 and sess[0]["stats"]["connected"] and sess[0]["stats"]["keys_in"] > 0,
          "the session is listed with its counters")

    # Classic console next to it.
    st, html = call("GET", f"/console/{VM}/novnc", token=ADMIN, ok=None)
    check(st == 200 and "<html" in str(html).lower(), "the classic noVNC console page still works while the Horcrux Console is open")

    # Ticket binding: a ticket for connection A is refused on connection B,
    # and works once on A.
    res = page.evaluate("""async (vm) => {
      const open = () => new Promise((res, rej) => {
        const ws = new WebSocket('wss://' + location.host + '/api/ws');
        ws.onopen = () => ws.send(JSON.stringify({type: 'console.hello'}));
        ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.type === 'ConsoleHello') res({ws, id: m.data.conn_id}); };
        ws.onerror = rej;
      });
      const ask = (c, msg) => new Promise((res) => {
        c.ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.type.startsWith('Console')) res(m); };
        c.ws.send(JSON.stringify(msg));
      });
      const a = await open(), b = await open();
      const t = await (await fetch('/api/console/' + vm + '/hxc/ticket', {method: 'POST',
        headers: {'Content-Type': 'application/json'}, body: JSON.stringify({conn_id: a.id})})).json();
      const pc = new RTCPeerConnection(); pc.addTransceiver('video', {direction: 'recvonly'}); pc.createDataChannel('ctl');
      await pc.setLocalDescription(await pc.createOffer());
      const wrong = await ask(b, {type: 'console.offer', ticket: t.ticket, sdp: pc.localDescription.sdp});
      const right = await ask(a, {type: 'console.offer', ticket: t.ticket, sdp: pc.localDescription.sdp});
      const reuse = await ask(a, {type: 'console.offer', ticket: t.ticket, sdp: pc.localDescription.sdp});
      pc.close();
      if (right.type === 'ConsoleAnswer') a.ws.send(JSON.stringify({type: 'console.stop', session: right.data.session}));
      return {wrong: wrong.data.message || '', right: right.type, reuse: reuse.data.message || ''};
    }""", VM)
    check("another signalling connection" in res["wrong"], "a ticket is refused on another signalling connection")
    check(res["right"] == "ConsoleAnswer", "the same ticket works on its own connection")
    check("already used" in res["reuse"], "a ticket works only once")

    # View-only user: picture yes, input refused (and nothing changes).
    viewer = login_context(browser, "hxcview", "Hxc-e2e-pass-1")
    vpage = open_console(viewer, VM, "viewer")
    check(vpage.evaluate("window.hxc.hello.input") is False, "a VM.Console-only user gets a view-only session")
    wait_static(vpage, quiet_ms=1500, timeout=30)
    lat = trial(vpage, f"window.hxc.move(10,10); window.hxc.click({g['width']//2},{g['height']//2},2)", timeout=4)
    time.sleep(1)
    check(lat is None, "the view-only user's click changed nothing in the guest")
    check(len(vpage.evaluate("window.hxc.denied")) >= 1, "the view-only user is told input needs VM.Console.Input")
    vpage.close()
    nobody = login_context(browser, "hxcnone", "Hxc-e2e-pass-1")
    r = nobody.request.post(f"{BASE}/api/console/{VM}/hxc/ticket", data={"conn_id": "x"})
    check(r.status == 403, f"a user without VM.Console gets no ticket ({r.status})")

    # UI: the VM console page opens the Horcrux Console first; Settings page.
    if os.environ.get("HXC_UI") == "1":
        ui = admin.new_page()
        ui.goto(f"{BASE}/vms/{VM}/console")
        ui.wait_for_selector("#hxc-frame", timeout=30000)
        check(ui.locator("#console-type").input_value() == "horcrux", "UI console page opens the Horcrux Console first")
        ui.goto(f"{BASE}/vms/{VM}/console?classic=1")
        ui.wait_for_selector("#console-frame", timeout=30000)
        check(True, "UI ?classic=1 opens the classic VNC console")
        ui.goto(f"{BASE}/system/console")
        ui.wait_for_selector("#hxc-settings", timeout=30000)
        ui.fill("input[name=session_idle_timeout_secs]", "40")
        ui.locator("input[name=session_idle_timeout_secs]").dispatch_event("change")
        ui.click("#hxc-settings-save")
        ui.wait_for_selector("#hxc-settings-notice", timeout=10000)
        _, after = call("GET", "/console/settings", token=ADMIN)
        check(after["session_idle_timeout_secs"] == 40, "UI Settings > Horcrux Console saves through the API")
        ui.screenshot(path=f"{OUT}/ui-settings.png")
        ui.close()

    page.close()
    time.sleep(3)
    _, audit = call("GET", "/audit/events", token=ADMIN, ok=None)
    text = json.dumps(audit)
    check("ConsoleSessionStarted" in text and "ConsoleAccessDenied" in text,
          "console sessions and refused tickets are in the audit log")

    # ------------------------------------------------------------ Windows VM
    if WIN_ISO:
        # one guest at a time: the CI job container has a 6 GiB cap
        call("POST", f"/vms/{VM}/stop", token=ADMIN, ok=None)
        WVM = "hxc-windows"
        make_vm(WVM, WIN_ISO, int(os.environ.get("HXC_WIN_MEM_MB", "3072")), cpus=2)
        wpage = open_console(admin, WVM, "windows")
        # Windows Setup takes a while to draw its first window.
        deadline = time.time() + 600
        while time.time() < deadline:
            time.sleep(10)
            wpage.screenshot(path=f"{OUT}/windows-latest.png")
            if wait_static(wpage, quiet_ms=8000, timeout=20) and wpage.evaluate("window.hxc.guest.width") >= 800:
                break
        wpage.screenshot(path=f"{OUT}/windows-setup.png")
        wins = []
        for i in range(6):
            wins.append(trial(wpage, key("Tab")))
            time.sleep(0.8)
        check(len([x for x in wins if x is not None]) >= 3,
              f"keyboard input reaches Windows Setup ({len([x for x in wins if x is not None])}/6 Tab presses moved the focus)")
        latency_summary("windows_key_input_to_picture", wins)
        RESULTS["windows_guest"] = wpage.evaluate("window.hxc.guest")
        wpage.close()
        call("POST", f"/vms/{WVM}/stop", token=ADMIN, ok=None)
    browser.close()

call("POST", f"/vms/{VM}/stop", token=ADMIN, ok=None)
json.dump(RESULTS, open(f"{OUT}/results.json", "w"), indent=1)
print("RESULTS " + json.dumps(RESULTS), flush=True)
print("PASS hx_console_e2e", flush=True)
