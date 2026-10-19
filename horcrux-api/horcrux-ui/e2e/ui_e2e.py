#!/usr/bin/env python3
"""Browser end-to-end test of the Horcrux web UI against a real horcrux-api.

Runs in CI (`UI E2E` job in .forgejo/workflows/ci.yml): horcrux-api serves the
trunk-built UI on BASE_URL and talks to the host's real Docker daemon. Every
check drives the UI the way a user does (headless Chromium via Playwright)
and then verifies the effect independently with the docker CLI, so a button
that "does nothing" or a list that lies fails the run.

Covers the daily-QA findings:
  * "Create Container" button did nothing (no handler, no route)
  * containers page crashed on `missing field image`; Created showed N/A
  * dashboard "missing field cpu_usage"
  * events feed printed raw JSON blobs
  * stack services had no shared network / DNS; ports were dropped;
    removed stacks lingered until a manual refresh
  * /favicon.ico 404 on every page
  * Monitoring > Notifications and Metrics Explorer called routes that did
    not exist (a channel test must really reach a webhook receiver; the
    explorer must chart real samples)

It also covers access control end to end over HTTPS (the browser trusts the node's
local CA, so WebAuthn runs in a real secure context):
  * login page, authenticator-app (TOTP) enrollment from the Account
    Security page, then a login that asks for the code
  * security key enrollment and login with Chromium's virtual authenticator
  * permission grants, groups, realms and the certificate page
"""
import base64
import hashlib
import hmac
import http.server
import json
import os
import ssl
import struct
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
# Trust the node's local CA when given (CI), never skip verification silently.
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
TFA_USER = "e2e-tfa-user"
TFA_PASS = "e2e-tfa-pass-1"


def urlopen(req):
    return urllib.request.urlopen(req, context=SSL)


