#!/usr/bin/env python3
"""Browser E2E of the LXC pages against a real horcrux-api running with LXC.

CI's "UI E2E" job starts the API + this build's UI inside a privileged LXC
sibling container (horcrux-api/tests/lxc_e2e.sh serve) that has a VLAN-aware
bridge `hxbr0` (10.77.0.1/24, DHCP). Everything here is done the way a user
does it in the browser; each effect is then checked independently through
the API (container exec reads cgroup values / addresses inside the real
container).
"""
import json
import os
import re
import time
import urllib.error
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "http://127.0.0.1:8007")
CT = "e2e-lxc"
_TOKEN = []


def token():
    if not _TOKEN:
        r = urllib.request.Request(
            f"{BASE}/api/auth/login",
            data=json.dumps({"username": "admin", "password": "admin"}).encode(),
            headers={"Content-Type": "application/json"},
        )
        body = json.load(urllib.request.urlopen(r, timeout=30))
        _TOKEN.append(body.get("ticket") or body.get("token"))
    return _TOKEN[0]


def api(method, path, body=None, timeout=600):
    r = urllib.request.Request(
        f"{BASE}/api{path}",
        data=None if body is None else json.dumps(body).encode(),
        method=method,
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {token()}"},
    )
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            text = resp.read().decode()
            return resp.status, (json.loads(text) if text else None)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def ct_exec(cmd):
    status, body = api("POST", f"/containers/{CT}/exec", {"command": ["sh", "-c", cmd]})
    assert status == 200, (status, body)
    return body["output"].strip()


