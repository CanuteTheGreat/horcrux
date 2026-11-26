#!/usr/bin/env python3
"""Browser check of the storage pool form generated from a plugin's schema
(real Chromium against a running horcrux-api).

A throwaway plugin (Python, the protocol is plain JSON over stdio) is put in
its own root-owned directory and the node's plugin settings are pointed at
it. Then, in the UI: the plugin is listed under Storage plugins, its type is
offered in Create Pool, the form shows each field with its hint (tooltip and
text), the number input carries the declared min/max, the secret is a
password input, a depends_on field appears only when its condition holds, an
out-of-range number is refused (message under the field, no pool created),
and a valid form creates the pool. The node's settings are restored at the
end.
"""
import json
import os
import shutil
import ssl
import stat
import urllib.error
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL_CTX = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
PLUGIN_DIR = os.environ.get("HX_E2E_PLUGIN_DIR", "/tmp/hx-e2e-storage-plugins")
HINT = "Largest size of the pool, in GB."

PLUGIN = r'''#!/usr/bin/env python3
import json, os, sys
req = json.load(sys.stdin)
def out(v):
    print(json.dumps({"ok": True, "result": v})); sys.exit(0)
if req["op"] == "describe":
    out({"id": "formtest", "name": "Form Test", "version": "1.0", "api_version": 1,
         "description": "Pool form generated from a schema",
         "schema": [
            {"key": "server", "type": "string", "label": "Server", "required": True,
             "hint": "Host name of the storage server.", "placeholder": "storage.example.org"},
            {"key": "quota_gb", "type": "int", "label": "Quota", "default": 10,
             "min": 1, "max": 100, "hint": "''' + HINT + r'''"},
            {"key": "mode", "type": "enum", "label": "Mode", "default": "fast",
             "options": [{"value": "fast", "label": "Fast"}, {"value": "safe", "label": "Safe"}]},
            {"key": "sync_every", "type": "int", "label": "Sync every (s)", "default": 30,
             "min": 5, "max": 600, "depends_on": {"key": "mode", "value": "safe"}},
            {"key": "token", "type": "secret", "label": "Access token", "required": True,
             "hint": "Never shown again after saving."}],
         "capabilities": {"content": ["images"]}})
st = req["pool"]["state_dir"]
if req["op"] == "activate":
    os.makedirs(os.path.join(st, "d"), exist_ok=True); out({})
if req["op"] == "status":
    out({"active": True, "total_bytes": 10 * 2**30, "available_bytes": 2**30})
if req["op"] in ("deactivate",):
    out({})
if req["op"] == "list_volumes":
    out([])
print(json.dumps({"ok": False, "error": {"code": "unsupported", "message": req["op"]}}))
'''


def log(m):
    print(m, flush=True)


_T = []