def totp_code(secret_b32, t=None):
    """RFC 6238 code (SHA1, 6 digits, 30 s) - what an authenticator app shows."""
    key = base64.b32decode(secret_b32 + "=" * (-len(secret_b32) % 8))
    counter = int((t or time.time()) // 30)
    h = hmac.new(key, struct.pack(">Q", counter), hashlib.sha1).digest()
    o = h[19] & 15
    return "%06d" % ((struct.unpack(">I", h[o:o + 4])[0] & 0x7FFFFFFF) % 1000000)
CT = "e2e-ui-ct"
STACK = "e2e-ui-stack"
EV_CT = "e2e-ui-events"
SDN_NET = "e2esdn0"
REP_VM = "e2e-rep-vm"
REP_CLONE = "e2e-rep-linked"
BK_VM = "e2e-bk-restored"
BK_JOB = "e2e-bk-job"
DISP_VM = "e2e-display-vm"
NOTIFY = "e2e-notify-hook"
ALERT_RULE = "e2e-alert-rule"
_TOKEN = []


def token():
    if not _TOKEN:
        req = urllib.request.Request(f"{BASE}/api/auth/login",
                                     data=json.dumps({"username": "admin", "password": "admin"}).encode(),
                                     headers={"Content-Type": "application/json"})
        _TOKEN.append(json.load(urlopen(req))["ticket"])
    return _TOKEN[0]


def _req(method, path, tok, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(f"{BASE}{path}", data=data, method=method,
                               headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json"})
    try:
        with urlopen(r) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        if method == "DELETE":
            return None
        raise AssertionError(f"{method} {path} -> {e.code}: {e.read()[:300]}")


def api_get(path, tok):
    return _req("GET", path, tok)


def api_post(path, tok, body):
    return _req("POST", path, tok, body)


def docker(*args, check=False):
    r = subprocess.run(["docker", *args], capture_output=True, text=True)
    if check and r.returncode != 0:
        raise AssertionError(f"docker {' '.join(args)} failed: {r.stderr}")
    return r


def docker_state(name):
    r = docker("inspect", "--format", "{{.State.Status}}", name)
    return r.stdout.strip() if r.returncode == 0 else None


def wait_for(fn, what, timeout=60):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = fn()
        if last:
            return last
        time.sleep(1)
    raise AssertionError(f"timed out waiting for {what} (last={last!r})")


def cleanup():
    for n in (CT, EV_CT, f"{STACK}_web", f"{STACK}_cache"):
        docker("rm", "-f", n)
    docker("network", "rm", f"{STACK}_default")
    try:
        tok = token()
        for j in api_get("/api/replication/jobs", tok) or []:
            if j.get("source_vm_id") == REP_VM:
                _req("DELETE", f"/api/replication/jobs/{j['id']}", tok)
        _req("DELETE", f"/api/backup-jobs/{BK_JOB}", tok)
        for b in api_get(f"/api/backups?target_id={REP_VM}", tok) or []:
            _req("PUT", f"/api/backups/{b['id']}", tok, {"protected": False})
            _req("DELETE", f"/api/backups/{b['id']}", tok)
        _req("DELETE", f"/api/vms/{BK_VM}", tok)
        # Linked clones first: their base can't be deleted while they exist.
        for v in api_get("/api/vms", tok) or []:
            if v.get("name") == REP_CLONE:
                _req("DELETE", f"/api/vms/{v['id']}", tok)
        _req("DELETE", f"/api/vms/{REP_VM}", tok)
        _req("DELETE", f"/api/cni/networks/{SDN_NET}", tok)
        for ch in api_get("/api/notifications/channels", tok) or []:
            if ch.get("name") == NOTIFY:
                _req("DELETE", f"/api/notifications/channels/{ch['id']}", tok)
        for r in api_get("/api/alerts/rules", tok) or []:
            if r.get("name") == ALERT_RULE:
                _req("DELETE", f"/api/alerts/rules/{r['id']}", tok)
        for v in api_get("/api/vms", tok) or []:
            if v.get("name") == DISP_VM:
                try:
                    _req("POST", f"/api/vms/{v['id']}/stop", tok, {})
                except AssertionError:
                    pass
                time.sleep(1)
                _req("DELETE", f"/api/vms/{v['id']}", tok)
        _req("DELETE", f"/api/users/{TFA_USER}", tok)
        _req("DELETE", "/api/groups/e2e-ui-group", tok)
        _req("DELETE", "/api/access/realms/e2e-ui-ldap", tok)
    except Exception as e:  # server may not be up yet on the first call
        print(f"cleanup: {e}")


def main():
    cleanup()
    docker("pull", "-q", "busybox:latest", check=True)

    # favicon is a real image, not a 404
    with urlopen(f"{BASE}/favicon.ico") as r:
        assert r.status == 200 and r.headers.get_content_type() != "text/html", r.headers
    with urlopen(f"{BASE}/favicon.png") as r:
        assert r.headers.get_content_type() == "image/png", r.headers

    # SPA deep links are served with 200 (they used to be 404 + index.html),
    # while a missing asset is still a real 404 and not the HTML shell.
    for path in ("/login", "/vms", "/backup/jobs", "/monitoring/alerts"):
        with urlopen(f"{BASE}{path}") as r:
            assert r.status == 200 and r.headers.get_content_type() == "text/html", (path, r.status)
    try:
        urlopen(f"{BASE}/pkg/horcrux-ui.css")
        raise AssertionError("/pkg/horcrux-ui.css: expected 404")
    except urllib.error.HTTPError as e:
        assert e.code == 404, e.code
    # Every page load must fetch only resources that exist (the UI used to
    # request /pkg/horcrux-ui.css, a cargo-leptos path trunk never builds).
    missing_resources = []

    console_errors = []
    with sync_playwright() as p:
        browser = p.chromium.launch()
        context = browser.new_context()
        page = context.new_page()
        page.on("console", lambda m: m.type == "error" and console_errors.append(m.text))
        page.on(
            "response",
            lambda r: r.status == 404
            and r.request.resource_type in ("document", "stylesheet", "script", "image", "font")
            and r.url.startswith(BASE)
            and missing_resources.append(r.url),
        )
        page.on("dialog", lambda d: d.accept(d.default_value) if d.type == "prompt" else d.accept())

        # ---- login ---------------------------------------------------------
        page.goto(f"{BASE}/login")
        page.locator("input[type=text], input:not([type])").first.fill("admin")
        page.locator("input[type=password]").fill("admin")
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/")

        # ---- dashboard reads node metrics (was: missing field cpu_usage) ---
        page.goto(f"{BASE}/")
        cpu = page.locator(".stat-card", has_text="CPU Usage").locator(".stat-value")
        expect(cpu).to_have_text(re.compile(r"^\d+(\.\d)?%$"), timeout=45000)
        expect(page.get_by_text("Could not reach the backend")).to_have_count(0)

        # ---- Monitoring > Notifications: create a webhook channel, Test it --
        # A local receiver stands in for the webhook target; the test message
        # must really arrive and the card's counter must go up.
        received = []

        class Hook(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                received.append(json.loads(self.rfile.read(n) or b"null"))
                self.send_response(204)
                self.end_headers()

            def log_message(self, *a):
                pass

        hook = http.server.HTTPServer(("127.0.0.1", 0), Hook)
        threading.Thread(target=hook.serve_forever, daemon=True).start()
        page.goto(f"{BASE}/monitoring/notifications")
        page.get_by_role("button", name="Create Channel").first.click()
        page.get_by_placeholder("Enter channel name").fill(NOTIFY)
        page.locator(".modal-content select").first.select_option("webhook")
        page.get_by_placeholder("https://api.example.com/webhooks/alerts").fill(
            f"http://127.0.0.1:{hook.server_port}/e2e")
        page.locator(".modal-content .modal-footer").get_by_role("button", name="Create Channel").click()
        card = page.locator(f".channel-card[data-channel-name='{NOTIFY}']")
        expect(card).to_be_visible(timeout=30000)
        card.get_by_role("button", name="Test").click()
        page.get_by_role("button", name="Send Test Notification").click()
        expect(page.locator("#notifications-notice")).to_contain_text(NOTIFY, timeout=30000)
        assert wait_for(lambda: received, "webhook test delivery"), received
        assert received[0]["event"] == "test", received
        expect(card.locator(".stat-value.success")).to_have_text("1", timeout=30000)

        # ---- Monitoring > Alert Center: a rule made in the form really fires
        # (the page used to fail with "invalid type: map, expected a string"
        # for any rule, and rules were never evaluated at all).
        def events(kind):
            return [m for m in received if isinstance(m, dict) and m.get("event") == kind]

        page.goto(f"{BASE}/monitoring/alerts")
        page.get_by_role("button", name="Create Alert Rule").first.click()
        page.fill("#rule-name", ALERT_RULE)
        page.locator("#rule-metric").select_option("node_load")
        page.locator("#rule-operator").select_option("greaterthan")
        page.fill("#rule-threshold", "-1")
        page.fill("#rule-duration", "0")
        page.locator("#rule-severity").select_option("critical")
        page.locator("#rule-submit").click()
        rcard = page.locator(f".alert-rule-card[data-rule-name='{ALERT_RULE}']")
        expect(rcard).to_be_visible(timeout=30000)
        expect(rcard).to_contain_text("Node load average (1 min) > -1")
        expect(page.locator("#alert-center-error")).to_have_count(0)
        rule = next(r for r in api_get("/api/alerts/rules", token()) if r["name"] == ALERT_RULE)
        assert rule["severity"] == "critical" and rule["enabled"], rule
        assert rule["condition"] == {"metric_type": "node_load", "operator": "greaterthan",
                                     "threshold": -1.0, "target_pattern": "*",
                                     "duration_seconds": 0}, rule

        rcard.get_by_role("button", name="Test").click()
        page.get_by_role("button", name="Send Test Alert").click()
        expect(page.locator("#alert-center-notice")).to_contain_text(
            re.compile(r"[1-9]\d* delivered"), timeout=30000)
        assert wait_for(lambda: events("alert_test"), "test alert delivery"), received

        # The server's evaluator (every 15 s) fires it from the node's load.
        page.get_by_role("button", name=re.compile(r"^Active Alerts")).click()
        active = page.locator(f".active-alert-card[data-rule-name='{ALERT_RULE}']")

        def fired():
            page.get_by_role("button", name="Refresh").click()
            page.wait_for_timeout(500)
            return active.count() > 0

        wait_for(fired, "rule to fire", timeout=90)
        expect(active).to_contain_text("firing")
        assert wait_for(lambda: events("alert"), "fired alert delivery"), received
        active.get_by_role("button", name="Acknowledge").click()
        expect(active).to_contain_text("acknowledged", timeout=30000)
        expect(active).to_contain_text("admin")

        # Edit: an unreachable threshold resolves the alert into the history.
        page.get_by_role("button", name=re.compile(r"^Alert Rules")).click()
        rcard.get_by_role("button", name="Edit").click()
        expect(page.locator("#rule-name")).to_have_value(ALERT_RULE)
        page.fill("#rule-threshold", "1000000")
        page.locator("#rule-submit").click()
        expect(page.locator("#alert-center-notice")).to_contain_text("updated", timeout=30000)
        expect(rcard).to_contain_text("> 1000000")
        rule = next(r for r in api_get("/api/alerts/rules", token()) if r["name"] == ALERT_RULE)
        assert rule["condition"]["threshold"] == 1000000, rule
        assert not [a for a in api_get("/api/alerts/active", token()) if a["rule_id"] == rule["id"]]
        page.get_by_role("button", name="Alert History").click()
        hist = page.locator(".alert-history-table tbody tr", has_text=ALERT_RULE)
        expect(hist.filter(has_text="resolved").first).to_be_visible(timeout=30000)

        # Disable, then Delete.
        page.get_by_role("button", name=re.compile(r"^Alert Rules")).click()
        rcard.get_by_role("button", name="Disable").click()
        expect(rcard.locator(".status-disabled")).to_be_visible(timeout=30000)
        assert not next(r for r in api_get("/api/alerts/rules", token())
                        if r["name"] == ALERT_RULE)["enabled"]
        rcard.get_by_role("button", name="Delete").click()
        expect(rcard).to_have_count(0, timeout=30000)
        assert not [r for r in api_get("/api/alerts/rules", token()) if r["name"] == ALERT_RULE]
        hook.shutdown()

        # ---- Monitoring > Metrics Explorer: real samples, real chart --------
        page.goto(f"{BASE}/monitoring/metrics")
        metric = page.locator(".metric-selector select")
        expect(metric.locator("option[value='horcrux_node_load1']")).to_have_count(1, timeout=30000)
        metric.select_option("horcrux_node_load1")
        page.get_by_role("button", name="Execute Query").click()
        # avg(...) of the node series = one plotted series with real points
        expect(page.locator("#metrics-chart svg .series")).to_have_count(1, timeout=30000)
        assert int(page.locator("#metrics-chart svg .series").get_attribute("data-points")) >= 1
        expect(page.get_by_text(re.compile(r"Data Points: [1-9]"))).to_be_visible()
        expect(page.get_by_text("Query failed")).to_have_count(0)

        # ---- Create Container button -> form -> real container -------------
        page.goto(f"{BASE}/containers")
        page.locator("#create-container-btn").click()
        page.wait_for_url(f"{BASE}/containers/create")
        page.fill("#ct-name", CT)
        page.fill("#ct-image", "busybox:latest")
        page.fill("#ct-memory", "64")
        page.fill("#ct-env", "E2E=1")
        page.fill("#ct-ports", "8080")
        page.fill("#ct-command", "httpd -f -p 8080")
        page.locator("#ct-submit").click()
        page.wait_for_url(f"{BASE}/containers", timeout=120000)

        row = page.locator(f"tr[data-container-id='{CT}']")
        expect(row).to_be_visible(timeout=30000)
        expect(row).to_contain_text("busybox:latest")
        expect(row).to_contain_text("running", timeout=30000)
        created_cell = row.locator("td").nth(5)
        expect(created_cell).not_to_have_text("N/A")

        assert wait_for(lambda: docker_state(CT) == "running", "container running"), "not running"
        info = json.loads(docker("inspect", CT, check=True).stdout)[0]
        assert "E2E=1" in info["Config"]["Env"], info["Config"]["Env"]
        assert info["Config"]["Cmd"] == ["httpd", "-f", "-p", "8080"], info["Config"]["Cmd"]
        assert info["Config"]["Labels"].get("horcrux.id") == CT, info["Config"]["Labels"]
        bindings = info["HostConfig"]["PortBindings"]
        assert "8080/tcp" in bindings, bindings

        # form validation error is shown, not swallowed
        page.goto(f"{BASE}/containers/create")
        page.fill("#ct-name", "bad name")
        page.fill("#ct-image", "busybox:latest")
        page.locator("#ct-submit").click()
        expect(page.locator("#create-container-error")).to_contain_text("name", timeout=30000)
        assert page.url == f"{BASE}/containers/create"

        # ---- Stop / Delete from the list reflect real state ----------------
        page.goto(f"{BASE}/containers")
        row = page.locator(f"tr[data-container-id='{CT}']")
        row.get_by_role("button", name="Stop").click()
        expect(row).to_contain_text("stopped", timeout=60000)
        assert wait_for(lambda: docker_state(CT) == "exited", "container exited")
        row.get_by_role("button", name="Delete").click()
        expect(page.locator(f"tr[data-container-id='{CT}']")).to_have_count(0, timeout=60000)
        assert wait_for(lambda: docker_state(CT) is None, "container removed")

        # ---- container created outside Horcrux shows up --------------------
        docker("run", "-d", "--name", EV_CT, "busybox:latest", "sleep", "600", check=True)
        page.goto(f"{BASE}/containers")
        expect(page.locator(f"tr[data-container-id='{EV_CT}']")).to_contain_text("running", timeout=30000)

        # ---- events feed is a readable table -------------------------------
        page.goto(f"{BASE}/containers/events")
        page.get_by_role("button", name="Connect").click()
        expect(page.get_by_text("● live")).to_be_visible(timeout=30000)
        docker("stop", "-t", "1", EV_CT, check=True)
        ev = page.locator("#events-table tbody tr", has_text=EV_CT).filter(has_text="die")
        expect(ev.first).to_be_visible(timeout=30000)
        expect(ev.first.locator("td").first).to_have_text(re.compile(r"^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d UTC$"))
        docker("rm", "-f", EV_CT)

        # ---- stacks: shared network + DNS + ports; remove updates list -----
        page.goto(f"{BASE}/containers/stacks")
        page.get_by_placeholder("stack name").fill(STACK)
        page.locator("textarea").fill(
            "services:\n"
            "  cache:\n"
            "    image: busybox:latest\n"
            "    command: [\"sleep\", \"600\"]\n"
            "  web:\n"
            "    image: busybox:latest\n"
            "    command: [\"httpd\", \"-f\", \"-p\", \"80\"]\n"
            "    ports:\n"
            "      - \"80\"\n"
            "    environment:\n"
            "      ROLE: web\n"
            "    depends_on: [cache]\n"
        )
        page.get_by_role("button", name="Deploy Stack").click()
        stack_row = page.locator("tr", has_text=STACK)
        expect(stack_row).to_be_visible(timeout=120000)
        web, cache = f"{STACK}_web", f"{STACK}_cache"
        assert wait_for(lambda: docker_state(web) == "running", "stack web running")
        nets = json.loads(docker("inspect", web, check=True).stdout)[0]["NetworkSettings"]["Networks"]
        assert f"{STACK}_default" in nets, nets
        winfo = json.loads(docker("inspect", web, check=True).stdout)[0]
        assert "ROLE=web" in winfo["Config"]["Env"], winfo["Config"]["Env"]
        assert "80/tcp" in winfo["HostConfig"]["PortBindings"], winfo["HostConfig"]["PortBindings"]
        r = docker("exec", web, "ping", "-c", "1", "-W", "3", "cache")
        assert r.returncode == 0, f"no DNS for service 'cache': {r.stdout} {r.stderr}"
        stack_row.get_by_role("button", name="Remove").click()
        expect(page.locator("tr", has_text=STACK)).to_have_count(0, timeout=60000)
        assert wait_for(lambda: docker_state(web) is None and docker_state(cache) is None, "stack removed")
        assert docker("network", "inspect", f"{STACK}_default").returncode != 0, "stack network left behind"

        # ---- SDN: default Networking tab creates a real CNI network --------
        page.goto(f"{BASE}/network")
        page.get_by_role("button", name="+ Create CNI Network").click()
        page.get_by_placeholder("cni0").fill(SDN_NET)
        page.get_by_placeholder("10.244.0.0/24").fill("10.251.0.0/24")
        page.get_by_placeholder("10.244.0.1").fill("10.251.0.1")
        page.locator(".modal-footer").get_by_role("button", name="Create").click()
        expect(page.get_by_text(SDN_NET).first).to_be_visible(timeout=30000)
        assert any(n["name"] == SDN_NET for n in api_get("/api/cni/networks", token())), "CNI network not in API"

        # ---- firewall Apply really reaches the API -------------------------
        page.get_by_role("button", name="Firewall").click()
        page.get_by_role("button", name="Apply Rules").click()
        # CI's job container has no CAP_NET_ADMIN: the page must then show the
        # real nftables error (the real apply is the "Firewall apply (real
        # nftables)" CI step); with privileges it shows the success notice.
        outcome = page.locator("#network-notice, .alert-error")
        expect(outcome.first).to_be_visible(timeout=30000)
        text = outcome.first.inner_text()
        assert "applied" in text or "nftables" in text, f"firewall apply outcome: {text!r}"

        # ---- replication: Create button opens a working form ---------------
        api_post("/api/vms", token(), {"id": REP_VM, "name": REP_VM, "hypervisor": "qemu",
                                       "architecture": "x86_64", "cpus": 1, "memory": 128,
                                       "disk_size": 1, "status": "stopped"})
        # ---- VM snapshot: the Create Snapshot dialog really creates one ----
        page.goto(f"{BASE}/snapshots")
        page.locator(".vm-selector select").select_option(REP_VM)
        page.get_by_role("button", name="Create Snapshot").click()
        page.fill("#snap-name", "e2e-snap")
        page.locator("#snap-submit").click()
        expect(page.locator("#snapshot-notice")).to_contain_text("e2e-snap", timeout=60000)
        expect(page.get_by_text("e2e-snap").first).to_be_visible(timeout=30000)
        snaps = api_get(f"/api/vms/{REP_VM}/snapshots", token())
        assert any(s["name"] == "e2e-snap" for s in snaps), snaps

        # ---- VM clone from the VM list: dialog, linked mode, base protected
        page.goto(f"{BASE}/vms")
        page.locator(f"tr[data-vm-id='{REP_VM}']").get_by_role("button", name="Clone").click()
        page.fill("#clone-name", REP_CLONE)
        page.select_option("#clone-mode", "linked")
        expect(page.locator("#clone-linked-help")).to_be_visible()
        page.locator("#clone-submit").click()
        expect(page.locator("#vm-notice")).to_contain_text(REP_CLONE, timeout=120000)
        cvm = next(v for v in api_get("/api/vms", token()) if v["name"] == REP_CLONE)
        cid = cvm["id"]
        assert re.fullmatch(r"vm-\d+", cid), f"clone id not in Create VM's scheme: {cid}"
        expect(page.locator(f"tr[data-vm-id='{cid}']")).to_be_visible(timeout=30000)
        info = json.loads(subprocess.run(
            ["qemu-img", "info", "--output=json", "-U", cvm["disks"][0]["path"]],
            capture_output=True, text=True, check=True).stdout)
        base = info.get("full-backing-filename") or info.get("backing-filename")
        rep_disk = api_get(f"/api/vms/{REP_VM}", token())["disks"][0]["path"]
        assert base and os.path.realpath(base) == os.path.realpath(rep_disk), info
        # ... it's a clone job on the Clones page ...
        page.goto(f"{BASE}/clones")
        expect(page.locator(f"tr.clone-job-row[data-target-name='{REP_CLONE}']")).to_contain_text(
            "completed", timeout=30000)
        # ... and Delete on its base is refused with the reason shown.
        page.goto(f"{BASE}/vms")
        page.locator(f"tr[data-vm-id='{REP_VM}']").get_by_role("button", name="Delete").click()
        expect(page.locator("#vm-notice")).to_contain_text("linked clone", timeout=30000)
        expect(page.locator("#vm-notice")).to_contain_text(cid)
        assert api_get(f"/api/vms/{REP_VM}", token())["id"] == REP_VM
        assert os.path.isfile(rep_disk), "base disk deleted under a linked clone"

        page.goto(f"{BASE}/replication")
        page.locator("#create-replication-btn").click()
        page.locator("#rep-vm").select_option(REP_VM)
        page.fill("#rep-node", "node-b")
        page.fill("#rep-pool", "tank")
        page.locator("#rep-submit").click()
        expect(page.locator("tr", has_text=REP_VM).first).to_be_visible(timeout=30000)
        assert any(j["source_vm_id"] == REP_VM for j in api_get("/api/replication/jobs", token()))

        # ---- backups: back up from /backup, verify, restore to a new VM --
        page.goto(f"{BASE}/backup")
        expect(page.locator(f"#backup-target option[value='{REP_VM}']")).to_have_count(1, timeout=30000)
        page.locator("#backup-target").select_option(REP_VM)
        page.locator("#backup-now-btn").click()
        expect(page.locator("#backup-notice")).to_contain_text("written", timeout=180000)
        row = page.locator(f"tr.backup-row[data-target-id='{REP_VM}']").first
        expect(row).to_be_visible(timeout=30000)
        bid = row.get_attribute("data-backup-id")
        archive = api_get(f"/api/backups/{bid}", token())["path"]
        assert os.path.getsize(archive) > 0, archive
        page.locator(f"tr.backup-row[data-backup-id='{bid}'] .verify-btn").click()
        expect(page.locator(f"tr.backup-row[data-backup-id='{bid}'] .verify-state")).to_contain_text("OK", timeout=120000)
        assert api_get(f"/api/backups/{bid}", token())["verify_state"] == "ok"
        page.locator(f"tr.backup-row[data-backup-id='{bid}'] .restore-btn").click()
        page.fill("#restore-target", BK_VM)
        page.locator("#restore-submit").click()
        expect(page.locator("#backup-notice")).to_contain_text(BK_VM, timeout=180000)
        restored = api_get(f"/api/vms/{BK_VM}", token())
        assert restored["id"] == BK_VM and os.path.isfile(restored["disks"][0]["path"]), restored

        # ---- backup jobs: create from the form, Run now, result shown -----
        page.goto(f"{BASE}/backup/jobs")
        page.locator("#new-job-btn").click()
        page.fill("#job-id", BK_JOB)
        page.locator(f"#job-targets input[value='{REP_VM}']").check()
        page.locator("#job-submit").click()
        job_row = page.locator(f"tr.job-row[data-job-id='{BK_JOB}']")
        expect(job_row).to_be_visible(timeout=30000)
        job_row.locator(".run-btn").click()
        expect(page.locator(f"tr.job-row[data-job-id='{BK_JOB}'] .job-last-run")).to_contain_text("OK", timeout=180000)
        assert any(b.get("job_id") == BK_JOB for b in api_get(f"/api/backups?target_id={REP_VM}", token()))

        # ---- retention preview + verify-all pages render real data --------
        page.goto(f"{BASE}/backup/retention")
        page.locator("#prune-target").select_option(REP_VM)
        page.fill("#prune-keep-0", "1")
        page.locator("#prune-preview").click()
        expect(page.locator("#prune-notice")).to_contain_text("would be removed", timeout=30000)
        page.goto(f"{BASE}/backup/verify")
        page.locator("#verify-all-btn").click()
        expect(page.locator("#verify-summary")).to_contain_text("0 failed", timeout=180000)
        access_control_checks(context, page)

        # ---- VM display: Create VM form -> console page -> live consoles ----
        page.goto(f"{BASE}/vms/create")
        page.locator(".form-group", has_text="VM Name").locator("input").fill(DISP_VM)
        page.locator(".form-group", has_text="Memory (MB)").locator("input").fill("512")
        page.locator(".form-group", has_text="Disk Size (GB)").locator("input").fill("10")
        page.select_option("#disp-protocol", "spice")
        page.select_option("#disp-video", "qxl")
        page.select_option("#disp-sound", "ich9-intel-hda")
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/vms", timeout=120000)
        dvm = next(v for v in api_get("/api/vms", token()) if v["name"] == DISP_VM)
        vid = dvm["id"]
        assert dvm["display"] == {"protocol": "spice", "video": "qxl", "sound": "ich9-intel-hda"}, dvm

        # stopped VM: the console page's Display settings switch it to VNC
        page.goto(f"{BASE}/vms/{vid}/console")
        page.select_option("#disp-protocol", "vnc")
        page.locator("#save-display").click()
        expect(page.locator("#display-notice")).to_contain_text("Saved", timeout=30000)
        d = api_get(f"/api/vms/{vid}", token())["display"]
        assert d["protocol"] == "vnc" and d["sound"] == "none", d

        # Start from the VM list; noVNC in the console page really connects
        # to QEMU's VNC server (password-protected, through the ticketed
        # WebSocket proxy).
        page.goto(f"{BASE}/vms")
        page.locator(f"tr[data-vm-id='{vid}']").get_by_role("button", name="Start").click()
        expect(page.locator(f"tr[data-vm-id='{vid}']")).to_contain_text(
            re.compile("running", re.IGNORECASE), timeout=60000)  # list shows "Running"
        page.goto(f"{BASE}/vms/{vid}/console")
        expect(page.frame_locator("iframe.console-iframe").locator("#status")).to_have_text(
            "Connected", timeout=90000)

        # Back to SPICE: stop, switch in the console page, start; the SPICE
        # panel shows the endpoint and the .vv download works with the
        # browser's own session.
        page.goto(f"{BASE}/vms")
        page.locator(f"tr[data-vm-id='{vid}']").get_by_role("button", name="Stop").click()
        expect(page.locator(f"tr[data-vm-id='{vid}']")).to_contain_text("stopped", timeout=60000)
        page.goto(f"{BASE}/vms/{vid}/console")
        page.select_option("#disp-protocol", "spice")
        page.select_option("#disp-sound", "intel-hda")
        page.locator("#save-display").click()
        expect(page.locator("#display-notice")).to_contain_text("Saved", timeout=30000)
        page.goto(f"{BASE}/vms")
        page.locator(f"tr[data-vm-id='{vid}']").get_by_role("button", name="Start").click()
        expect(page.locator(f"tr[data-vm-id='{vid}']")).to_contain_text(
            re.compile("running", re.IGNORECASE), timeout=60000)  # list shows "Running"
        page.goto(f"{BASE}/vms/{vid}/console")
        expect(page.locator("#spice-endpoint")).to_contain_text("127.0.0.1:", timeout=30000)
        href = page.locator("#spice-vv").get_attribute("href")
        # urllib with the node CA (Playwright's request context is Node's TLS
        # stack, which doesn't trust the CA the browser was given).
        vv_req = urllib.request.Request(href if href.startswith("http") else f"{BASE}{href}",
                                        headers={"Authorization": f"Bearer {token()}"})
        with urlopen(vv_req) as vv:
            assert vv.status == 200, vv.status
            body = vv.read().decode()
        assert "type=spice" in body and "enable-usbredir=1" in body, body
        page.goto(f"{BASE}/vms")
        page.locator(f"tr[data-vm-id='{vid}']").get_by_role("button", name="Stop").click()
        expect(page.locator(f"tr[data-vm-id='{vid}']")).to_contain_text("stopped", timeout=60000)

        s3_gateway_checks(page)
        monitoring_dashboard_checks(page)
        users_edit_checks(page)
        vm_hardware_checks(page, vid)

        browser.close()

    bad = [e for e in console_errors if "favicon" in e or "missing field" in e]
    assert not bad, f"console errors: {bad}"
    assert not missing_resources, f"page loads fetched missing resources (404): {sorted(set(missing_resources))}"
    print("UI E2E: all checks passed")


def vm_hardware_checks(page, vid):
    """ISO library download, VM Hardware page (CD-ROM, boot order, CPUs),
    Host Bridges page, and the Migration Center's resource cards (they were
    rendered once, empty, before the VM list loaded - no Migrate button)."""
    # ---- Infrastructure > ISO Images: download from a URL ---------------
    iso_dir = "/tmp/e2e-isosrc"
    os.makedirs(iso_dir, exist_ok=True)
    with open(f"{iso_dir}/e2e-ui.iso", "wb") as f:
        f.write(b"\0" * 65536)
    handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=iso_dir, **k)
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    tok = token()
    _req("DELETE", "/api/storage/isos/e2e-ui.iso", tok)
    page.goto(f"{BASE}/storage/isos")
    page.fill("#iso-url", f"http://127.0.0.1:{srv.server_address[1]}/e2e-ui.iso")
    page.locator("#iso-download").click()
    expect(page.locator("tr.iso-row[data-iso='e2e-ui.iso'] .iso-status")).to_have_text("ready", timeout=60000)
    iso = next(i for i in api_get("/api/storage/isos", tok) if i["name"] == "e2e-ui.iso")

    # ---- VM > Hardware: CD-ROM + boot order + CPUs, saved ----------------
    page.goto(f"{BASE}/vms")
    page.locator(f"tr[data-vm-id='{vid}']").get_by_role("link", name="Hardware").click()
    page.wait_for_url(f"{BASE}/vms/{vid}/hardware")
    expect(page.locator("#vm-cdrom option", has_text="e2e-ui.iso")).to_have_count(1, timeout=30000)
    page.select_option("#vm-cdrom", iso["path"])
    page.select_option("#vm-boot", "cdrom")
    page.fill("#vm-hw-cpus", "2")
    page.locator("#vm-hw-save").click()
    expect(page.locator("#vm-hw-notice")).to_contain_text("Saved", timeout=30000)
    v = api_get(f"/api/vms/{vid}", tok)
    assert v["cpus"] == 2 and v["hardware"]["cdrom"] == iso["path"], v
    assert v["hardware"]["boot"] == ["cdrom", "disk"], v["hardware"]
    # "Add network card" adds a row (or explains there is no bridge yet,
    # with a link to Host Bridges - never a dead end).
    page.locator("#vm-nic-add").click()
    expect(page.locator(".vm-nic-row")).to_have_count(1)
    if page.locator("#vm-no-bridges").count():
        page.locator("#vm-no-bridges a").click()
        page.wait_for_url(f"{BASE}/network/bridges")
    # eject so the ISO can be deleted
    _req("PUT", f"/api/vms/{vid}/config", tok, {"hardware": {"cdrom": None}})
    _req("DELETE", "/api/storage/isos/e2e-ui.iso", tok)

    # ---- VM > Hardware: System (CPU type, chipset) + Disks -----------------
    # (the NIC step above may have followed the link to Host Bridges)
    page.goto(f"{BASE}/vms/{vid}/hardware")
    expect(page.locator("#vm-system")).to_be_visible(timeout=30000)
    page.select_option("#vm-cpu-type", "qemu64")
    page.select_option("#vm-machine", "q35")
    page.locator("#vm-hw-save").click()
    expect(page.locator("#vm-hw-notice")).to_contain_text("Saved", timeout=30000)
    v = api_get(f"/api/vms/{vid}", tok)
    assert v["hardware"].get("cpu_type") == "qemu64" and v["hardware"].get("machine") == "q35", v["hardware"]
    n_disks = len(v["disks"])
    page.fill("#vm-disk-add-size", "1")
    page.select_option("#vm-disk-add-bus", "scsi")
    page.locator("#vm-disk-add").click()
    expect(page.locator("#vm-disk-notice")).to_contain_text("Added", timeout=30000)
    expect(page.locator(".vm-disk-row")).to_have_count(n_disks + 1)
    v = api_get(f"/api/vms/{vid}", tok)
    assert len(v["disks"]) == n_disks + 1 and v["disks"][-1]["disk_type"] == "scsi", v["disks"]
    last = page.locator(f".vm-disk-row[data-disk='{n_disks}']")
    last.locator(".vm-disk-delete-file").check()
    last.locator(".vm-disk-detach").click()
    expect(page.locator("#vm-disk-notice")).to_contain_text("detached", timeout=30000)
    assert len(api_get(f"/api/vms/{vid}", tok)["disks"]) == n_disks
    _req("PUT", f"/api/vms/{vid}/config", tok, {"hardware": {"machine": "i440fx"}})

    # ---- Infrastructure > Host Bridges renders -----------------------------
    page.goto(f"{BASE}/network/bridges")
    expect(page.locator("#bridge-table")).to_be_visible(timeout=30000)
    expect(page.locator("#bridge-create-form")).to_be_visible()

    # ---- HA > Migration Center lists the VM with a Migrate button -----------
    page.goto(f"{BASE}/ha/migration")
    card = page.locator("#migration-vm-grid .resource-card", has_text=v["name"])
    expect(card).to_have_count(1, timeout=30000)
    migrate = card.get_by_role("button", name="Migrate")
    expect(migrate).to_be_enabled()
    migrate.click()
    expect(page.locator(".modal-content", has_text="Migrate Resource")).to_be_visible()
    # a stopped VM is offered an offline migration
    expect(page.locator(".modal-content select").nth(1)).to_have_value("offline")
    page.locator(".modal-content .modal-close").click()
    srv.shutdown()

    # ---- Monitoring overview buttons do something (they had no handler) --
    page.goto(f"{BASE}/monitoring")
    with page.expect_download(timeout=30000) as dl:
        page.locator("#monitoring-export").click()
    data = json.loads(open(dl.value.path()).read())
    assert "exported_at" in data and "node" in data and "vms" in data, data
    page.get_by_role("link", name="Configure Alerts").click()
    page.wait_for_url(f"{BASE}/monitoring/alerts")


def _free_port():
    import socket
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def _s3_get(port, bucket, key, ak, sk, region="us-east-1"):
    """GET an object with an access key, signed by curl (SigV4)."""
    r = subprocess.run(["curl", "-s", "-o", "/dev/null", "-w", "%{http_code}",
                        "--aws-sigv4", f"aws:amz:{region}:s3", "--user", f"{ak}:{sk}",
                        f"http://127.0.0.1:{port}/{bucket}/{key}"], capture_output=True, text=True)
    return int(r.stdout.strip() or 0)


def s3_gateway_checks(page):
    """NAS > S3 Gateway page end to end against the real server (silo + mcli
    installed by ci/install-s3-gateway.sh): start, bucket, upload, download,
    S3 user, access key used by a real S3 client, Rotate, Disable, Delete,
    bucket delete, stop."""
    tok = token()
    _req("POST", "/api/nas/s3/stop", tok, {})
    port, cport = _free_port(), _free_port()
    _req("PUT", "/api/nas/s3/config", tok, {"address": "127.0.0.1", "port": port, "console_port": cport})
    page.goto(f"{BASE}/nas/s3")
    expect(page.locator("#s3-state")).to_have_text("Stopped", timeout=30000)
    expect(page.locator("#s3-missing")).to_have_count(0)
    page.locator("#s3-start").click()
    expect(page.locator("#s3-state")).to_have_text("Running", timeout=90000)
    expect(page.locator("#s3-endpoint")).to_contain_text(f":{port}")

    bucket = "e2e-ui-bucket"
    page.locator("#s3-bucket-name").fill(bucket)
    page.locator("#s3-bucket-create").click()
    row = page.locator(f"tr[data-bucket='{bucket}']")
    expect(row).to_be_visible(timeout=30000)
    row.locator(".s3-browse").click()
    expect(page.locator("#s3-objects")).to_be_visible()
    payload = b"hello from the Horcrux UI\n"
    page.set_input_files("#s3-upload-file", files=[{"name": "hello.txt", "mimeType": "text/plain", "buffer": payload}])
    page.locator("#s3-upload").click()
    expect(page.locator("tr[data-key='hello.txt']")).to_be_visible(timeout=30000)
    with page.expect_download(timeout=30000) as dl:
        page.locator("tr[data-key='hello.txt'] .s3-download").click()
    assert open(dl.value.path(), "rb").read() == payload

    # S3 user (read-only) and an access key for it
    page.locator("#s3-tab-users").click()
    page.locator("#s3-user-name").fill("e2e-ui-user")
    page.select_option("#s3-user-policy", "readonly")
    page.locator("#s3-user-create").click()
    expect(page.locator("tr[data-user='e2e-ui-user']")).to_be_visible(timeout=30000)
    expect(page.locator("#s3-secret")).to_be_visible()
    page.locator("#s3-secret button").click()

    page.locator("#s3-tab-keys").click()
    page.select_option("#s3-key-user", "e2e-ui-user")
    page.locator("#s3-key-desc").fill("e2e ui key")
    page.locator("#s3-key-create").click()
    expect(page.locator("#s3-secret")).to_be_visible(timeout=30000)
    ak = page.locator("#s3-secret-access").inner_text().strip()
    sk = page.locator("#s3-secret-secret").inner_text().strip()
    assert len(sk) >= 20, sk
    assert _s3_get(port, bucket, "hello.txt", ak, sk) == 200, "new key reads the object"
    page.locator("#s3-secret button").click()
    key_row = page.locator(f"tr[data-key='{ak}']")
    expect(key_row).to_contain_text("e2e-ui-user")

    # Rotate: the page shows a new secret; the old one is refused
    key_row.locator(".s3-key-rotate").click()
    expect(page.locator("#s3-secret")).to_contain_text("New secret", timeout=30000)
    sk2 = page.locator("#s3-secret-secret").inner_text().strip()
    assert sk2 and sk2 != sk
    page.locator("#s3-secret button").click()
    assert _s3_get(port, bucket, "hello.txt", ak, sk) == 403, "old secret refused after Rotate"
    assert _s3_get(port, bucket, "hello.txt", ak, sk2) == 200, "rotated secret works"

    # Disable / Enable
    key_row.locator(".s3-key-toggle").click()
    expect(key_row).to_have_attribute("data-enabled", "false", timeout=30000)
    assert _s3_get(port, bucket, "hello.txt", ak, sk2) == 403, "disabled key refused"
    key_row.locator(".s3-key-toggle").click()
    expect(key_row).to_have_attribute("data-enabled", "true", timeout=30000)

    # Delete key (confirm dialog accepted by the page handler)
    key_row.locator(".s3-key-delete").click()
    expect(key_row).to_have_count(0, timeout=30000)
    assert _s3_get(port, bucket, "hello.txt", ak, sk2) == 403, "deleted key refused"

    page.locator("#s3-tab-users").click()
    page.locator("tr[data-user='e2e-ui-user'] .s3-user-delete").click()
    expect(page.locator("tr[data-user='e2e-ui-user']")).to_have_count(0, timeout=30000)

    page.locator("#s3-tab-buckets").click()
    page.locator(f"tr[data-bucket='{bucket}'] .s3-bucket-delete").click()
    expect(page.locator(f"tr[data-bucket='{bucket}']")).to_have_count(0, timeout=30000)

    page.locator("#s3-stop").click()
    expect(page.locator("#s3-state")).to_have_text("Stopped", timeout=60000)
    import socket
    try:
        socket.create_connection(("127.0.0.1", port), timeout=2).close()
        raise AssertionError("S3 port still open after Stop")
    except OSError:
        pass
    print("UI E2E: S3 gateway (start, bucket, upload/download, user, key rotate/disable/delete, stop) passed")


def monitoring_dashboard_checks(page):
    """Monitoring overview draws real trend charts; Monitoring > Dashboards
    creates, saves, reloads, edits and deletes a user-defined dashboard."""
    page.goto(f"{BASE}/monitoring")
    cpu = page.locator("#performance-trends .metric-panel[data-query='horcrux_node_cpu_usage_percent']")
    expect(cpu.locator("polyline.series")).to_have_count(1, timeout=60000)
    expect(page.locator("#performance-trends")).not_to_contain_text("coming soon")
    page.select_option("#trend-range", "900")
    expect(cpu.locator("polyline.series")).to_have_count(1, timeout=30000)

    for d in api_get("/api/monitoring/dashboards", token()):
        if d["name"].startswith("E2E dashboard"):
            _req("DELETE", f"/api/monitoring/dashboards/{d['id']}", token())
    page.goto(f"{BASE}/monitoring/dashboards")
    expect(page.locator("body")).not_to_contain_text("coming soon")
    page.locator("#create-dashboard").click()
    expect(page.locator("#dashboard-editor")).to_be_visible()
    page.locator("#dash-name").fill("E2E dashboard")
    page.select_option("#dash-range", "15m")
    page.locator("#add-panel").click()
    p2 = page.locator(".panel-editor[data-index='1']")
    p2.locator(".panel-title").fill("Load now")
    p2.locator(".panel-title").press("Tab")
    p2.locator(".panel-query").fill("horcrux_node_load1")
    p2.locator(".panel-query").press("Tab")
    p2.locator(".panel-chart").select_option("stat")
    page.locator("#save-dashboard").click()
    expect(page.locator("#dashboard-title")).to_have_text("E2E dashboard", timeout=30000)
    page.wait_for_url(re.compile(r".*/monitoring/dashboards\?id=.+"), timeout=30000)
    expect(page.locator("#dashboard-view .metric-panel")).to_have_count(2)
    expect(page.locator("#dashboard-view .metric-panel").nth(0).locator("polyline.series")).to_have_count(1, timeout=60000)
    expect(page.locator("#dashboard-view .metric-panel").nth(1).locator(".stat-value")).to_have_count(1, timeout=60000)

    # saved: survives a full reload
    url = page.url
    page.goto(url)
    expect(page.locator("#dashboard-title")).to_have_text("E2E dashboard", timeout=30000)
    expect(page.locator("#dashboard-view .metric-panel")).to_have_count(2)
    stored = [d for d in api_get("/api/monitoring/dashboards", token()) if d["name"] == "E2E dashboard"]
    assert len(stored) == 1 and [p["query"] for p in stored[0]["panels"]] == [
        "horcrux_node_cpu_usage_percent", "horcrux_node_load1"], stored

    # edit: rename, drop the first panel
    page.locator("#edit-dashboard").click()
    page.locator("#dash-name").fill("E2E dashboard v2")
    page.locator(".panel-editor[data-index='0'] .panel-remove").click()
    page.locator("#save-dashboard").click()
    expect(page.locator("#dashboard-title")).to_have_text("E2E dashboard v2", timeout=30000)
    expect(page.locator("#dashboard-view .metric-panel")).to_have_count(1)

    page.locator("#delete-dashboard").click()
    expect(page.locator("#dashboard-notice")).to_contain_text("deleted", timeout=30000)
    assert not [d for d in api_get("/api/monitoring/dashboards", token()) if d["name"].startswith("E2E dashboard")]
    print("UI E2E: monitoring trends + user-defined dashboards passed")


def users_edit_checks(page):
    """Access > Users: the row's edit icon opens a working edit form."""
    tok = token()
    _req("DELETE", "/api/users/e2e-edit-user", tok)
    api_post("/api/users", tok, {"username": "e2e-edit-user", "password": "e2e-edit-pass-1"})
    page.goto(f"{BASE}/auth/users")
    row = page.locator("tr", has_text="e2e-edit-user")
    expect(row).to_be_visible(timeout=30000)
    row.locator(".edit-btn").click()
    expect(page.locator("#edit-user-panel")).to_be_visible()
    page.locator("#edit-comment").fill("edited from the UI")
    page.locator("#edit-email").fill("e2e-edit@errant.solutions")
    page.locator("#edit-save").click()
    expect(page.locator("#edit-user-panel")).to_have_count(0, timeout=30000)
    u = next(u for u in api_get("/api/users", tok) if u["username"] == "e2e-edit-user")
    assert u.get("comment") == "edited from the UI" and u.get("email") == "e2e-edit@errant.solutions", u
    expect(row).to_contain_text("edited from the UI", timeout=30000)
    _req("DELETE", "/api/users/e2e-edit-user", tok)
    print("UI E2E: users edit icon passed")


def ui_login(page, user, password):
    page.goto(f"{BASE}/login")
    page.fill("#login-username", user)
    page.fill("#login-password", password)
    page.locator("#login-submit").click()


def ui_logout(page):
    page.locator("#logout-button").click()
    page.wait_for_url(f"{BASE}/login")


def access_control_checks(context, page):
    tok = token()
    # http on the HTTPS port redirects
    plain = BASE.replace("https://", "http://", 1)

    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None

    try:
        urllib.request.build_opener(NoRedirect).open(f"{plain}/vms")
        raise AssertionError("plain HTTP was served")
    except urllib.error.HTTPError as e:
        assert e.code == 308 and e.headers["Location"].startswith("https://"), (e.code, e.headers)

    # ---- admin pages: permissions/groups, realms, certificates ----------
    page.goto(f"{BASE}/auth/permissions")
    page.fill("#group-name", "e2e-ui-group")
    page.locator("#group-submit").click()
    expect(page.locator("#groups-table")).to_contain_text("e2e-ui-group", timeout=30000)
    assert any(g["name"] == "e2e-ui-group" for g in api_get("/api/groups", tok))

    page.goto(f"{BASE}/auth/realms")
    page.fill("#realm-name", "e2e-ui-ldap")
    page.locator("#realm-submit").click()
    expect(page.locator("#realms-table")).to_contain_text("e2e-ui-ldap", timeout=30000)
    realm = next(r for r in api_get("/api/access/realms", tok) if r["name"] == "e2e-ui-ldap")
    assert realm["realm_type"] == "ldap", realm

    page.goto(f"{BASE}/system/certificates")
    expect(page.locator("#cert-fingerprint")).not_to_have_text("", timeout=30000)
    before = page.locator("#cert-fingerprint").inner_text()
    page.fill("#cert-extra-san", "e2e-ui.horcrux.test")
    page.locator("#cert-regenerate").click()
    expect(page.locator("#cert-notice")).to_be_visible(timeout=30000)
    expect(page.locator("#cert-fingerprint")).not_to_have_text(before, timeout=30000)
    expect(page.locator("#cert-san")).to_contain_text("e2e-ui.horcrux.test")

    # ---- a user created by the admin logs in and enrolls TOTP -----------
    _req("DELETE", f"/api/users/{TFA_USER}", tok)
    api_post("/api/users", tok, {"username": TFA_USER, "password": TFA_PASS})
    api_post("/api/acl", tok, {"path": "/vms", "subject_type": "user", "subject": TFA_USER, "role": "VMUser"})
    ui_logout(page)
    ui_login(page, TFA_USER, TFA_PASS)
    page.wait_for_url(f"{BASE}/")
    expect(page.locator("#navbar-user")).to_contain_text(TFA_USER, timeout=30000)
    page.goto(f"{BASE}/auth/mfa")
    page.locator("#mfa-add-totp").click()
    expect(page.locator("#totp-setup svg")).to_be_visible(timeout=30000)
    secret = page.locator("#totp-secret").inner_text().strip()
    page.fill("#totp-code", totp_code(secret))
    page.locator("#totp-confirm").click()
    expect(page.locator("#recovery-codes")).to_be_visible(timeout=30000)
    expect(page.locator("#mfa-methods")).to_contain_text("Authenticator app")

    # ---- security key via Chromium's virtual authenticator --------------
    cdp = context.new_cdp_session(page)
    cdp.send("WebAuthn.enable")
    cdp.send("WebAuthn.addVirtualAuthenticator", {"options": {
        "protocol": "ctap2", "transport": "usb", "hasResidentKey": False,
        "hasUserVerification": True, "isUserVerified": True}})
    page.fill("#mfa-factor-name", "virtual key")
    page.locator("#mfa-add-webauthn").click()
    try:
        expect(page.locator("#mfa-notice")).to_contain_text("Security key added", timeout=30000)
    except AssertionError:
        # Say WHY (the page shows the API/browser error in #mfa-error).
        err = page.locator("#mfa-error")
        detail = err.inner_text() if err.count() else "(no #mfa-error shown)"
        raise AssertionError(f"security key registration failed: {detail}") from None
    expect(page.locator("#mfa-methods")).to_contain_text("virtual key")

    # ---- next login asks for the second factor: code ---------------------
    ui_logout(page)
    ui_login(page, TFA_USER, TFA_PASS)
    expect(page.locator("#tfa-step")).to_be_visible(timeout=30000)
    page.fill("#tfa-code", totp_code(secret, time.time() + 30))
    page.locator("#tfa-submit").click()
    page.wait_for_url(f"{BASE}/", timeout=30000)
    # ---- ... or the security key ------------------------------------------
    ui_logout(page)
    ui_login(page, TFA_USER, TFA_PASS)
    expect(page.locator("#tfa-webauthn")).to_be_visible(timeout=30000)
    page.locator("#tfa-webauthn").click()
    page.wait_for_url(f"{BASE}/", timeout=30000)
    expect(page.locator("#navbar-user")).to_contain_text(TFA_USER)
    # a VMUser can't open user administration data
    page.goto(f"{BASE}/auth/users")
    status = api_status("/api/users", page)
    assert status == [403], status
    ui_logout(page)
    _req("DELETE", f"/api/users/{TFA_USER}", tok)
    # The checks after this one run as the administrator again.
    ui_login(page, "admin", "admin")
    page.wait_for_url(f"{BASE}/", timeout=30000)
    expect(page.locator("#navbar-user")).to_contain_text("admin", timeout=30000)
    print("UI E2E: login, TOTP + security key 2FA, permissions, realms, certificates passed")


def api_status(path, page):
    """Status of GET `path` with the browser page's own stored token."""
    t = page.evaluate("() => localStorage.getItem('horcrux_auth_token')")
    try:
        urlopen(urllib.request.Request(f"{BASE}{path}", headers={"Authorization": f"Bearer {t}"}))
        return [200]
    except urllib.error.HTTPError as e:
        return [e.code]


if __name__ == "__main__":
    try:
        main()
    finally:
        cleanup()
