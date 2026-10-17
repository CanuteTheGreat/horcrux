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
"""
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "http://127.0.0.1:8006")
CT = "e2e-ui-ct"
STACK = "e2e-ui-stack"
EV_CT = "e2e-ui-events"
SDN_NET = "e2esdn0"
REP_VM = "e2e-rep-vm"
_TOKEN = []


def token():
    if not _TOKEN:
        req = urllib.request.Request(f"{BASE}/api/auth/login",
                                     data=json.dumps({"username": "admin", "password": "admin"}).encode(),
                                     headers={"Content-Type": "application/json"})
        _TOKEN.append(json.load(urllib.request.urlopen(req))["ticket"])
    return _TOKEN[0]


def _req(method, path, tok, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(f"{BASE}{path}", data=data, method=method,
                               headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r) as resp:
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
        _req("DELETE", f"/api/vms/{REP_VM}-clone", tok)
        _req("DELETE", f"/api/vms/{REP_VM}", tok)
        _req("DELETE", f"/api/cni/networks/{SDN_NET}", tok)
    except Exception as e:  # server may not be up yet on the first call
        print(f"cleanup: {e}")


def main():
    cleanup()
    docker("pull", "-q", "busybox:latest", check=True)

    # favicon is a real image, not a 404
    with urllib.request.urlopen(f"{BASE}/favicon.ico") as r:
        assert r.status == 200 and r.headers.get_content_type() != "text/html", r.headers
    with urllib.request.urlopen(f"{BASE}/favicon.png") as r:
        assert r.headers.get_content_type() == "image/png", r.headers

    console_errors = []
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.on("console", lambda m: m.type == "error" and console_errors.append(m.text))
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
        expect(page.locator("#network-notice")).to_contain_text("applied", timeout=30000)

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

        # ---- VM clone from the VM list -------------------------------------
        page.goto(f"{BASE}/vms")
        page.locator(f"tr[data-vm-id='{REP_VM}']").get_by_role("button", name="Clone").click()
        expect(page.locator(f"tr[data-vm-id='{REP_VM}-clone']")).to_be_visible(timeout=120000)
        assert api_get(f"/api/vms/{REP_VM}-clone", token())["id"] == f"{REP_VM}-clone"

        page.goto(f"{BASE}/replication")
        page.locator("#create-replication-btn").click()
        page.locator("#rep-vm").select_option(REP_VM)
        page.fill("#rep-node", "node-b")
        page.fill("#rep-pool", "tank")
        page.locator("#rep-submit").click()
        expect(page.locator("tr", has_text=REP_VM).first).to_be_visible(timeout=30000)
        assert any(j["source_vm_id"] == REP_VM for j in api_get("/api/replication/jobs", token()))

        browser.close()

    bad = [e for e in console_errors if "favicon" in e or "missing field" in e]
    assert not bad, f"console errors: {bad}"
    print("UI E2E: all checks passed")


if __name__ == "__main__":
    try:
        main()
    finally:
        cleanup()