def _tok():
    if not _T:
        r = urllib.request.Request(f"{BASE}/api/auth/login", method="POST",
                                   data=json.dumps({"username": "admin", "password": "admin"}).encode(),
                                   headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(r, context=SSL_CTX) as resp:
            j = json.load(resp)
            _T.append(j.get("ticket") or j.get("token"))
    return _T[0]


def api(method, path, body=None):
    r = urllib.request.Request(f"{BASE}/api{path}", method=method,
                               data=json.dumps(body).encode() if body is not None else None,
                               headers={"Authorization": f"Bearer {_tok()}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r, context=SSL_CTX) as resp:
            raw = resp.read()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        return e.code, e.read()[:500]


def main():
    shutil.rmtree(PLUGIN_DIR, ignore_errors=True)
    os.makedirs(PLUGIN_DIR)
    os.chmod(PLUGIN_DIR, 0o755)
    exe = os.path.join(PLUGIN_DIR, "formtest")
    with open(exe, "w") as f:
        f.write(PLUGIN)
    os.chmod(exe, stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)
    s, old = api("GET", "/storage/plugin-settings")
    assert s == 200, (s, old)
    new = dict(old, plugin_dir=PLUGIN_DIR, require_root_owned=os.geteuid() == 0)
    s, r = api("PUT", "/storage/plugin-settings", new)
    assert s == 200, (s, r)
    errors = []
    try:
        s, l = api("GET", "/storage/plugins")
        assert any(p["id"] == "formtest" and p["usable"] for p in l["plugins"]), l
        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page(ignore_https_errors=True)
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(f"{BASE}/login")
            page.locator("input[type=text], input:not([type])").first.fill("admin")
            page.locator("input[type=password]").fill("admin")
            page.locator("button[type=submit]").click()
            page.wait_for_url(f"{BASE}/")

            page.goto(f"{BASE}/storage")
            row = page.locator(".storage-plugin[data-plugin=formtest]")
            expect(row).to_contain_text("Form Test")
            expect(row).to_contain_text("ready")
            log("ok - the plugin is listed under Storage plugins")

            page.get_by_role("button", name="+ Create Pool").click()
            sel = page.locator(".modal .pool-type-select")
            expect(sel.locator("option[value='plugin:formtest']")).to_have_text("Form Test (plugin)")
            sel.select_option("plugin:formtest")
            form = page.locator(".plugin-config-form[data-plugin=formtest]")
            expect(form).to_be_visible()
            expect(page.locator(".modal input[placeholder='/var/lib/horcrux/images']")).to_have_count(0)
            log("ok - choosing the plugin type swaps the path input for the generated form")

            quota = form.locator("input[data-key=quota_gb]")
            expect(quota).to_have_attribute("type", "number")
            expect(quota).to_have_attribute("min", "1")
            expect(quota).to_have_attribute("max", "100")
            expect(quota).to_have_value("10")
            wrap = form.locator(".plugin-field-wrap[data-key=quota_gb]")
            expect(wrap.locator(".plugin-field-hint")).to_have_text(HINT)
            expect(wrap.locator(".field-hint-icon")).to_have_attribute("title", HINT)
            log("ok - the number field shows its hint (text + tooltip), default and min/max")
            expect(form.locator("input[data-key=token]")).to_have_attribute("type", "password")
            expect(form.locator("input[data-key=server]")).to_have_attribute("placeholder", "storage.example.org")
            log("ok - the secret is a password input; placeholders come from the schema")

            expect(form.locator("input[data-key=sync_every]")).to_have_count(0)
            form.locator("select[data-key=mode]").select_option("safe")
            expect(form.locator("input[data-key=sync_every]")).to_be_visible()
            form.locator("select[data-key=mode]").select_option("fast")
            expect(form.locator("input[data-key=sync_every]")).to_have_count(0)
            log("ok - a depends_on field appears only when its condition holds")

            page.locator(".modal .form-group", has_text="Name").locator("input").first.fill("e2e-plugin-pool")
            form.locator("input[data-key=server]").fill("storage.example.org")
            form.locator("input[data-key=token]").fill("t0ken")
            quota.fill("1000")
            page.locator(".modal button.btn-primary", has_text="Create").click()
            expect(wrap.locator(".plugin-field-error")).to_contain_text("at most 100")
            s, pools = api("GET", "/storage/pools")
            assert all(x["name"] != "e2e-plugin-pool" for x in pools), pools
            log("ok - an out-of-range number is refused under its field and nothing is created")

            quota.fill("50")
            expect(wrap.locator(".plugin-field-error")).to_have_count(0)
            page.locator(".modal button.btn-primary", has_text="Create").click()
            expect(page.locator(".modal")).to_have_count(0, timeout=30000)
            s, pools = api("GET", "/storage/pools")
            made = [x for x in pools if x["name"] == "e2e-plugin-pool"]
            assert made and made[0]["storage_type"] == "plugin:formtest", pools
            assert made[0]["plugin_config"]["quota_gb"] == 50, made
            assert "t0ken" not in json.dumps(pools), "secret returned by the API"
            log("ok - a valid form creates the plugin pool (number typed, secret kept out of the API)")
            browser.close()
            for x in made:
                api("DELETE", f"/storage/pools/{x['id']}")
    finally:
        api("PUT", "/storage/plugin-settings", old)
        shutil.rmtree(PLUGIN_DIR, ignore_errors=True)
    assert not errors, f"uncaught page errors: {errors}"
    log("ok - storage plugin form e2e passed")


if __name__ == "__main__":
    main()
