#!/usr/bin/env python3
"""Declarative page (/declarative) in a real browser against a real server: desired spec, drift, history + rollback, GitOps.

Class of check (owner UX rules): the page loads with zero uncaught JS errors; empty states carry the obvious next action; tabs
and rows are real links; every route has its own <title>; no horizontal page scroll at a 390 px phone width; a zero count is
never red; plaintext secrets are refused in the GitOps form with the reason on the page.

Flow (all through the UI): empty Desired tab -> start from the node's current state -> add a VM -> Validate -> Save -> Plan & drift lists
the VM as missing -> Preview apply -> Apply -> in sync -> History has the record -> Roll back preview -> GitOps: a plaintext token is
refused, a real file:// repository (made here with git) is saved, Sync now queues a pending approval, Approve applies it.
"""
import json
import os
import ssl
import subprocess
import sys
import tempfile
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
PW = os.environ.get("ADMIN_PASSWORD", "admin")
VM = "uidecl-vm"
GVM = "uidecl-git"


def git(cwd, *args):
    env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")
    subprocess.check_call(["git", "-c", "user.name=CI", "-c", "user.email=ci@example.test", *args], cwd=cwd, env=env)


def cleanup():
    """Leave the server as we found it: no stored spec, no GitOps config, none of our VMs."""
    ctx = ssl.create_default_context()
    ctx.check_hostname, ctx.verify_mode = False, ssl.CERT_NONE

    def call(method, path, body=None, token=None):
        h = {"Content-Type": "application/json"}
        if token:
            h["Authorization"] = f"Bearer {token}"
        req = urllib.request.Request(f"{BASE}/api{path}", method=method, headers=h,
                                     data=json.dumps(body).encode() if body is not None else None)
        try:
            with urllib.request.urlopen(req, timeout=60, context=ctx) as r:
                return json.loads(r.read() or b"null")
        except Exception as e:  # noqa: BLE001 - best effort
            print(f"cleanup {method} {path}: {e}")
            return None

    login = call("POST", "/auth/login", {"username": "admin", "password": PW}) or {}
    tok = login.get("ticket") or login.get("token")
    call("DELETE", "/declarative/desired", token=tok)
    call("DELETE", "/declarative/gitops/config", token=tok)
    for vm in (VM, GVM):
        call("DELETE", f"/vms/{vm}", token=tok)


