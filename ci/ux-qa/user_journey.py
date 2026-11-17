#!/usr/bin/env python3
"""USER-JOURNEY QA: what a first-time administrator does, in a real browser, against a fresh install.

Unlike ux_crawl.py (every page loads cleanly) and the feature e2e tests (one feature, driven the way its developer
expects), this walks the product like a new user and asserts what a person would conclude:

  J1 first login            log in as the admin account, land somewhere useful
  J2 iso library            see the library, get an ISO into it from the page (upload or URL), no redundant buttons
  J3 create vm              the form offers sensible CPU/RAM/disk choices, a network card by default, sound over VNC,
                            the ISO in the CD-ROM list; create it with an ISO + network + sound + console
  J4 boot the iso           start it: QEMU really got the ISO, a NIC and the sound device, the guest runs the ISO
                            (the test ISO prints a marker and powers the VM off - a VM that never booted it stays up),
                            the console shows the guest screen
  J5 create container       the container form renders and creates, or says plainly why it cannot
  J6 storage / networks     pools, bridges, DHCP pages: content, and a way to add one
  J7 backups / snapshots    the VM can be snapshotted from the UI; backup pages explain themselves
  J8 settings / updates     users, certificates, configuration, package updates are reachable and not empty

Every step takes a screenshot (--shots DIR/NN-name.png), records JS console errors (any = failure) and asserts the
OUTCOME, not the click. Findings are ids `journey:<step>:<check>`; known/tracked ones live in
ci/ux-qa/user-journey-baseline.json (a NEW failure fails the run; a baseline entry that no longer fails is reported so
it gets removed). The screenshots then go to the LLM review (ci/ux-qa/journey-review-checklist.md).

  BASE_URL=https://host:8006 ADMIN_PASSWORD=... user_journey.py --shots out/shots --report out/journey.json
Env: HORCRUX_CA_FILE (trust the node's CA; else TLS errors are ignored), HX_EXEC_PREFIX (e.g. "docker exec -i ctr":
the server runs elsewhere - ISO is pushed and QEMU command lines are read through it), HX_JOURNEY_BOOT_TIMEOUT (s),
HX_BROWSER (chromium|firefox).
"""
import argparse
import json
import os
import re
import shlex
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import http.server
import functools

HERE = os.path.dirname(os.path.abspath(__file__))
BASE = os.environ.get("BASE_URL", "https://localhost:8006").rstrip("/")
PASSWORD = os.environ.get("ADMIN_PASSWORD", "admin")
CA = os.environ.get("HORCRUX_CA_FILE")
PREFIX = shlex.split(os.environ.get("HX_EXEC_PREFIX", ""))
BOOT_TIMEOUT = int(os.environ.get("HX_JOURNEY_BOOT_TIMEOUT", "240"))
SSLCTX = ssl.create_default_context(cafile=CA) if CA else ssl._create_unverified_context()
VM_NAME = "journey-vm"
CT_NAME = "journey-ct"
MARKER = "HORCRUX-JOURNEY-ISO-BOOTED"

state = {"bridge": True}
findings = []   # {id, sev, detail, step}
steps = []      # {n, id, title, shot, ok, console_errors, notes}
_console = []   # console errors seen since the step started


def finding(step, check, detail, sev="fail"):
    findings.append({"id": f"journey:{step}:{check}", "sev": sev, "detail": detail[:400], "step": step})
    print(f"  [{sev.upper()}] journey:{step}:{check} - {detail[:200]}", flush=True)


def expect(step, check, cond, detail):
    if not cond:
        finding(step, check, detail)
    return bool(cond)


# --------------------------------------------------------------------------------------------------- API helpers
_tok = []


