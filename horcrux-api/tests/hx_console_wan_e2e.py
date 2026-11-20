#!/usr/bin/env python3
"""Horcrux Console phase C3 (WAN) end to end, driven by hx_console_wan_e2e.sh.

A real Alpine VM (QEMU + KVM, no guest tools) is viewed from a real
Chromium that runs inside its own network namespace; the veth link to the
node is shaped with netem per condition (RTT, loss, rate). Paths:

* LAN: direct ICE to the node's console sockets;
* WAN: through the node's built-in relay (TURN over UDP), forced and as an
  automatic fallback when direct UDP is blocked; TURN over TCP when UDP is
  blocked entirely;
* link adaptation: a rate-limited link and a configured uplink cap must
  bring the node's target bitrate under the limit while frames keep coming.

Per condition: keystroke-to-picture latency at the guest's shell (the
client sends the key, the guest echoes, the node encodes, the browser
decodes a changed frame), data-channel RTT, the ICE path the browser chose,
receiver stats (fps, loss, NACKs, jitter buffer, decode time) and the
node's link estimate/target. Printed as `LATENCY {...}` and `PATH {...}`
lines and `RESULTS {...}`.
"""
import json
import os
import ssl
import statistics
import subprocess
import time
import urllib.error
import urllib.request

NODE = "https://127.0.0.1:8006"
CLIENT_BASE = "https://10.77.0.1:8006"
NS = os.environ.get("HXC_NS", "hxcwan")
SUDO = os.environ.get("SUDO", "")
ALPINE_ISO = os.environ["HXC_ALPINE_ISO"]
OUT = os.environ.get("HXC_OUT", "/tmp/hxc-wan-e2e")
ONLY = [c for c in os.environ.get("HXC_WAN_CONDITIONS", "").split(",") if c]
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
RESULTS = {}
os.makedirs(OUT, exist_ok=True)


def call(method, path, body=None, token=None, ok=(200, 201, 202, 204)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(NODE + "/api" + path, data=data, method=method)
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


def run(argv, ns=False, check_rc=True):
    pre = ([SUDO] if SUDO else []) + (["ip", "netns", "exec", NS] if ns else [])
    r = subprocess.run(pre + argv, capture_output=True, text=True)
    if check_rc and r.returncode != 0:
        raise SystemExit(f"FAIL {' '.join(argv)}: {r.stderr.strip()}")
    return r


def shape(rtt_ms=0, loss=0.0, rate_kbit=None):
    """netem on both ends: half the RTT each way, loss each way; the rate
    limit applies to node -> client (the video direction)."""
    half = f"{rtt_ms / 2:g}ms"
    for dev, ns in (("hxwan0", False), ("hxwan1", True)):
        argv = ["tc", "qdisc", "replace", "dev", dev, "root", "netem", "delay", half]
        if loss:
            argv += ["loss", f"{loss:g}%"]
        if rate_kbit and not ns:
            argv += ["rate", f"{rate_kbit}kbit", "limit", "200"]
        run(argv, ns=ns)


def block(udp_ports):
    """Drop the client's outgoing UDP to these ports ('a:b' ranges)."""
    run(["iptables", "-F", "OUTPUT"], ns=True)
    for p in udp_ports or []:
        run(["iptables", "-A", "OUTPUT", "-p", "udp", "--dport", p, "-j", "DROP"], ns=True)


def login(user, pw):
    _, r = call("POST", "/auth/login", {"username": user, "password": pw}, ok=(200,))
    return r.get("ticket") or r.get("token")


ADMIN = login("admin", "admin")
_, BASE_SETTINGS = call("GET", "/console/settings", token=ADMIN)
check(BASE_SETTINGS["wan"]["enabled"] is False and BASE_SETTINGS["wan"]["relay_port"] == 3478,
      "WAN access is off by default (relay port 3478 when turned on)")
_, rs = call("GET", "/console/relay", token=ADMIN)
check(rs["running"] is False, "no relay runs while WAN access is off")


def settings(**wan):
    s = json.loads(json.dumps(BASE_SETTINGS))
    s["wan"].update(wan)
    _, saved = call("PUT", "/console/settings", s, token=ADMIN)
    return saved


VM = "hxc-wan"
call("POST", f"/vms/{VM}/stop", token=ADMIN, ok=None)
call("DELETE", f"/vms/{VM}", token=ADMIN, ok=None)
call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 2, "memory": 512, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "vnc", "video": "std", "sound": "none"},
    "hardware": {"nets": [], "cdrom": ALPINE_ISO, "boot": ["cdrom", "disk"]},
}, token=ADMIN)
call("POST", f"/vms/{VM}/start", token=ADMIN, ok=(200, 202))
boot_started = time.time()