def wait_for(fn, what, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        try:
            if fn():
                return True
        except AssertionError:
            pass
        time.sleep(2)
    raise AssertionError(f"timed out: {what}")


def main():
    status, cat = api("GET", "/lxc/templates/catalog?arch=amd64&distro=alpine")
    assert status == 200, cat
    rels = [e for e in cat if e["variant"] == "default" and re.fullmatch(r"\d+\.\d+", e["release"])]
    newest = max(rels, key=lambda e: tuple(int(x) for x in e["release"].split(".")))
    tid = newest["id"]
    print(f"[lxc-ui-e2e] using template {tid}")

    console_errors = []
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.on("console", lambda m: m.type == "error" and console_errors.append(m.text))
        page.on("dialog", lambda d: d.accept())

        page.goto(f"{BASE}/login")
        page.locator("input[type=text], input:not([type])").first.fill("admin")
        page.locator("input[type=password]").fill("admin")
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/")

        # ---- pveam: catalog -> download ---------------------------------
        page.goto(f"{BASE}/containers")
        page.locator("#lxc-templates-btn").click()
        page.wait_for_url(f"{BASE}/containers/lxc/templates")
        page.fill("#lxc-catalog-filter", f"alpine/{newest['release']}/amd64")
        row = page.locator(f"tr[data-catalog-id='{tid}']")
        expect(row).to_be_visible(timeout=60000)
        row.locator("button.lxc-download").click()
        expect(page.locator(".lxc-ok")).to_contain_text("sha256 verified", timeout=300000)
        trow = page.locator(f"tr[data-template-id='{tid}']")
        expect(trow).to_be_visible()

        # ---- pct create from the form -----------------------------------
        trow.get_by_role("link", name="Create container").click()
        page.wait_for_url(re.compile(r".*/containers/create\?runtime=lxc.*"))
        expect(page.locator("#lxc-create-section")).to_be_visible(timeout=30000)
        expect(page.locator("#ct-image")).to_have_value(tid)
        page.fill("#ct-name", CT)
        page.fill("#ct-memory", "128")
        expect(page.locator("#lxc-bridge option[value='hxbr0']")).to_have_count(1, timeout=30000)
        page.locator("#lxc-bridge").select_option("hxbr0")
        page.locator("#lxc-ip-mode").select_option("static")
        page.fill("#lxc-ip", "10.77.0.70/24")
        page.fill("#lxc-gw", "10.77.0.1")
        page.fill("#lxc-disk", "1")
        page.fill("#lxc-mounts", "mp=/data")
        page.locator("#lxc-nesting").check()
        page.locator("#ct-submit").click()
        page.wait_for_url(f"{BASE}/containers", timeout=300000)
        row = page.locator(f"tr[data-container-id='{CT}']")
        expect(row).to_contain_text("lxc", timeout=30000)
        expect(row).to_contain_text("running", timeout=60000)
        wait_for(lambda: "10.77.0.70/24" in ct_exec("ip -4 addr show eth0"), "static IP from the form")
        assert ct_exec("cat /sys/fs/cgroup/memory.max") == str(128 << 20)
        assert ct_exec("test -d /data && echo y") == "y"

        # ---- manage page: pct set (live limit) --------------------------
        row.locator("a.lxc-manage").click()
        page.wait_for_url(f"{BASE}/containers/{CT}/lxc")
        expect(page.locator("#lxc-status")).to_have_text("running", timeout=30000)
        expect(page.locator("#lxc-raw-config")).to_contain_text("lxc.cgroup2.memory.max = 134217728")
        expect(page.locator("#lxc-nets")).to_have_value(re.compile(r"bridge=hxbr0,ip=10\.77\.0\.70/24"))
        page.fill("#lxc-memory", "192")
        page.locator("#lxc-save").click()
        expect(page.locator(".lxc-ok")).to_contain_text("Settings saved", timeout=60000)
        assert ct_exec("cat /sys/fs/cgroup/memory.max") == str(192 << 20)

        # ---- pending change + reboot ------------------------------------
        page.fill("#lxc-hostname", "ui-renamed")
        page.locator("#lxc-save").click()
        expect(page.locator("#lxc-pending")).to_contain_text("hostname", timeout=60000)
        page.locator("#lxc-reboot").click()
        expect(page.locator(".lxc-ok")).to_contain_text("reboot: done", timeout=180000)
        expect(page.locator("#lxc-pending")).to_have_count(0, timeout=30000)
        assert ct_exec("hostname") == "ui-renamed"

        # ---- snapshot + resize ------------------------------------------
        page.fill("#lxc-snap-name", "ui-snap")
        page.locator("#lxc-snap-create").click()
        expect(page.locator("tr[data-snapshot='ui-snap']")).to_be_visible(timeout=180000)
        page.fill("#lxc-resize-gb", "2")
        page.locator("#lxc-resize").click()
        expect(page.locator(".lxc-ok")).to_contain_text("2 GB", timeout=120000)
        size = int(ct_exec("df -Pm / | tail -1 | awk '{print $2}'"))
        assert size >= 1800, size

        # ---- web terminal (pct enter) -----------------------------------
        page.goto(f"{BASE}/containers/{CT}/exec?runtime=lxc")
        expect(page.locator("#console-runtime")).to_have_value("lxc")
        page.get_by_role("button", name="Connect").click()
        expect(page.get_by_text("● connected")).to_be_visible(timeout=30000)
        expect(page.locator("pre")).to_contain_text("#", timeout=30000)  # shell prompt
        cmd = page.get_by_placeholder("type a command, press Send")
        cmd.fill("echo ui-pty-$((6*7))")
        cmd.press("Enter")
        expect(page.locator("pre")).to_contain_text("ui-pty-42", timeout=30000)

        # ---- shutdown, rollback while stopped, delete -------------------
        page.goto(f"{BASE}/containers/{CT}/lxc")
        page.locator("#lxc-shutdown").click()
        expect(page.locator("#lxc-status")).to_have_text("stopped", timeout=120000)
        page.locator("tr[data-snapshot='ui-snap'] button.lxc-rollback").click()
        expect(page.locator(".lxc-ok")).to_contain_text("Rolled back to ui-snap", timeout=120000)
        page.goto(f"{BASE}/containers")
        row = page.locator(f"tr[data-container-id='{CT}']")
        row.get_by_role("button", name="Delete").click()
        expect(page.locator(f"tr[data-container-id='{CT}']")).to_have_count(0, timeout=60000)
        status, lst = api("GET", "/containers")
        assert not any(c["id"] == CT for c in lst), lst

        # ---- delete the template ----------------------------------------
        page.goto(f"{BASE}/containers/lxc/templates")
        page.locator(f"tr[data-template-id='{tid}']").get_by_role("button", name="Delete").click()
        expect(page.locator(f"tr[data-template-id='{tid}']")).to_have_count(0, timeout=30000)
        browser.close()

    bad = [e for e in console_errors if "missing field" in e or "panicked" in e]
    assert not bad, f"console errors: {bad}"
    print("LXC UI E2E: all checks passed")


if __name__ == "__main__":
    main()