def api(method, path, body=None, ok=(200, 201, 202, 204)):
    if not _tok:
        r = urllib.request.Request(BASE + "/api/auth/login", data=json.dumps({"username": "admin", "password": PASSWORD}).encode(),
                                   headers={"Content-Type": "application/json"})
        _tok.append(json.load(urllib.request.urlopen(r, context=SSLCTX))["ticket"])
    data = None if body is None else json.dumps(body).encode()
    r = urllib.request.Request(BASE + path, data=data, method=method,
                               headers={"Authorization": "Bearer " + _tok[0], "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r, context=SSLCTX, timeout=120) as resp:
            raw = resp.read()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except ValueError:
            return e.code, raw.decode("utf8", "replace")


def run_on_server(cmd, stdin_path=None, timeout=60):
    """Run a shell command where the server runs (HX_EXEC_PREFIX) or locally."""
    stdin = open(stdin_path, "rb") if stdin_path else None
    try:
        return subprocess.run(PREFIX + ["sh", "-c", cmd], stdin=stdin, capture_output=True, text=True, timeout=timeout)
    finally:
        if stdin:
            stdin.close()


def qemu_cmdlines():
    r = run_on_server("for f in /proc/[0-9]*/cmdline; do tr '\\000' ' ' < $f 2>/dev/null; echo; done | grep 'qemu-system' || true")
    return [l for l in r.stdout.splitlines() if "qemu-system" in l and "grep" not in l]


# --------------------------------------------------------------------------------------------------- test ISO
def make_iso(path):
    """A tiny BIOS-bootable ISO whose GRUB prints MARKER, waits, and powers the machine off."""
    work = tempfile.mkdtemp(prefix="journey-iso-")
    try:
        os.makedirs(f"{work}/boot/grub")
        open(f"{work}/boot/grub/grub.cfg", "w").write(
            f'set timeout=0\nterminal_output console\necho "{MARKER}"\necho "This is the journey test ISO."\nsleep 8\nhalt\n')
        # BIOS-only image: needs no mtools/EFI modules (the guest is a default SeaBIOS machine)
        bios = "/usr/lib/grub/i386-pc"
        cmd = ["grub-mkrescue"] + (["-d", bios] if os.path.isdir(bios) else []) + ["-o", path, work, "--", "-volid", "JOURNEY"]
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
        if r.returncode != 0 or not os.path.exists(path):
            raise RuntimeError("grub-mkrescue failed: " + r.stderr[-300:])
    finally:
        shutil.rmtree(work, ignore_errors=True)


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass


def serve_dir(d):
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=d))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


# --------------------------------------------------------------------------------------------------- browser
class Journey:
    def __init__(self, page, shots):
        self.page, self.shots, self.n = page, shots, 0

    def go(self, path, settle=1.2):
        self.page.goto(BASE + path, wait_until="load")
        try:
            self.page.wait_for_function("() => document.body && document.body.innerText.trim().length > 20", timeout=15000)
        except Exception:  # noqa: BLE001
            pass
        self.page.wait_for_timeout(int(settle * 1000))

    def shot(self, name, full=True):
        self.n += 1
        fn = f"{self.n:02d}-{name}.png"
        if self.shots:
            os.makedirs(self.shots, exist_ok=True)
            self.page.screenshot(path=os.path.join(self.shots, fn), full_page=full)
        return fn

    def text(self):
        return self.page.inner_text("body")


def step(j, sid, title):
    """Decorator-free helper: returns a context object."""
    class Ctx:
        def __enter__(self_):
            _console.clear()
            self_.f0 = len(findings)
            self_.t = time.time()
            print(f"== {sid}: {title}", flush=True)
            return self_

        def __exit__(self_, et, ev, tb):
            if et is not None:
                finding(sid, "crashed", f"{et.__name__}: {ev}")
                try:
                    j.shot(f"{sid}-crash")
                except Exception:  # noqa: BLE001
                    pass
            errs = list(dict.fromkeys(_console))
            if errs:
                finding(sid, "js-console-errors", "; ".join(errs)[:380])
            steps.append({"id": sid, "title": title, "ok": len(findings) == self_.f0, "console_errors": errs})
            return True   # a failing step never stops the journey
    return Ctx()


def select_options(page, sel):
    return page.eval_on_selector(sel, "e => [...e.options].map(o => ({v:o.value,t:o.textContent.trim(),d:o.disabled}))") if page.locator(sel).count() else []