def main():
    errors = []
    repo = tempfile.mkdtemp(prefix="uidecl-repo-")
    git(repo, "init", "-q", "-b", "main")
    with open(os.path.join(repo, "horcrux.yaml"), "w") as f:
        f.write(f"version: 1\nvms:\n  - {{name: {GVM}, memory: 64, cpus: 1, power: stopped}}\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "add the git VM")

    with sync_playwright() as p:
        browser = (getattr(p, os.environ.get("BROWSER", "firefox"))).launch()
        ctx = browser.new_context(ignore_https_errors=True, viewport={"width": 1280, "height": 900})
        page = ctx.new_page()
        page.on("pageerror", lambda e: errors.append(f"pageerror: {e}"))
        page.on("console", lambda m: errors.append(f"console.error: {m.text}") if m.type == "error" and "401" not in m.text and "favicon" not in m.text else None)
        page.on("dialog", lambda d: d.accept("e2e reason"))

        page.goto(f"{BASE}/login")
        page.locator("#login-username").fill("admin")
        page.locator("input[type=password]").fill(PW)
        page.locator("#login-submit").click()
        page.wait_for_url(f"{BASE}/")

        # dashboard card links to the drift tab, and is not red while there is nothing to report
        card = page.locator("#stat-config-drift")
        expect(card).to_be_visible(timeout=30000)
        assert card.get_attribute("href") == "/declarative?tab=drift", card.get_attribute("href")
        expect(card).to_contain_text("No spec")
        assert "alert-count" not in (card.locator(".stat-value").get_attribute("class") or ""), "a zero/none count must not be red"

        # --- Desired spec tab: empty state with the obvious next action -------------------------------------
        page.goto(f"{BASE}/declarative")
        expect(page).to_have_title("Declarative Config · Horcrux")
        expect(page.locator("#decl-desired-empty")).to_be_visible(timeout=30000)
        for tab in ("desired", "drift", "gitops", "history"):
            href = page.locator(f"#decl-tab-{tab}").get_attribute("href")
            assert href == f"/declarative?tab={tab}", href
        page.locator("#decl-from-live").click()
        expect(page.locator("#decl-spec")).not_to_have_value("", timeout=30000)
        spec = page.locator("#decl-spec").input_value()
        assert "version: 1" in spec, spec[:200]
        add = f"\n  - {{name: {VM}, memory: 64, cpus: 1, power: stopped}}\n"
        if "vms: []" in spec:
            spec = spec.replace("vms: []", "vms:" + add.rstrip("\n"))
        else:
            spec = spec.rstrip("\n") + add
        page.locator("#decl-spec").fill(spec)
        page.locator("#decl-validate").click()
        res = page.locator("#decl-validate-result")
        expect(res).to_contain_text(VM, timeout=30000)
        page.locator("#decl-save").click()
        expect(page.locator(".decl-notice")).to_contain_text("Saved", timeout=30000)
        expect(page.locator("#decl-desired-meta")).to_contain_text("via ui")

        # a broken spec: the editor says why, nothing is stored
        page.locator("#decl-spec").fill("version: 9\n")
        page.locator("#decl-validate").click()
        expect(page.locator("#decl-validate-result")).to_contain_text("not supported", timeout=30000)
        page.reload()
        expect(page.locator("#decl-spec")).to_have_value(spec, timeout=30000)

        # --- Plan & drift -------------------------------------------------------------------------------------
        page.locator("#decl-tab-drift").click()
        expect(page.locator("#decl-drift-summary")).to_contain_text("difference", timeout=30000)
        row = page.locator("tr.drift-row", has_text=VM)
        expect(row).to_be_visible()
        expect(row).to_contain_text("present")
        expect(row).to_contain_text("missing")
        assert row.locator("a").first.get_attribute("href") is not None
        assert page.locator("tr.drift-row a[href='/vms/" + VM + "']").count() == 1
        expect(page.locator("#decl-diff")).to_be_visible()
        page.locator("#decl-preview").click()
        expect(page.locator("#decl-apply-result")).to_contain_text("Preview", timeout=30000)
        page.locator("#decl-apply").click()
        expect(page.locator("#decl-apply-result")).to_contain_text("Applied", timeout=60000)
        page.locator("#decl-check").click()
        expect(page.locator("#decl-drift-summary")).to_contain_text("In sync", timeout=30000)
        page.goto(f"{BASE}/")
        expect(page.locator("#stat-config-drift")).to_contain_text("In sync", timeout=30000)

        # --- History + rollback preview ---------------------------------------------------------------------
        page.goto(f"{BASE}/declarative?tab=history")
        hrow = page.locator("tr.history-row").first
        expect(hrow).to_be_visible(timeout=30000)
        expect(hrow).to_contain_text("ui")
        link = hrow.locator("a").first
        assert "tab=history&id=" in link.get_attribute("href")
        link.click()
        expect(page.locator("#decl-history-detail")).to_contain_text(VM, timeout=30000)
        page.locator("#decl-rb-preview").click()
        # the only record has nothing older to go back to: the page shows the server's reason
        expect(page.locator(".decl-error")).to_contain_text("no earlier apply", timeout=30000)

        # --- GitOps ---------------------------------------------------------------------------------------------
        page.goto(f"{BASE}/declarative?tab=gitops")
        expect(page.locator("#decl-git-empty")).to_be_visible(timeout=30000)
        page.locator("#git-url").fill(f"https://git.example.com/x.git")
        page.locator("details summary").click()
        page.locator("#git-cred").fill("hunter2")
        page.locator("#git-save").click()
        expect(page.locator(".decl-error")).to_contain_text("references", timeout=30000)
        page.locator("#git-cred").fill("")
        page.locator("#git-url").fill(f"file://{repo}")
        page.locator("#git-interval").fill("3600")
        page.locator("#git-save").click()
        expect(page.locator(".decl-notice")).to_contain_text("saved", timeout=30000)
        page.locator("#decl-sync").click()
        expect(page.locator("#decl-pending-empty")).to_have_count(0, timeout=30000)
        pend = page.locator(".pending-card")
        expect(pend).to_have_count(1, timeout=30000)
        expect(pend).to_contain_text("add the git VM")
        expect(pend).to_contain_text(GVM)
        pend.locator(".decl-approve").click()
        expect(page.locator("#decl-approve-result")).to_contain_text("Applied", timeout=60000)
        expect(page.locator("#decl-git-commit")).to_contain_text("add the git VM")
        expect(page.locator("#decl-pending-empty")).to_be_visible(timeout=30000)

        # --- phone width, light and dark: no sideways scroll, no uncaught errors ---------------------------------
        for scheme in ("light", "dark"):
            ph = browser.new_context(ignore_https_errors=True, viewport={"width": 390, "height": 800}, color_scheme=scheme)
            pp = ph.new_page()
            pp.on("pageerror", lambda e: errors.append(f"phone pageerror: {e}"))
            pp.goto(f"{BASE}/login")
            pp.locator("#login-username").fill("admin")
            pp.locator("input[type=password]").fill(PW)
            pp.locator("#login-submit").click()
            pp.wait_for_url(f"{BASE}/")
            for tab in ("desired", "drift", "gitops", "history"):
                pp.goto(f"{BASE}/declarative?tab={tab}")
                pp.wait_for_selector(f"#decl-{tab}", timeout=30000)
                pp.wait_for_timeout(800)
                sw = pp.evaluate("document.documentElement.scrollWidth")
                cw = pp.evaluate("document.documentElement.clientWidth")
                assert sw <= cw + 1, f"{scheme} phone {tab}: page scrolls sideways ({sw} > {cw})"
            ph.close()

        browser.close()
    cleanup()
    if errors:
        print("\n".join(errors))
        sys.exit(1)
    print("declarative page e2e: ok")


if __name__ == "__main__":
    main()
