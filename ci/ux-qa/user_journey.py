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
# 424 = the node lacks a tool/runtime (structured NodeLacks answer the UI turns into an empty state, as ux_crawl.py
# treats it): not a page error, but reported as a warning so the review sees what the test host was missing.
_unavailable = []


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


def server_argv(cmd, prefix=None):
    """argv that runs `sh -c cmd` where the server runs. ssh joins its remote arguments with spaces and the remote
    shell splits them again, so behind an ssh prefix the command is quoted once (unquoted, `sh -c` got only the first
    word and the rest ran on the ssh host: the review lane read no QEMU command line and made no bridge)."""
    prefix = PREFIX if prefix is None else prefix
    argv = ["sh", "-c", cmd]
    if prefix and os.path.basename(prefix[0]) == "ssh":
        return prefix + [shlex.quote(a) for a in argv]
    return prefix + argv


def run_on_server(cmd, stdin_path=None, timeout=60):
    """Run a shell command where the server runs (HX_EXEC_PREFIX) or locally."""
    stdin = open(stdin_path, "rb") if stdin_path else None
    try:
        return subprocess.run(server_argv(cmd), stdin=stdin, capture_output=True, text=True, timeout=timeout)
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
            _unavailable.clear()
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
            na = list(dict.fromkeys(_unavailable))
            if na:
                finding(sid, "node-lacks", "424 (feature unavailable on this node): " + "; ".join(na)[:340], sev="warn")
            steps.append({"id": sid, "title": title, "ok": not any(f["sev"] == "fail" for f in findings[self_.f0:]), "console_errors": errs})
            return True   # a failing step never stops the journey
    return Ctx()


def select_options(page, sel):
    return page.eval_on_selector(sel, "e => [...e.options].map(o => ({v:o.value,t:o.textContent.trim(),d:o.disabled}))") if page.locator(sel).count() else []


def vm_by_name(name):
    try:
        code, vms = api("GET", "/api/vms")
    except Exception as e:  # noqa: BLE001  (called between steps: a failed login must not end the run without a report)
        print(f"  vm_by_name: {type(e).__name__}: {e}", flush=True)
        return None
    return next((v for v in (vms or []) if v.get("name") == name), None) if code == 200 else None


# An empty state that points at a form "above" AND repeats it with its own button (owner finding 18 class).
REDUNDANT_EMPTY_JS = """() => [...document.querySelectorAll('.no-data, .empty-state')]
  .filter(e => e.offsetParent !== null && /\\babove\\b/i.test(e.innerText) && e.querySelector('button'))
  .map(e => e.innerText.trim().slice(0, 120))"""
# Tab strips that render as plain run-together buttons (no CSS for the page's tab classes).
UNSTYLED_TABS_JS = """() => [...document.querySelectorAll('.tab-buttons, .tabs, .dashboard-tabs')]
  .filter(e => e.offsetParent !== null && getComputedStyle(e).display !== 'flex').map(e => e.className)"""
