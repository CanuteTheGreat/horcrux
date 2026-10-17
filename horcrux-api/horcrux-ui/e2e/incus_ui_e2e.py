#!/usr/bin/env python3
"""Browser E2E of the LXD/Incus consoles against a real horcrux-api + Incus.

The API and this build's UI run in a privileged Incus sibling sharing this
job's network namespace (horcrux-api/tests/incus_e2e.sh serve). Everything
is done the way a user does it in the browser.

MODE=ct: create an Incus container from the Create Container form, start it,
         open its Console from the list (log in on /dev/console, run a
         command) and its Terminal (exec PTY, run a command).
MODE=vm: (KVM runner) an Incus VM's console page offers Serial + SPICE but
         no VNC; log in on the serial console and run a command; SPICE shows
         the relay endpoint and downloads a working .vv file.
No page may tell the user to use a CLI console.
"""
import json
import os
import re
import ssl
import subprocess
import time
import urllib.error
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://127.0.0.1:8008")
# The API serves HTTPS from the node-local CA: trust that CA when given
# (HORCRUX_CA_FILE), otherwise accept the self-signed certificate.
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL_CTX = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
MODE = os.environ.get("MODE", "ct")
# Runs a command inside the Incus sibling (fixtures only, e.g. passwd -d).
SIBLING_EXEC = os.environ.get("SIBLING_EXEC", "bash horcrux-api/tests/incus_e2e.sh exec")
CT = "e2e-incus-ct"
VM = "e2e-incus-vm"
CLI_HINT = re.compile(r"incus console|lxc console|use the console|--type=vga", re.I)
_TOKEN = []


def log(msg):
    print(f"[incus-ui-e2e:{MODE}] {msg}", flush=True)


def token():
    if not _TOKEN:
        r = urllib.request.Request(
            f"{BASE}/api/auth/login",
            data=json.dumps({"username": "admin", "password": "admin"}).encode(),
            headers={"Content-Type": "application/json"},
        )
        body = json.load(urllib.request.urlopen(r, timeout=30, context=SSL_CTX))
        _TOKEN.append(body.get("ticket") or body.get("token"))
    return _TOKEN[0]