from playwright.sync_api import sync_playwright  # noqa: E402


def key(code):
    return f"window.hxc.sendKey('{code}', true); window.hxc.sendKey('{code}', false);"


def code_of(c):
    return {" ": "Space", "-": "Minus", "/": "Slash", ".": "Period", "\n": "Enter", ";": "Semicolon"}.get(
        c, f"Digit{c}" if c.isdigit() else f"Key{c.upper()}")


SHIFTED = {">": "Period", "_": "Minus", "|": "Backslash"}


def keystroke(c):
    if c in SHIFTED:
        return "window.hxc.sendKey('ShiftLeft', true);" + key(SHIFTED[c]) + "window.hxc.sendKey('ShiftLeft', false);"
    return key(code_of(c))


def type_text(page, text):
    for c in text:
        page.evaluate(keystroke(c))
        time.sleep(0.06)


def wait_static(page, quiet_ms=1500, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        if page.evaluate("performance.now() - (window.hxc.lastChangeAt || 0)") > quiet_ms:
            return True
        time.sleep(0.2)
    return False


def trial(page, action, timeout=8):
    n = page.evaluate("window.hxc.inputToFrameCount")
    page.evaluate(action)
    end = time.time() + timeout
    while time.time() < end:
        if page.evaluate("window.hxc.inputToFrameCount") > n:
            return page.evaluate("window.hxc.inputToFrameMs[window.hxc.inputToFrameMs.length-1]")
        time.sleep(0.03)
    return None


CLIENT_STATS = """async () => {
  const st = await window.hxc.pc.getStats();
  let pair = null, inb = null;
  st.forEach(r => {
    if (r.type === 'transport' && r.selectedCandidatePairId) pair = st.get(r.selectedCandidatePairId);
    if (r.type === 'inbound-rtp' && r.kind === 'video') inb = r;
  });
  if (!pair) st.forEach(r => { if (r.type === 'candidate-pair' && r.nominated && r.state === 'succeeded') pair = r; });
  const lc = pair ? st.get(pair.localCandidateId) : null;
  const rc = pair ? st.get(pair.remoteCandidateId) : null;
  const n = (x) => (typeof x === 'number' && isFinite(x)) ? Math.round(x * 10) / 10 : null;
  return {
    local_candidate: lc ? lc.candidateType : null, relay_protocol: lc ? (lc.relayProtocol || null) : null,
    remote_candidate: rc ? rc.address + ':' + rc.port : null,
    ice_rtt_ms: pair ? n(pair.currentRoundTripTime * 1000) : null,
    fps: inb ? n(inb.framesPerSecond) : null, frames_decoded: inb ? inb.framesDecoded : null,
    packets_lost: inb ? inb.packetsLost : null, packets_received: inb ? inb.packetsReceived : null,
    nacks_sent: inb ? inb.nackCount : null, plis_sent: inb ? inb.pliCount : null,
    frames_dropped: inb ? inb.framesDropped : null,
    jitter_buffer_ms: inb && inb.jitterBufferEmittedCount ? n(inb.jitterBufferDelay / inb.jitterBufferEmittedCount * 1000) : null,
    decode_ms: inb && inb.framesDecoded ? n(inb.totalDecodeTime / inb.framesDecoded * 1000) : null,
    width: inb ? inb.frameWidth : null, height: inb ? inb.frameHeight : null,
  };
}"""


def summary(v):
    v = [x for x in v if x is not None]
    if not v:
        return None
    v.sort()
    return {"samples": len(v), "median_ms": round(statistics.median(v), 1), "min_ms": round(v[0], 1),
            "p90_ms": round(v[min(len(v) - 1, int(len(v) * 0.9))], 1), "max_ms": round(v[-1], 1)}


def wait_allocations(n, timeout=15):
    end = time.time() + timeout
    while time.time() < end:
        _, rs = call("GET", "/console/relay", token=ADMIN)
        if rs.get("allocations", -1) == n:
            return rs
        time.sleep(0.5)
    return rs


CONDITIONS = [
    # name, netem (rtt ms, loss %, rate kbit), wan settings, blocked UDP ports, expectation
    ("lan_direct", (0, 0, None), dict(enabled=False), None, dict(path="direct")),
    ("wan_relay_rtt20", (20, 0, None), dict(enabled=True, force_relay=True), None, dict(path="relay", proto="udp")),
    ("wan_relay_rtt50_loss1", (50, 1, None), dict(enabled=True, force_relay=True), None, dict(path="relay", proto="udp")),
    ("wan_relay_rtt100_loss2", (100, 2, None), dict(enabled=True, force_relay=True), None, dict(path="relay", proto="udp")),
    ("wan_fallback_rtt40", (40, 0, None), dict(enabled=True, force_relay=False), ["61200:61299"], dict(path="relay", proto="udp")),
    ("wan_relay_tcp_rtt40", (40, 0, None), dict(enabled=True, force_relay=True), ["3478", "61200:61299"], dict(path="relay", proto="tcp")),
    ("wan_rate3mbit_rtt40", (40, 0, 3000), dict(enabled=True, force_relay=True), None, dict(path="relay", max_kbps=3000, flood=True)),
    ("wan_uplink1000_rtt20", (20, 0, None), dict(enabled=True, force_relay=True, uplink_kbps=1000), None, dict(path="relay", max_kbps=1000, limit="uplink", flood=True)),
]


with sync_playwright() as pw:
    real = pw.chromium.executable_path
    wrapper = os.path.join(OUT, "chromium-in-netns.sh")
    with open(wrapper, "w") as f:
        f.write(f"#!/bin/sh\nexec {SUDO} ip netns exec {NS} \"{real}\" \"$@\"\n")
    os.chmod(wrapper, 0o755)
    browser = pw.chromium.launch(executable_path=wrapper, args=["--autoplay-policy=no-user-gesture-required"])
    ctx = browser.new_context(ignore_https_errors=True, viewport={"width": 1280, "height": 860})
    r = ctx.request.post(CLIENT_BASE + "/api/auth/login", data={"username": "admin", "password": "admin"})
    check(r.ok, "browser in the client namespace logs in to the node over the veth link")

    def open_console(name):
        page = ctx.new_page()
        page.on("pageerror", lambda e: print(f"[{name} pageerror] {e}", flush=True))
        page.goto(f"{CLIENT_BASE}/api/console/{VM}/hxc")
        try:
            page.wait_for_function("window.hxc && (window.hxc.state === 'connected' || window.hxc.state === 'failed')",
                                   timeout=90000)
            page.wait_for_function("window.hxc.framesDecoded > 3", timeout=60000)
        except Exception:
            page.screenshot(path=f"{OUT}/{name}-timeout.png")
            print("DEBUG", page.evaluate("""() => JSON.stringify({state: window.hxc && window.hxc.state,
                err: window.hxc && window.hxc.lastError, ice: window.hxc && window.hxc.pc && window.hxc.pc.iceConnectionState,
                servers: window.hxc && window.hxc.iceServers && window.hxc.iceServers.map(s => s.urls)})"""), flush=True)
            raise
        check(page.evaluate("window.hxc.state") == "connected",
              f"{name}: console connected ({page.evaluate('window.hxc.lastError')})")
        return page

    # Log in on the guest console once (LAN), stop the cursor blink so a
    # picture change means the input arrived.
    shape(0, 0)
    block(None)
    settings(enabled=False)
    page = open_console("setup")
    left = int(os.environ.get("HXC_ALPINE_BOOT_SECS", "40")) - (time.time() - boot_started)
    if left > 0:
        time.sleep(left)
    type_text(page, "root\n")
    time.sleep(3)
    type_text(page, "echo 0 >/sys/class/graphics/fbcon/cursor_blink\n")
    check(wait_static(page, quiet_ms=2000, timeout=30), "logged in on the Alpine console, picture still")
    page.screenshot(path=f"{OUT}/setup.png")
    page.close()

    for name, (rtt, loss, rate), wan, blocked, want in CONDITIONS:
        if ONLY and name not in ONLY:
            continue
        print(f"---- {name}: rtt {rtt} ms, loss {loss}%, rate {rate or '-'} kbit/s, wan {wan}, blocked {blocked}", flush=True)
        saved = settings(**wan)
        if wan.get("enabled"):
            _, rs = call("GET", "/console/relay", token=ADMIN)
            check(rs["running"], f"{name}: the relay runs ({rs.get('listen')})")
        shape(rtt, loss, rate)
        block(blocked)
        page = open_console(name)
        ice = page.evaluate("window.hxc.iceServers")
        if wan.get("enabled"):
            check(ice and any("turn:10.77.0.1:3478" in u for u in ice[0]["urls"]) and ice[0]["username"],
                  f"{name}: the ticket carries the node's relay with session credentials")
        time.sleep(2)
        wait_static(page, quiet_ms=800, timeout=20)
        lat = []
        for i in range(14):
            wait_static(page, quiet_ms=500, timeout=10)
            lat.append(trial(page, key("KeyA" if i % 2 == 0 else "Backspace")))
        got = len([x for x in lat if x is not None])
        check(got >= 10, f"{name}: keystrokes reach the guest and the echo comes back ({got}/14)")
        time.sleep(2.5)
        cs = page.evaluate(CLIENT_STATS)
        link = page.evaluate("window.hxc.link") or {}
        server = page.evaluate("window.hxc.server") or {}
        rtt_dc = page.evaluate("window.hxc.rttMs")
        path = link.get("path") or server.get("path")
        check(path == want["path"], f"{name}: the node sees the {want['path']} path (got {path})")
        want_local = "relay" if want["path"] == "relay" else "host"
        check(cs["local_candidate"] == want_local,
              f"{name}: the browser's selected candidate is {want_local} (got {cs['local_candidate']}, {cs['relay_protocol']})")
        if want.get("proto"):
            check(cs["relay_protocol"] == want["proto"], f"{name}: relay reached over {want['proto']}")
        res = {"conditions": {"rtt_ms": rtt, "loss_pct": loss, "rate_kbit": rate, "blocked_udp": blocked},
               "path": path, "keystroke_to_picture": summary(lat), "datachannel_rtt_ms": rtt_dc and round(rtt_dc, 1),
               "client": cs, "node": {k: server.get(k) for k in ("estimate_kbps", "target", "rtt_ms", "loss_pct", "nacks", "encode_ms", "codec", "encoder")}}
        if want.get("flood"):
            # fill the screen continuously so the encoder needs more than the link allows
            type_text(page, "while true; do cat /proc/cpuinfo; done\n")
            ok = False
            end = time.time() + 60
            target = None
            while time.time() < end:
                time.sleep(1)
                link = page.evaluate("window.hxc.link") or {}
                target = link.get("target")
                if target and target["kbps"] <= want["max_kbps"] and (not want.get("limit") or target["limit"] == want["limit"]):
                    ok = True
                    break
            frames0 = page.evaluate("window.hxc.framesDecoded")
            time.sleep(3)
            frames1 = page.evaluate("window.hxc.framesDecoded")
            cs2 = page.evaluate(CLIENT_STATS)
            res["adapted"] = {"target": target, "estimate_kbps": link.get("estimate_kbps"),
                              "fps_while_flooding": round((frames1 - frames0) / 3.0, 1), "client": cs2}
            page.evaluate("window.hxc.sendKey('ControlLeft', true);" + key("KeyC") + "window.hxc.sendKey('ControlLeft', false);")
            time.sleep(0.5)
            type_text(page, "clear\n")
            check(ok, f"{name}: adaptation brings the target to {target} (limit {want.get('limit', 'link')}, at most {want['max_kbps']} kbit/s)")
            check(frames1 - frames0 >= 5, f"{name}: frames keep arriving while adapted ({frames1 - frames0} in 3 s)")
        page.screenshot(path=f"{OUT}/{name}.png")
        RESULTS[name] = res
        print("PATH " + json.dumps({name: res}), flush=True)
        if res["keystroke_to_picture"]:
            print("LATENCY " + json.dumps({name: res["keystroke_to_picture"]}), flush=True)
        page.close()
        if wan.get("enabled"):
            rs = wait_allocations(0)
            check(rs["allocations"] == 0, f"{name}: the session's relay allocations are gone after it closed ({rs['allocations']})")
            check(rs["auth_failures"] == 0 and rs["forbidden_peers"] == 0,
                  f"{name}: relay counters clean (auth failures {rs['auth_failures']}, forbidden peers {rs['forbidden_peers']})")

    # A relay credential stops working once its session has ended (revoked).
    settings(enabled=True, force_relay=True)
    shape(0, 0)
    block(None)
    page = open_console("revoke")
    servers = page.evaluate("window.hxc.iceServers")
    session = page.evaluate("window.hxc.session")
    page.close()
    wait_allocations(0)
    probe = ctx.new_page()
    probe.goto(f"{CLIENT_BASE}/api/console/{VM}/hxc/status")
    reused = probe.evaluate("""async (servers) => {
      const pc = new RTCPeerConnection({iceServers: servers, iceTransportPolicy: 'relay'});
      pc.createDataChannel('x');
      const cands = [];
      pc.onicecandidate = (e) => { if (e.candidate) cands.push(e.candidate.type); };
      await pc.setLocalDescription(await pc.createOffer());
      await new Promise(r => setTimeout(r, 4000));
      pc.close();
      return cands;
    }""", servers)
    check("relay" not in reused, f"credentials of an ended session ({session}) get no relay allocation (candidates {reused})")
    probe.close()
    settings(enabled=False)
    _, rs = call("GET", "/console/relay", token=ADMIN)
    check(rs["running"] is False, "turning WAN access off stops the relay")
    browser.close()

shape(0, 0)
block(None)
print("RESULTS " + json.dumps(RESULTS), flush=True)
with open(os.path.join(OUT, "results.json"), "w") as f:
    json.dump(RESULTS, f, indent=1)