# The page scrolls sideways (owner rule: never).
H_OVERFLOW_JS = "() => document.documentElement.scrollWidth - document.documentElement.clientWidth"
# Bridges a container runtime made for itself (never a VM network).
CONTAINER_BRIDGE_RE = re.compile(r"^(docker|podman|cni|kube-bridge$|br-[0-9a-f]{12}$)")
RT_BRIDGE = "br-fa11bac0ffee"   # Docker-network-style bridge the journey makes (when it can) to test that filter


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
        page.on("response", lambda r: (_unavailable if r.status == 424 else _console).append(f"http {r.status} {r.request.method} {r.url.replace(BASE, '')[:90]}")
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
            # a host running Docker has docker0 / br-<hash>: make a Docker-style network bridge where we can, so the
            # VM network list is really checked for leaving it out (removed at the end)
            try:
                state["rt_bridge"] = run_on_server(f"ip link show docker0 >/dev/null 2>&1 && exit 0; ip link add {RT_BRIDGE} type bridge "
                                                   f"&& ip link set {RT_BRIDGE} up && echo made", timeout=20).stdout.strip() == "made"
            except Exception:  # noqa: BLE001  (no ip tool / no permission: the check then reads the real bridges only)
                state["rt_bridge"] = False
            code, opts = api("GET", "/api/vms/host-options")
            if code == 200 and not (opts or {}).get("bridges"):
                j.go("/vms/create", settle=2.5)
                j.shot("create-vm-no-bridge")
                finding("j3-create-vm", "fresh-host-no-bridge", "a fresh install has no network bridge, so a VM cannot get a network card; "
                        "the create form should offer to create one (or default to NAT/user networking) instead of failing at Create")
                run_on_server("ip link add vmbr0 type bridge && ip link set vmbr0 up", timeout=20)   # the rest of the journey needs one
                code, opts = api("GET", "/api/vms/host-options")
                state["bridge"] = bool((opts or {}).get("bridges"))   # an unprivileged runner cannot make one: the VM is then created without a NIC
                if not state["bridge"]:
                    finding("j3-create-vm", "nic-path-untested", "no bridge could be created on this test host (needs iproute2 + CAP_NET_ADMIN), so the "
                            "VM was created WITHOUT a NIC and qemu-has-nic/saved-nic were not really checked", sev="warn")
            j.go("/vms/create", settle=2.5)
            j.shot("create-vm-top")
            n_range = page.locator("form input[type=range]").count()
            expect("j3-create-vm", "resource-sliders", n_range >= 3,
                   f"CPU/RAM/disk are plain number boxes ({n_range} sliders): no sliders, supported steps or presets with a custom override")
            expect("j3-create-vm", "nic-by-default", page.locator(".vm-nic-row").count() >= 1,
                   "a new VM form has no network card by default")
            # container runtimes' own bridges (docker0, podman0, cni0, br-<hash>) are never offered to a VM
            nic_opts = [o["v"] for o in select_options(page, ".vm-nic-bridge")] if page.locator(".vm-nic-bridge").count() else []
            api_br = [b.get("name", "") for b in ((opts or {}).get("bridges") or []) if isinstance(b, dict)] if code == 200 and isinstance(opts, dict) else []
            bad_br = sorted({b for b in nic_opts + api_br if CONTAINER_BRIDGE_RE.match(b)})
            expect("j3-create-vm", "nic-no-container-bridges", not bad_br,
                   f"the VM network list offers container runtime bridges {bad_br} (Docker/Podman own those; a VM NIC there gets no DHCP)")
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
                    # a stopped VM's row offers Start, not Shutdown/Reboot/Stop; the row fits the page
                    row = page.locator(f"tr[data-vm-id='{vid}']").first
                    if stopped and row.count():
                        live = {c: row.locator(f"button.{c}").first for c in ("vm-start", "vm-shutdown", "vm-reboot", "vm-stop")}
                        bad = [c for c in ("vm-shutdown", "vm-reboot", "vm-stop") if live[c].count() and live[c].is_enabled()]
                        expect("j4-boot-iso", "stopped-vm-power-buttons", not bad and (live["vm-start"].count() == 0 or live["vm-start"].is_enabled()),
                               f"the stopped VM's row offers {bad or 'no Start'}: power buttons do not follow the VM state")
                    over = page.evaluate(H_OVERFLOW_JS)
                    expect("j4-boot-iso", "vm-list-fits", over <= 2, f"the VM list with one VM scrolls sideways by {over}px at 1366 wide")

        # ------------------------------------------------------------------ J5 container
        with step(j, "j5-container", "create a container") as _:
            j.go("/containers/create", settle=2)
            page.wait_for_selector("#ct-runtime, #ct-no-runtime, #ct-runtimes-error", timeout=30000)
            j.shot("container-create")
            body = j.text()
            has_form = page.locator("#create-container-form").count() > 0 and page.locator("#create-container-form").is_visible()
            unavailable = re.search(r"not available|not installed|unavailable|no container runtime|enable", body, re.I)
            expect("j5-container", "form-or-explanation", has_form or unavailable, "container create page shows neither a form nor why containers are unavailable")
            # the form offers exactly the runtimes that work on this node (live probe), never one that fails at Create
            code, rts = api("GET", "/api/containers/runtimes")
            works = [r["runtime"] for r in (rts or {}).get("runtimes", []) if r.get("available")] if code == 200 and isinstance(rts, dict) else None
            offered = [o["v"] for o in select_options(page, "#ct-runtime")]
            expect("j5-container", "runtime-api", works is not None, f"GET /api/containers/runtimes answered {code}")
            if works is not None:
                expect("j5-container", "only-working-runtimes", offered == works,
                       f"the Runtime list offers {offered} but the runtimes that work on this node are {works}")
                if not works:
                    nr = page.locator("#ct-no-runtime")
                    expect("j5-container", "no-runtime-explained", nr.count() > 0 and nr.is_visible() and not has_form
                           and page.locator("#ct-no-runtime .ct-runtime-hint").count() > 0
                           and page.locator("#ct-runtime-recheck").count() > 0,
                           "no container runtime works here, but the page does not say so up front with how to enable one")
            if has_form and page.locator("#ct-image").count():
                n_range = page.locator("#create-container-form input[type=range]").count()
                expect("j5-container", "resource-sliders", n_range >= 2,
                       f"container CPU/RAM are plain number boxes ({n_range} sliders), unlike the Create VM form")
                page.fill("#ct-name", CT_NAME)
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
                    red = page.evaluate(REDUNDANT_EMPTY_JS)
                    expect(sid, f"{name}-empty-state-no-redundant-button", not red, f"{path}: empty state points at the form above and repeats it with a button: {red}")
                    if path == "/storage" and page.locator(".storage-card").count():
                        styled = page.evaluate("""() => ['.storage-overview', '.storage-grid', '.usage-labels']
                            .map(s => document.querySelector(s)).every(e => e && ['grid', 'flex'].includes(getComputedStyle(e).display))""")
                        expect(sid, "storage-styled", styled, "Storage Pools page is unstyled: totals and the pool card render as stacked run-together text")
                        # the volumes panel stays inside its card (it once spilled over the next card, which took the clicks)
                        tog = page.locator(".storage-card .pool-volumes-toggle").first
                        if tog.count():
                            tog.click()
                            page.wait_for_timeout(1500)
                            spill = page.evaluate("""() => { const p = document.querySelector('.storage-card .pool-volumes'); if (!p) return 0;
                                // content wider than the panel that is not scrolled inside it spills over the neighbour card
                                const over = getComputedStyle(p).overflowX === 'visible' ? p.scrollWidth - p.clientWidth : 0;
                                // and every control in the panel takes its own clicks
                                const blocked = [...p.querySelectorAll('button, input')].filter(e => e.offsetParent !== null).filter(e => {
                                    const r = e.getBoundingClientRect(); const x = r.left + r.width / 2, y = r.top + r.height / 2;
                                    if (x < 0 || x > innerWidth || y < 0 || y > innerHeight) return false;
                                    const hit = document.elementFromPoint(x, y); return hit && !e.contains(hit) && !p.contains(hit); }).length;
                                return over + (blocked ? 1000 : 0) }""")
                            expect(sid, "pool-volumes-inside-card", spill <= 2, f"the open volumes panel sticks out of its pool card by {spill:.0f}px")
                            j.shot("storage-volumes-open")
                            tog.click()
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
                create = page.locator("#snapshot-create-btn")
                expect("j7-backup-snapshots", "snapshot-control", create.count() > 0 and create.is_enabled(),
                       "the Snapshots page has no (enabled) way to create a snapshot although a VM exists")
                taken = False
                if create.count() and create.is_enabled():
                    # take it the way a user does: the dialog (VM preselected), name, Create
                    create.click()
                    page.wait_for_selector("#create-snapshot-form", timeout=10000)
                    vm_opts = [o["v"] for o in select_options(page, "#snap-vm")]
                    if vm["id"] in vm_opts:
                        page.select_option("#snap-vm", vm["id"])
                    page.fill("#snap-name", "journey-snap")
                    page.fill("#snap-desc", "journey")
                    page.click("#snap-submit")
                    try:
                        page.wait_for_selector("#snapshot-notice", timeout=60000)
                        taken = True
                    except Exception:  # noqa: BLE001
                        err = page.locator("#create-snapshot-error")
                        finding("j7-backup-snapshots", "snapshot-ui", "creating a snapshot from the dialog failed: "
                                + (err.inner_text()[:200] if err.count() else "no message"))
                if not taken:
                    code, res = api("POST", f"/api/vms/{vm['id']}/snapshots", {"name": "journey-snap", "description": "journey"})
                    expect("j7-backup-snapshots", "snapshot-api", code in (200, 201, 202), f"snapshot of the stopped VM failed: {code} {str(res)[:150]}")
                page.wait_for_timeout(1500)
                j.shot("snapshots-after")
                row = page.locator("tr[data-snapshot-name='journey-snap']")
                expect("j7-backup-snapshots", "new-snapshot-visible", row.count() > 0 and row.first.is_visible(),
                       "the snapshot just taken is not listed on the Snapshots page (no VM picked = nothing shown)")
                # a fresh visit (no VM picked) lists it too
                j.go("/snapshots", settle=2.5)
                expect("j7-backup-snapshots", "snapshots-default-all-vms", page.locator("tr[data-snapshot-name='journey-snap']").count() > 0,
                       "opening /snapshots shows no snapshot until a VM is picked")

        # ------------------------------------------------------------------ J8 settings / updates
        with step(j, "j8-settings", "settings, users, updates") as _:
            for path, name in (("/system/config", "config"), ("/auth/users", "users"), ("/system/certificates", "certificates"),
                               ("/system/packages", "updates"), ("/system/health", "health"), ("/tasks", "tasks")):
                j.go(path, settle=2)
                j.shot(name)
                t = j.text()
                expect("j8-settings", f"{name}-not-blank", len(t) > 150, f"{path} is nearly blank")
                bad_tabs = page.evaluate(UNSTYLED_TABS_JS)
                expect("j8-settings", f"{name}-tabs-styled", not bad_tabs, f"{path}: tab strip is unstyled ({bad_tabs})")
                if name == "config":
                    # time zone / locale come from the node and show its current values (Etc/UTC was blank)
                    code, cfg = api("GET", "/api/system/config")
                    if code == 200 and isinstance(cfg, dict) and page.locator("#cfg-timezone").count():
                        page.wait_for_timeout(1000)
                        tz_n = page.eval_on_selector("#cfg-timezone", "e => e.options.length")
                        tz_v = page.eval_on_selector("#cfg-timezone", "e => e.value")
                        lc_v = page.eval_on_selector("#cfg-locale", "e => e.value") if page.locator("#cfg-locale").count() else ""
                        expect("j8-settings", "timezone-shows-current", tz_v == cfg.get("timezone"),
                               f"Timezone select shows {tz_v!r}, the node is in {cfg.get('timezone')!r}")
                        expect("j8-settings", "timezone-is-a-real-zone", bool(tz_v) and not tz_v.startswith("/")
                               and run_on_server(f"test -f /usr/share/zoneinfo/{tz_v} && echo yes").stdout.strip() == "yes",
                               f"the node's time zone is reported as {tz_v!r}, which is not a zone in its zoneinfo database")
                        expect("j8-settings", "locale-shows-current", lc_v == cfg.get("locale"),
                               f"Locale select shows {lc_v!r}, the node's locale is {cfg.get('locale')!r}")
                        expect("j8-settings", "timezone-list-from-node", tz_n > 50 or not run_on_server("ls /usr/share/zoneinfo/America 2>/dev/null | head -1").stdout.strip(),
                               f"the Timezone list has {tz_n} entries although the node has the zoneinfo database")
                        expect("j8-settings", "timezone-searchable", page.locator("#cfg-timezone-search").count() > 0,
                               "the Timezone list (hundreds of zones) has no search box")
                    else:
                        finding("j8-settings", "timezone-select", "System Configuration has no #cfg-timezone select")
                if name == "tasks":
                    # the empty state promises that starting/stopping a guest records a task; the journey started and stopped a VM
                    expect("j8-settings", "tasks-record-vm-power", re.search(r"vm-start|vm-stop", t) is not None,
                           "the Tasks page lists nothing for the VM that was just started and stopped (its empty state says that records a task)")
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
        if state.get("rt_bridge"):
            run_on_server(f"ip link del {RT_BRIDGE}", timeout=20)
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
