#!/usr/bin/env python3
"""Browser check of the Create VM form and the ISO Library (real Chromium).

Covers: CPU / memory / disk sliders whose stops come from the host, the
Custom override (limits + overcommit warning), the default network card,
a sound card with a VNC display (warning, not a block), picking an ISO,
uploading one from the form and downloading one from a URL with a
checksum, and the ISO Library page (no redundant "Download an ISO" button;
upload + download share the form's component).
"""
import hashlib
import http.server
import json
import os
import socket
import ssl
import tempfile
import threading
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL_CTX = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
VM = "e2e-form-vm"


def log(m):
    print(m, flush=True)


def api(method, path, body=None, tok=None):
    tok = tok or _tok()
    r = urllib.request.Request(f"{BASE}/api{path}", method=method,
                               data=json.dumps(body).encode() if body is not None else None,
                               headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r, context=SSL_CTX) as resp:
            raw = resp.read()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        return e.code, e.read()[:300]


_T = []


def _tok():
    if not _T:
        r = urllib.request.Request(f"{BASE}/api/auth/login", method="POST",
                                   data=json.dumps({"username": "admin", "password": "admin"}).encode(),
                                   headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(r, context=SSL_CTX) as resp:
            _T.append(json.load(resp)["ticket"])
    return _T[0]


def slider_to(page, rid, index):
    """Move a range input to a stop, as a user dragging it would."""
    page.eval_on_selector(f"#{rid}-slider", """(el, i) => {
        el.value = String(i);
        el.dispatchEvent(new Event('input', {bubbles: true}));
    }""", index)


def slider_max(page, rid):
    return int(page.get_attribute(f"#{rid}-slider", "max"))


def make_iso(size):
    f = tempfile.NamedTemporaryFile(suffix=".iso", delete=False, prefix="e2e-form-")
    f.write(os.urandom(size))
    f.close()
    return f.name


def main():
    for v in api("GET", "/vms")[1]:
        if v["name"] == VM:
            api("DELETE", f"/vms/{v['id']}")
    for i in api("GET", "/storage/isos")[1]:
        if i["name"].startswith("e2e-form-"):
            api("DELETE", f"/storage/isos/{i['name']}?storage={i['storage']}")
    opts = api("GET", "/vms/host-options")[1]
    host = opts["host"]
    errors = []

    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(ignore_https_errors=True)
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.goto(f"{BASE}/login")
        page.locator("input[type=text], input:not([type])").first.fill("admin")
        page.locator("input[type=password]").fill("admin")
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/")

        # ---- sliders: stops come from the host -----------------------------
        page.goto(f"{BASE}/vms/create")
        page.locator(".form-group", has_text="VM Name").locator("input").first.fill(VM)
        expect(page.locator("#vm-cpus-slider")).to_be_attached(timeout=30000)
        page.wait_for_function("document.querySelector('#vm-cpus-slider').max !== '0' || %d === 1" % host["cpus"])
        assert slider_max(page, "vm-cpus") == host["cpus"] - 1, "CPU stops are 1..host CPUs"
        slider_to(page, "vm-cpus", slider_max(page, "vm-cpus"))
        expect(page.locator("#vm-cpus-value")).to_have_text(f"{host['cpus']} vCPU" + ("" if host["cpus"] == 1 else "s"))
        slider_to(page, "vm-cpus", 0)
        expect(page.locator("#vm-cpus-value")).to_have_text("1 vCPU")
        assert page.locator("#vm-cpus-input").count() == 0, "free typing is hidden until Custom is ticked"
        log("ok: CPU slider spans 1..host CPUs")

        # memory: 512 MB, 1 GB, 2 GB ... below host RAM minus the reserve
        mem_stops = page.eval_on_selector("#vm-memory-slider", "e => Number(e.max)") + 1
        assert mem_stops >= 2
        slider_to(page, "vm-memory", 0)
        expect(page.locator("#vm-memory-value")).to_have_text("512 MB")
        slider_to(page, "vm-memory", 3 if mem_stops > 3 else mem_stops - 1)
        want = "4 GB" if mem_stops > 3 else None
        if want:
            expect(page.locator("#vm-memory-value")).to_have_text(want)
        slider_to(page, "vm-memory", mem_stops - 1)
        top = page.inner_text("#vm-memory-value")
        assert page.locator("#vm-memory-warning").count() == 0, "the top stop is never an overcommit"
        log(f"ok: memory slider 512 MB .. {top}")

        # disk: stops up to the storage's free space (4, 8, 16 ... GB)
        slider_to(page, "vm-disk", 0)
        expect(page.locator("#vm-disk-value")).to_have_text("4 GB")
        slider_to(page, "vm-disk", 1)
        expect(page.locator("#vm-disk-value")).to_have_text("8 GB")
        log("ok: disk slider")

        # ---- Custom override: validated input, warns on overcommit ---------
        page.check("#vm-cpus-custom")
        inp = page.locator("#vm-cpus-input")
        expect(inp).to_be_visible()
        expect(page.locator("#vm-cpus-slider")).to_be_disabled()
        inp.fill(str(host["cpus"] + 1))
        expect(page.locator("#vm-cpus-warning")).to_be_visible()
        expect(page.locator("#vm-cpus-warning")).to_contain_text("more than this host has")
        inp.fill("0")
        assert not inp.evaluate("e => e.checkValidity()"), "0 CPUs is invalid"
        inp.fill(str(host["cpus"] * 100))
        assert not inp.evaluate("e => e.checkValidity()"), "far above the limit is invalid"
        inp.fill("2")
        assert inp.evaluate("e => e.checkValidity()")
        expect(page.locator("#vm-cpus-warning")).to_have_count(0)
        expect(page.locator("#vm-cpus-value")).to_have_text("2 vCPUs")
        page.uncheck("#vm-cpus-custom")
        expect(page.locator("#vm-cpus-slider")).to_be_enabled()
        # memory in MB, disk in GB: an unusual value no slider stop has
        page.check("#vm-memory-custom")
        page.fill("#vm-memory-input", "1536")
        expect(page.locator("#vm-memory-value")).to_have_text("1.5 GB")
        page.check("#vm-disk-custom")
        page.fill("#vm-disk-input", "10")
        expect(page.locator("#vm-disk-value")).to_have_text("10 GB")
        page.fill("#vm-disk-input", str(opts["host"]["default_disk_available_gb"] + 1000))
        expect(page.locator("#vm-disk-warning")).to_be_visible()
        page.fill("#vm-disk-input", "10")
        log("ok: Custom override validates and warns on overcommit")

        # ---- default network card -------------------------------------------
        dn = opts["default_networks"]
        want_bridge = dn.get("override") or dn.get("qemu")
        if want_bridge:
            expect(page.locator(".vm-nic-row")).to_have_count(1)
            assert page.locator(".vm-nic-bridge").first.input_value() == want_bridge
            log(f"ok: new VM gets a NIC on the default network ({want_bridge})")
        else:
            log("skip: this host has no bridge for a default NIC")

        # ---- sound with VNC: warning, not a block ----------------------------
        page.select_option("#disp-protocol", "vnc")
        expect(page.locator("#disp-sound")).to_be_enabled()
        assert page.locator("#disp-sound-warning").count() == 0
        page.select_option("#disp-sound", "intel-hda")
        expect(page.locator("#disp-sound-warning")).to_be_visible()
        expect(page.locator("#disp-sound-warning")).to_contain_text("SPICE")
        page.select_option("#disp-protocol", "spice")
        expect(page.locator("#disp-sound-warning")).to_have_count(0)
        page.select_option("#disp-protocol", "vnc")
        expect(page.locator("#disp-sound-warning")).to_be_visible()
        assert page.locator("#disp-sound").input_value() == "intel-hda", "switching protocol keeps the sound card"
        log("ok: sound card selectable with VNC, with a warning")

        # ---- ISO: upload from the form, pick it ------------------------------
        panel = page.locator("#vm-iso-add")
        if not panel.evaluate("e => e.open"):
            panel.locator("summary").click()
        expect(page.locator("#vm-iso-storage option").first).to_be_attached()
        assert page.locator("#vm-iso-storage").input_value() == opts["default_iso_storage"]
        iso_path = make_iso(300_000)
        iso_sha = hashlib.sha256(open(iso_path, "rb").read()).hexdigest()
        iso_name = os.path.basename(iso_path)
        # a wrong checksum is refused with the reason
        page.set_input_files("#vm-iso-file", iso_path)
        page.fill("#vm-iso-sha256", "0" * 64)
        page.click("#vm-iso-upload")
        expect(page.locator("#vm-iso-error")).to_contain_text("sha256 mismatch", timeout=30000)
        page.fill("#vm-iso-sha256", iso_sha)
        page.click("#vm-iso-upload")
        expect(page.locator("#vm-iso-status")).to_contain_text("Uploaded", timeout=60000)
        expect(page.locator("#vm-cdrom option", has_text=iso_name)).to_have_count(1, timeout=30000)
        sel = page.locator("#vm-cdrom").evaluate("e => e.options[e.selectedIndex].text")
        assert iso_name in sel, f"the uploaded ISO is selected: {sel!r}"
        log("ok: ISO uploaded from the Create VM form and selected")

        # ---- ISO: download from a URL with a checksum ------------------------
        d = tempfile.mkdtemp()
        blob = os.urandom(200_000)
        open(f"{d}/e2e-form-dl.iso", "wb").write(blob)
        handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=d, **k)
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        probe.connect(("192.0.2.1", 9))
        mirror = probe.getsockname()[0]
        probe.close()
        srv = http.server.ThreadingHTTPServer((mirror, 0), handler)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        url = f"http://{mirror}:{srv.server_address[1]}/e2e-form-dl.iso"
        page.fill("#vm-iso-url", url)
        page.fill("#vm-iso-sha256", "f" * 64)
        page.click("#vm-iso-download")
        expect(page.locator("#vm-iso-error")).to_contain_text("sha256 mismatch", timeout=60000)
        page.fill("#vm-iso-filename", "e2e-form-dl2.iso")
        page.fill("#vm-iso-sha256", hashlib.sha256(blob).hexdigest())
        page.click("#vm-iso-download")
        expect(page.locator("#vm-iso-status")).to_contain_text("Downloaded", timeout=60000)
        expect(page.locator("#vm-cdrom option", has_text="e2e-form-dl2.iso")).to_have_count(1, timeout=30000)
        log("ok: ISO downloaded from a URL with a checksum from the Create VM form")

        # ---- create: everything lands in the VM -------------------------------
        page.select_option("#vm-cdrom", index=[
            i for i, t in enumerate(page.locator("#vm-cdrom option").all_inner_texts()) if iso_name in t][0])
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/vms", timeout=60000)
        vm = next(v for v in api("GET", "/vms")[1] if v["name"] == VM)
        assert vm["cpus"] == 2 and vm["memory"] == 1536 and vm["disk_size"] == 10, vm
        assert vm["display"]["protocol"] == "vnc" and vm["display"]["sound"] == "intel-hda", vm["display"]
        assert vm["hardware"]["cdrom"].endswith(iso_name), vm["hardware"]
        if want_bridge:
            assert vm["hardware"]["nets"] and vm["hardware"]["nets"][0]["bridge"] == want_bridge, vm["hardware"]
        log("ok: VM created with slider/custom values, sound on VNC, ISO and default NIC")
        api("DELETE", f"/vms/{vm['id']}")

        # ---- ISO Library: no redundant button, upload + download -------------
        for i in api("GET", "/storage/isos")[1]:
            api("DELETE", f"/storage/isos/{i['name']}?storage={i['storage']}")
        page.goto(f"{BASE}/storage/isos")
        expect(page.locator("#iso-empty")).to_be_visible(timeout=30000)
        expect(page.locator("#iso-empty")).to_contain_text("above")
        assert page.get_by_role("button", name="Download an ISO").count() == 0, "redundant empty-state button is gone"
        assert page.locator("#iso-empty button").count() == 0
        expect(page.locator("#iso-upload")).to_be_visible()
        expect(page.locator("#iso-download")).to_be_visible()
        lib = make_iso(120_000)
        page.set_input_files("#iso-file", lib)
        page.click("#iso-upload")
        expect(page.locator(f"tr.iso-row[data-iso='{os.path.basename(lib)}']")).to_have_count(1, timeout=60000)
        expect(page.locator("#iso-empty")).to_have_count(0)
        page.fill("#iso-url", url)
        page.fill("#iso-filename", "e2e-form-dl3.iso")
        page.click("#iso-download")
        expect(page.locator("tr.iso-row[data-iso='e2e-form-dl3.iso'] .iso-status")).to_have_text("ready", timeout=60000)
        log("ok: ISO Library: empty state without the button; upload and download work")

        # ---- Settings: global default network ---------------------------------
        bridges = [b["name"] for b in opts["bridges"]]
        if bridges:
            page.goto(f"{BASE}/system/config")
            page.get_by_role("button", name="Network", exact=True).click()
            expect(page.locator("#vm-default-network-select")).to_be_visible(timeout=30000)
            pick = bridges[-1]
            page.select_option("#vm-default-network-select", pick)
            page.click("#vm-default-network-save")
            expect(page.locator("#vm-default-network-msg")).to_contain_text("saved", timeout=30000)
            saved = api("GET", "/vms/defaults")[1]
            assert saved.get("default_network") == pick, f"Settings saved {saved}, picked {pick}"
            page.goto(f"{BASE}/vms/create")
            expect(page.locator(".vm-nic-row")).to_have_count(1, timeout=30000)
            expect(page.locator(".vm-nic-bridge").first).to_have_value(pick, timeout=30000)
            page.goto(f"{BASE}/system/config")
            page.get_by_role("button", name="Network", exact=True).click()
            page.select_option("#vm-default-network-select", "")
            page.click("#vm-default-network-save")
            expect(page.locator("#vm-default-network-msg")).to_contain_text("saved", timeout=30000)
            log(f"ok: global default network override ({pick}) applies to new VMs")

        for i in api("GET", "/storage/isos")[1]:
            if i["name"].startswith("e2e-form-") or i["name"] == os.path.basename(lib):
                api("DELETE", f"/storage/isos/{i['name']}?storage={i['storage']}")
        browser.close()
    assert not errors, f"JS errors: {errors}"
    log("PASS vm_create_form_e2e")


if __name__ == "__main__":
    main()
