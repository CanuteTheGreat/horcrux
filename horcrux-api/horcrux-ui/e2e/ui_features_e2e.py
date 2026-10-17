#!/usr/bin/env python3
"""Browser check that the web UI follows the server's USE flags.

The UI is one WASM bundle for every Gentoo USE set; it reads the server's
compiled-in features from GET /api/features and hides the menu entries,
pages and form options of the others (horcrux-ui/src/features.rs).

Runs in CI's "UI E2E" job twice:
  MODE=full    against the --all-features server: everything is offered.
  MODE=reduced against a server built --no-default-features --features lxc
               (what USE="lxc -docker -podman -qemu -backup -cluster -nas
               -kubernetes" compiles): only LXC is offered, every other
               page says it is not compiled in, and the dashboard does not
               call the (404) cluster API.
"""
import json
import os
import sys
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "http://127.0.0.1:8006")
MODE = os.environ.get("MODE", "full")

GATED_NAV = {
    "Images": "docker|podman",
    "Volumes": "docker|podman",
    "Container Networks": "docker|podman",
    "Stacks (Compose)": "docker|podman",
    "Container Events": "docker|podman",
    "Kubernetes": "kubernetes",
    "High Availability": "cluster",
    "Backup & Protection": "backup",
    "NAS Overview": "nas",
}
GATED_PAGES = {
    "/backup": "backup",
    "/backup/jobs": "backup",
    "/ha": "cluster",
    "/ha/groups": "cluster",
    "/containers/images": "docker|podman",
    "/containers/stacks": "docker|podman",
    "/kubernetes": "kubernetes",
    "/nas": "nas",
    "/nas/shares": "nas",
}
ALWAYS_NAV = ["Virtual Machines", "Containers", "Storage", "Snapshots"]


def server_features():
    with urllib.request.urlopen(f"{BASE}/api/features") as r:
        return set(json.load(r)["features"])


def has(feats, expr):
    return any(f in feats for f in expr.split("|"))


def main():
    feats = server_features()
    print(f"MODE={MODE} server features: {sorted(feats)}")
    if MODE == "reduced":
        assert feats == {"lxc"}, f"reduced server should only have lxc: {feats}"
    else:
        for f in ("docker", "podman", "lxc", "backup", "cluster", "nas", "kubernetes"):
            assert f in feats, f"full server lacks {f}: {feats}"

    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"{BASE}/login")
        page.locator("input[type=text], input:not([type])").first.fill("admin")
        page.locator("input[type=password]").fill("admin")
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/")
        expect(page.locator("nav.navbar[data-features-loaded='true']")).to_have_count(1, timeout=30000)

        # ---- menu --------------------------------------------------------
        for text in ALWAYS_NAV:
            expect(page.locator("nav a", has_text=text).first).to_be_attached()
        for text, expr in GATED_NAV.items():
            want = 1 if has(feats, expr) else 0
            got = page.locator("nav a").filter(has_text=text).count()
            assert (got > 0) == bool(want), f"[{MODE}] menu entry {text!r} ({expr}): found {got}"

        # ---- dashboard: no cluster panel / no 404 error without cluster --
        expect(page.locator("#stat-cluster-nodes")).to_have_count(1 if "cluster" in feats else 0)
        page.wait_for_timeout(3000)
        expect(page.get_by_text("Could not reach the backend")).to_have_count(0)

        # ---- pages reached by URL ----------------------------------------
        for path, expr in GATED_PAGES.items():
            page.goto(f"{BASE}{path}")
            disabled = page.locator("#feature-disabled")
            if has(feats, expr):
                page.wait_for_timeout(1500)
                assert disabled.count() == 0, f"[{MODE}] {path} says not compiled in"
            else:
                expect(disabled).to_be_visible(timeout=20000)
                expect(disabled).to_contain_text("Not available in this build")

        # ---- form options follow the compiled runtimes/hypervisors --------
        page.goto(f"{BASE}/containers/create")
        opts = page.locator("#ct-runtime option")
        expect(opts.first).to_be_attached(timeout=20000)
        runtimes = opts.evaluate_all("els => els.map(e => e.value)")
        want = [r for r in ("docker", "podman", "lxc", "lxd", "incus") if r in feats]
        assert runtimes == want, f"[{MODE}] runtime options {runtimes}, expected {want}"
        assert page.locator("#ct-runtime").input_value() == want[0]

        page.goto(f"{BASE}/vms/create")
        hv_want = [h for h in ("qemu", "lxd", "incus") if h in feats]
        if hv_want:
            expect(page.locator("#vm-hypervisor option").first).to_be_attached(timeout=20000)
            hvs = page.locator("#vm-hypervisor option").evaluate_all("els => els.map(e => e.value)")
            assert hvs == hv_want, f"[{MODE}] hypervisor options {hvs}, expected {hv_want}"
        else:
            expect(page.locator("#vm-no-hypervisor")).to_be_visible(timeout=20000)
            assert page.locator("#vm-hypervisor option").count() == 0

        browser.close()
    print(f"UI feature gating ({MODE}): all checks passed")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as e:
        print(f"FAIL: {e}")
        sys.exit(1)