def api(method, path, body=None, timeout=1200):
    r = urllib.request.Request(
        f"{BASE}/api{path}",
        data=None if body is None else json.dumps(body).encode(),
        method=method,
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {token()}"},
    )
    try:
        with urllib.request.urlopen(r, timeout=timeout, context=SSL_CTX) as resp:
            text = resp.read().decode()
            return resp.status, (json.loads(text) if text else None)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def sibling(cmd):
    p = subprocess.run(f"{SIBLING_EXEC} {cmd}", shell=True, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


def wait_for(fn, what, timeout=120):
    end = time.time() + timeout
    while time.time() < end:
        try:
            if fn():
                return True
        except AssertionError:
            pass
        time.sleep(3)
    raise AssertionError(f"timed out: {what}")


def no_cli_hint(page, where):
    text = page.locator("body").inner_text()
    m = CLI_HINT.search(text)
    assert not m, f"{where} tells the user to use a CLI: ...{text[max(0, m.start() - 80):m.end() + 80]}..."


def login(page):
    page.goto(f"{BASE}/login")
    page.locator("input[type=text], input:not([type])").first.fill("admin")
    page.locator("input[type=password]").fill("admin")
    page.locator("button[type=submit]").click()
    page.wait_for_url(f"{BASE}/")


def getty_login(page, inp, out, what):
    """Press Enter until a login prompt (or shell) shows, log in, run a command."""
    def prompt():
        inp.press("Enter")
        time.sleep(2)
        t = out.inner_text()
        return "login:" in t or re.search(r"#\s*$", t)
    wait_for(prompt, f"{what}: login prompt", 240)
    if out.inner_text().rstrip().endswith("login:"):
        inp.fill("root")
        inp.press("Enter")
        expect(out).to_contain_text("#", timeout=60000)
    inp.fill("echo ui-console-$((6*7))")
    inp.press("Enter")
    expect(out).to_contain_text("ui-console-42", timeout=60000)
    log(f"ok: {what}: logged in and ran a command from the browser")


def run_ct(page):
    api("DELETE", f"/containers/{CT}")
    page.goto(f"{BASE}/containers/create")
    page.fill("#ct-name", CT)
    page.locator("#ct-runtime").select_option("incus")
    page.fill("#ct-image", "alpine/3.22")
    page.fill("#ct-memory", "256")
    page.locator("#ct-submit").click()
    page.wait_for_url(f"{BASE}/containers", timeout=600000)
    row = page.locator(f"tr[data-container-id='{CT}']")
    expect(row).to_contain_text("incus", timeout=30000)
    row.get_by_role("button", name="Start").click()
    wait_for(lambda: sibling(f"incus exec {CT} -- cat /etc/alpine-release")[0] == 0, "container running")
    rc, out = sibling(f"incus exec {CT} -- passwd -d root")
    assert rc == 0, out
    log("ok: Incus container created from the form (alpine) and started")

    # Console from the list row
    page.goto(f"{BASE}/containers")
    row = page.locator(f"tr[data-container-id='{CT}']")
    row.locator("a.container-console").click()
    page.wait_for_url(re.compile(rf".*/containers/{CT}/console\?runtime=incus"))
    expect(page.locator("#console-runtime")).to_have_value("incus")
    expect(page.get_by_text("● connected")).to_be_visible(timeout=30000)
    getty_login(page, page.locator("#console-input"), page.locator("#console-output"), "container console")
    no_cli_hint(page, "container console page")

    # Terminal (exec PTY)
    page.goto(f"{BASE}/containers/{CT}/exec?runtime=incus")
    expect(page.locator("#console-runtime")).to_have_value("incus")
    page.locator("#console-connect").click()
    expect(page.get_by_text("● connected")).to_be_visible(timeout=30000)
    cmd = page.locator("#console-input")
    cmd.fill("echo ui-pty-$((6*7))")
    cmd.press("Enter")
    expect(page.locator("#console-output")).to_contain_text("ui-pty-42", timeout=60000)
    log("ok: container terminal round-trip from the browser")
    no_cli_hint(page, "container terminal page")

    api("POST", f"/containers/{CT}/stop")
    api("DELETE", f"/containers/{CT}")


def run_vm(page):
    api("DELETE", f"/vms/{VM}")
    st, body = api("POST", "/vms", {
        "id": VM, "name": VM, "hypervisor": "incus", "memory": 1024, "cpus": 1,
        "disk_size": 10, "status": "stopped", "architecture": "x86_64",
    })
    assert st == 201, body
    st, body = api("POST", f"/vms/{VM}/start")
    assert st == 200, body
    wait_for(lambda: sibling(f"incus exec {VM} -- true")[0] == 0, "VM agent", 300)
    rc, out = sibling(f"incus exec {VM} -- passwd -d root")
    assert rc == 0, out

    page.goto(f"{BASE}/vms/{VM}/console")
    sel = page.locator("#console-type")
    expect(sel).to_have_value("serial", timeout=30000)
    assert sel.locator("option[value='vnc']").count() == 0, "VNC offered for an Incus VM"
    assert sel.locator("option[value='spice']").count() == 1
    getty_login(page, page.locator(".serial-input"), page.locator("#serial-output"), "VM serial console")
    no_cli_hint(page, "VM serial console page")

    sel.select_option("spice")
    expect(page.locator("#spice-endpoint")).to_contain_text("SPICE is listening on", timeout=60000)
    with page.expect_download() as dl:
        page.locator("#spice-vv").click()
    vv = open(dl.value.path()).read()
    assert "type=spice" in vv and "password=" in vv and re.search(r"port=\d+", vv), vv
    log("ok: SPICE panel shows the relay and the .vv downloads")
    no_cli_hint(page, "VM SPICE console page")

    api("POST", f"/vms/{VM}/stop")
    api("DELETE", f"/vms/{VM}")


def main():
    errors = []
    with sync_playwright() as p:
        browser = p.chromium.launch()
        # TLS trust in the browser is covered by ui_e2e.py; here only the pages matter.
        page = browser.new_page(accept_downloads=True, ignore_https_errors=True)
        page.on("console", lambda m: m.type == "error" and errors.append(m.text))
        page.on("dialog", lambda d: d.accept())
        login(page)
        try:
            if MODE == "vm":
                run_vm(page)
            else:
                run_ct(page)
        except BaseException:
            print(f"[incus-ui-e2e:{MODE}] page at failure ({page.url}):\n" + page.locator("body").inner_text()[-3000:])
            print(f"[incus-ui-e2e:{MODE}] browser console errors: {errors[-10:]}")
            print(sibling("sh -c \"grep -v 'Failed to broadcast' /tmp/api.log | tail -40; incus list\"")[1])
            raise
        browser.close()
    log("PASS")


if __name__ == "__main__":
    main()