def vm_by_name(name):
    code, vms = api("GET", "/api/vms")
    return next((v for v in (vms or []) if v.get("name") == name), None) if code == 200 else None


def main():
    from playwright.sync_api import sync_playwright   # lazy: the unit tests import this module without a browser

    ap = argparse.ArgumentParser()
    ap.add_argument("--shots")
    ap.add_argument("--report")
    ap.add_argument("--baseline", default=os.path.join(HERE, "user-journey-baseline.json"))
    ap.add_argument("--iso", help="a bootable test ISO (default: built with grub-mkrescue)")
    a = ap.parse_args()

    tmp = tempfile.mkdtemp(prefix="journey-")
    iso = a.iso
    if not iso:
        iso = os.path.join(tmp, "journey-test.iso")
        make_iso(iso)
    iso_name = os.path.basename(iso)
    srv = serve_dir(os.path.dirname(os.path.abspath(iso)))
    iso_url = f"http://127.0.0.1:{srv.server_address[1]}/{iso_name}"

    with sync_playwright() as p:
        browser = getattr(p, os.environ.get("HX_BROWSER", "chromium")).launch()
        ctx = browser.new_context(viewport={"width": 1366, "height": 900}, ignore_https_errors=not CA)
        page = ctx.new_page()
        # "Failed to load resource" console lines carry no URL: failed API calls are recorded from the response instead
        page.on("console", lambda m: _console.append(f"console.{m.type}: {m.text[:160]}") if m.type == "error" and "favicon" not in m.text and "Failed to load resource" not in m.text else None)
        page.on("response", lambda r: _console.append(f"http {r.status} {r.request.method} {r.url.replace(BASE, '')[:90]}")
                if r.status >= 400 and "/api/" in r.url and not (r.status == 401 and "/api/auth/" in r.url) and not (r.status == 403 and r.url.endswith("/api/storage/isos/download")) and "favicon" not in r.url else None)
        page.on("pageerror", lambda e: _console.append(f"pageerror: {str(e)[:160]}"))
        j = Journey(page, a.shots)

        # ------------------------------------------------------------------ J1 first login
        with step(j, "j1-login", "first login") as _:
            page.goto(BASE + "/", wait_until="load")
            page.wait_for_timeout(1500)
            j.shot("first-visit")
            expect("j1-login", "redirect-to-login", "/login" in page.url, f"unauthenticated visit did not land on the login page ({page.url})")
            page.locator("input[type=password]").first.wait_for(timeout=30000)
            page.locator("input[type=text],input[name=username],input:not([type])").first.fill("admin")
            page.locator("input[type=password]").first.fill(PASSWORD)
            page.locator("button[type=submit],button:has-text('Log in'),button:has-text('Sign in')").first.click()
            page.wait_for_function("() => !location.pathname.startsWith('/login')", timeout=30000)
            page.wait_for_timeout(2500)
            j.shot("dashboard")
            body = j.text()
            expect("j1-login", "dashboard-content", len(body) > 200 and "Dashboard" in body, "after login the page is not a dashboard")
            expect("j1-login", "no-nan-or-undefined", not re.search(r"\b(NaN|undefined|null)\b", body), "dashboard shows NaN/undefined/null")

        # ------------------------------------------------------------------ J2 ISO library
        with step(j, "j2-iso-library", "ISO library: empty state, getting an ISO in") as _:
            j.go("/storage/isos")
            j.shot("iso-library-empty")
            body = j.text()
            expect("j2-iso-library", "empty-state-no-redundant-button",
                   not (re.search(r"download one above", body, re.I) and page.locator("#iso-table button:has-text('Download')").count()),
                   "empty state says 'Download one above' next to a second, redundant Download button")
            has_upload = page.locator("input[type=file]").count() > 0 or page.locator("button:has-text('Upload'),label:has-text('Upload')").count() > 0
            expect("j2-iso-library", "upload-option", has_upload, "ISO Library offers no way to upload an ISO from the browser (only download from a URL)")
            # get the test ISO in: upload if the page can, else download from a URL, else place the file (and fail the step)
            got = False
            if page.locator("input[type=file]").count():
                page.locator("input[type=file]").first.set_input_files(iso)
                page.wait_for_timeout(1500)
                for b in page.locator("button:has-text('Upload')").all():
                    if b.is_visible() and b.is_enabled():
                        b.click()
                        break
            elif page.locator("#iso-url").count():
                # a loopback URL is refused by the server (SSRF guard) - the page must SAY so, not stay silent
                page.fill("#iso-url", iso_url)
                page.click("#iso-download")
                page.wait_for_timeout(2000)
                j.shot("iso-library-download-feedback")
                expect("j2-iso-library", "download-gives-feedback", page.locator("#iso-error, #iso-notice").count() > 0,
                       "submitting a download URL showed no message at all (neither progress nor an error)")
            for _i in range(60):
                code, lst = api("GET", "/api/storage/isos")
                if code == 200 and any(i.get("name") == iso_name and not i.get("downloading") and not i.get("error") for i in lst):
                    got = True
                    break
                time.sleep(1)
            if not got:
                if has_upload:
                    finding("j2-iso-library", "iso-arrives", "the ISO did not appear in the library via the page (upload/URL)")
                code, opts = api("GET", "/api/vms/host-options")
                d = (opts or {}).get("iso_dir", "/var/lib/horcrux/isos") if code == 200 else "/var/lib/horcrux/isos"
                run_on_server(f"mkdir -p {shlex.quote(d)} && cat > {shlex.quote(d + '/' + iso_name)}", stdin_path=iso, timeout=120)
            j.go("/storage/isos")
            j.shot("iso-library-with-iso")
            expect("j2-iso-library", "iso-listed", iso_name in j.text(), "the ISO is not listed on the page after it was added")

        # ------------------------------------------------------------------ J3 create VM
        with step(j, "j3-create-vm", "create VM form") as _:
            code, opts = api("GET", "/api/vms/host-options")
            if code == 200 and not (opts or {}).get("bridges"):
                j.go("/vms/create", settle=2.5)
                j.shot("create-vm-no-bridge")
                finding("j3-create-vm", "fresh-host-no-bridge", "a fresh install has no network bridge, so a VM cannot get a network card; "
                        "the create form should offer to create one (or default to NAT/user networking) instead of failing at Create")
                run_on_server("ip link add vmbr0 type bridge && ip link set vmbr0 up", timeout=20)   # the rest of the journey needs one
                code, opts = api("GET", "/api/vms/host-options")
                state["bridge"] = bool((opts or {}).get("bridges"))   # an unprivileged runner cannot make one: the VM is then created without a NIC
            j.go("/vms/create", settle=2.5)
            j.shot("create-vm-top")
            n_range = page.locator("form input[type=range]").count()
            expect("j3-create-vm", "resource-sliders", n_range >= 3,
                   f"CPU/RAM/disk are plain number boxes ({n_range} sliders): no sliders, supported steps or presets with a custom override")
            expect("j3-create-vm", "nic-by-default", page.locator(".vm-nic-row").count() >= 1,
                   "a new VM form has no network card by default")
            snd = select_options(page, "#disp-sound")
            proto = select_options(page, "#disp-protocol")
            if page.locator("#disp-protocol").count():
                page.select_option("#disp-protocol", "vnc") if any(o["v"] == "vnc" for o in proto) else None
            snd = select_options(page, "#disp-sound")
            usable = [o for o in snd if o["v"] not in ("", "none") and not o["d"]] if page.locator("#disp-sound").count() and page.locator("#disp-sound").is_enabled() else []
            expect("j3-create-vm", "sound-with-vnc", bool(usable),
                   f"no sound device can be chosen while the display is VNC (options: {[o['t'] for o in snd]})")
            cd = select_options(page, "#vm-cdrom")
            iso_opt = [o for o in cd if iso_name in o["t"] or iso_name in o["v"]]
            expect("j3-create-vm", "cdrom-lists-iso", bool(iso_opt), f"CD-ROM list does not offer the ISO in the library (options: {[o['t'] for o in cd]})")
            expect("j3-create-vm", "cdrom-add-iso-from-form",
                   page.locator("form input[type=file], form button:has-text('Upload'), form a:has-text('ISO Library'), form a:has-text('Add ISO'), form button:has-text('Add ISO')").count() > 0,
                   "no way to add/upload an ISO from the create form itself (and no link to the ISO Library)")
            page.fill("form input[placeholder='my-vm']", VM_NAME)
            if iso_opt:
                page.select_option("#vm-cdrom", iso_opt[0]["v"])
                if page.locator("#vm-boot").count():
                    page.select_option("#vm-boot", "cdrom")
            if usable:
                preferred = next((o for o in usable if "ac97" in o["v"].lower() or "hda" in o["v"].lower() or "ich9" in o["v"].lower()), usable[0])
                page.select_option("#disp-sound", preferred["v"])
            if state["bridge"] and page.locator(".vm-nic-row").count() == 0 and page.locator("#vm-nic-add").count():
                page.click("#vm-nic-add")   # keep going: the later steps test the NIC at runtime
            if not state["bridge"]:
                for b in page.locator(".vm-nic-remove").all():
                    b.click()   # nothing to attach it to: create without, so the boot journey still runs
            page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
            j.shot("create-vm-filled")
            page.click("button[type=submit]:has-text('Create VM')")
            try:
                page.wait_for_function("() => location.pathname === '/vms'", timeout=60000)
            except Exception:  # noqa: BLE001
                finding("j3-create-vm", "created", "creating the VM did not return to the VM list: " + j.text()[-300:].replace("\n", " "))
            page.wait_for_timeout(1500)
            j.shot("vm-list-after-create")
            vm = vm_by_name(VM_NAME)
            expect("j3-create-vm", "created", vm is not None, "VM does not exist after the form was submitted")
            if vm:
                code, full = api("GET", f"/api/vms/{vm['id']}")
                hw = (full or {}).get("hardware") or {}
                expect("j3-create-vm", "saved-nic", len(hw.get("nets") or []) >= 1 or not state["bridge"], "the created VM has no network interface")
                expect("j3-create-vm", "saved-cdrom", bool(hw.get("cdrom")) and iso_name in str(hw.get("cdrom")), f"the created VM's CD-ROM is {hw.get('cdrom')!r}, not the chosen ISO")
                snd_saved = ((full or {}).get("display") or {}).get("sound")
                if usable:
                    expect("j3-create-vm", "saved-sound", snd_saved not in (None, "none"), f"sound device was not saved ({snd_saved!r})")

        # ------------------------------------------------------------------ J4 boot the ISO
        vm = vm_by_name(VM_NAME)
        with step(j, "j4-boot-iso", "start the VM and boot the ISO") as _:
            if not vm:
                finding("j4-boot-iso", "vm-exists", "no VM to boot (J3 failed)")
            else:
                vid = vm["id"]
                page.goto(BASE + "/vms", wait_until="load")
                page.wait_for_timeout(2000)
                row = page.locator("tr", has_text=VM_NAME).first
                start = row.locator("button:has-text('Start')").first
                expect("j4-boot-iso", "start-button", start.count() > 0, "no Start button for the new VM in the VM list")
                if start.count():
                    start.click()
                    page.wait_for_timeout(3000)
                j.shot("vm-list-starting")
                running = False
                for _i in range(60):
                    code, cur = api("GET", f"/api/vms/{vid}")
                    if code == 200 and str(cur.get("status", "")).lower() == "running":
                        running = True
                        break
                    time.sleep(1)
                expect("j4-boot-iso", "vm-running", running, "the VM did not reach 'running' after Start: " + j.text()[-200:].replace("\n", " "))
                if running:
                    cl = " ".join(qemu_cmdlines())
                    expect("j4-boot-iso", "qemu-has-iso", iso_name in cl, "QEMU's command line does not reference the chosen ISO")
                    expect("j4-boot-iso", "qemu-has-nic", not state["bridge"] or re.search(r"-(device|net)[ =]\S*(virtio-net|e1000|rtl8139|vmxnet3|nic)", cl) is not None or "netdev" in cl,
                           "QEMU was started without a network device")
                    if usable:
                        expect("j4-boot-iso", "qemu-has-sound", re.search(r"(AC97|intel-hda|ich9-intel-hda|hda-duplex|hda-output|usb-audio)", cl, re.I) is not None, "QEMU was started without the sound device")
                    # console: the guest screen shows up in the browser
                    j.go(f"/vms/{vid}/console", settle=6)
                    j.shot("console", full=False)
                    ok_canvas = False
                    for _i in range(15):
                        info = page.evaluate("""() => {const fs=[document,...[...document.querySelectorAll('iframe')].map(f=>{try{return f.contentDocument}catch(e){return null}}).filter(Boolean)];
                          for (const d of fs){const c=d.querySelector('canvas'); if(c && c.width>0){try{const x=c.getContext('2d').getImageData(0,0,c.width,c.height).data;let n=0;for(let i=0;i<x.length;i+=4*97){if(x[i]|x[i+1]|x[i+2])n++}return {w:c.width,lit:n}}catch(e){return {w:c.width,lit:-1}}}} return null}""")
                        if info and (info["lit"] > 0 or info["lit"] == -1):
                            ok_canvas = True
                            break
                        page.wait_for_timeout(2000)
                    expect("j4-boot-iso", "console-shows-guest", ok_canvas, "the VNC console never showed a guest screen (blank/absent canvas)")
                    j.shot("console-guest", full=False)
                    # the guest must run the ISO: its GRUB script powers the VM off after printing the marker
                    stopped = False
                    for _i in range(BOOT_TIMEOUT // 3):
                        code, cur = api("GET", f"/api/vms/{vid}")
                        if code == 200 and str(cur.get("status", "")).lower() == "stopped":
                            stopped = True
                            break
                        time.sleep(3)
                    expect("j4-boot-iso", "guest-ran-the-iso", stopped, f"the guest did not run the ISO (it never powered itself off within {BOOT_TIMEOUT}s): the VM is not booting the CD-ROM")
                    j.go("/vms")
                    j.shot("vm-list-after-boot")

        # ------------------------------------------------------------------ J5 container
        with step(j, "j5-container", "create a container") as _:
            j.go("/containers/create", settle=2)
            j.shot("container-create")
            body = j.text()
            has_form = page.locator("form input").count() > 0
            unavailable = re.search(r"not available|not installed|unavailable|no container runtime|enable", body, re.I)
            expect("j5-container", "form-or-explanation", has_form or unavailable, "container create page shows neither a form nor why containers are unavailable")
            if has_form and page.locator("#ct-image").count():
                page.fill("form input[placeholder*='name' i], #ct-name", CT_NAME) if page.locator("#ct-name, form input[placeholder*='name' i]").count() else None
                page.fill("#ct-image", "busybox:latest")
                j.shot("container-create-filled")
                btn = page.locator("button[type=submit]").first
                expect("j5-container", "submit-enabled", btn.is_enabled(), "the container form cannot be submitted")
            j.go("/containers")
            j.shot("container-list")
            expect("j5-container", "list-explains-empty", len(j.text()) > 120, "container list is blank")

        # ------------------------------------------------------------------ J6 storage / networks
        for sid, title, routes, add_re in (
                ("j6-storage", "storage pools", [("/storage", "storage"), ("/storage/content", "storage-content")], r"add|create|new"),
                ("j6-network", "networks", [("/network", "network"), ("/network/bridges", "bridges"), ("/network/dhcp", "dhcp")], r"add|create|new")):
            with step(j, sid, title) as _:
                for path, name in routes:
                    j.go(path, settle=1.8)
                    j.shot(name)
                    t = j.text()
                    expect(sid, f"{name}-not-blank", len(t) > 150, f"{path} is nearly blank")
                    expect(sid, f"{name}-no-raw-error", not re.search(r"\b(panicked|stack trace|Internal Server Error|NaN|undefined)\b", t), f"{path} shows a raw error/NaN")
                j.go(routes[0][0])
                expect(sid, "can-add", page.locator("button, a.btn").filter(has_text=re.compile(add_re, re.I)).count() > 0, f"{routes[0][0]} has no add/create control")

        # ------------------------------------------------------------------ J7 backups / snapshots
        with step(j, "j7-backup-snapshots", "snapshots and backups") as _:
            for path, name in (("/snapshots", "snapshots"), ("/backup/snapshots", "backup-snapshots"), ("/backup", "backup"), ("/backup/jobs", "backup-jobs")):
                j.go(path, settle=1.8)
                j.shot(name)
                t = j.text()
                expect("j7-backup-snapshots", f"{name}-not-blank", len(t) > 150, f"{path} is nearly blank")
            vm = vm_by_name(VM_NAME)
            if vm:
                j.go("/snapshots")
                create = page.locator("button, a.btn").filter(has_text=re.compile(r"create|take|new|add", re.I)).first
                expect("j7-backup-snapshots", "snapshot-control", create.count() > 0, "the Snapshots page has no way to create a snapshot")
                code, res = api("POST", f"/api/vms/{vm['id']}/snapshots", {"name": "journey-snap", "description": "journey"})
                expect("j7-backup-snapshots", "snapshot-api", code in (200, 201, 202), f"snapshot of the stopped VM failed: {code} {str(res)[:150]}")
                j.go("/snapshots")
                j.shot("snapshots-after")

        # ------------------------------------------------------------------ J8 settings / updates
        with step(j, "j8-settings", "settings, users, updates") as _:
            for path, name in (("/system/config", "config"), ("/auth/users", "users"), ("/system/certificates", "certificates"),
                               ("/system/packages", "updates"), ("/system/health", "health"), ("/tasks", "tasks")):
                j.go(path, settle=2)
                j.shot(name)
                t = j.text()
                expect("j8-settings", f"{name}-not-blank", len(t) > 150, f"{path} is nearly blank")
                if name == "updates":
                    expect("j8-settings", "updates-actionable", re.search(r"check|update|upgrade|refresh", t, re.I) is not None, "the updates page offers no way to check for / install updates")
            j.go("/")
            expect("j8-settings", "dashboard-mentions-updates", re.search(r"update", j.text(), re.I) is not None,
                   "nothing on the dashboard tells the admin whether updates are pending (an unpatched node looks identical to a patched one)", )
            j.shot("dashboard-end")

        # leave the install tidy
        vm = vm_by_name(VM_NAME)
        if vm:
            api("DELETE", f"/api/vms/{vm['id']}", ok=None)
        browser.close()
    srv.shutdown()
    shutil.rmtree(tmp, ignore_errors=True)

    base = {}
    if os.path.exists(a.baseline):
        base = {k: v for k, v in json.load(open(a.baseline)).items() if not k.startswith("_")}
    fails = sorted({f["id"] for f in findings if f["sev"] == "fail"})
    new = [f for f in fails if f not in base]
    fixed = [b for b in base if b not in fails]
    report = {"base": BASE, "steps": steps, "findings": findings, "new_failures": new, "baseline_hit": [f for f in fails if f in base], "baseline_fixed": fixed}
    if a.report:
        json.dump(report, open(a.report, "w"), indent=1)
    for f in fixed:
        print(f"  baseline entry no longer fails (remove it from {os.path.basename(a.baseline)}): {f}")
    print(f"user-journey: {len(steps)} steps, {len(fails)} failing checks ({len(fails) - len(new)} known/tracked), {len(new)} NEW")
    for f in new:
        print("  NEW:", f)
    sys.exit(1 if new else 0)


if __name__ == "__main__":
    main()
