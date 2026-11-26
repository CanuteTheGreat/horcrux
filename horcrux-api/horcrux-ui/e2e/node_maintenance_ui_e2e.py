#!/usr/bin/env python3
"""Cluster > Nodes: maintenance badge and Enter / Exit maintenance of another member (Firefox).

The cluster is mocked in the browser (Playwright routes; the real lone server answers everything else): three members,
nb holds two guests. The flow an admin follows:
  1. the Nodes list shows no badge, nb's row offers "Enter maintenance...";
  2. the panel shows the plan (2 guests move, counts are links to the table) and Enter sends
     POST /api/nodes/nb/maintenance {"enabled": true, "drain": true, "force": false} (no return key: the node's/cluster's);
  3. the drain progress appears, the badge turns to "maintenance (2 moved away)" and links to /ha/maintenance?node=nb;
  4. Exit with return policy "origin" sends {"enabled": false, ..., "return": "origin"}; the badge disappears;
  5. /ha/maintenance?node=nb opens the same panel for nb from the node picker.
"""
import json
import os
import re
import sys

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
NODES = ["na", "nb", "nc"]


def cluster_nodes():
    return [{"id": i, "name": n, "ip": f"192.0.2.{10 + i}", "status": "online", "priority": 100, "is_local": n == "na",
             "architecture": "x86_64", "cpu_cores": 4, "memory_total": 8 << 30, "cpu_usage": 5.0, "memory_used": 2 << 30}
            for i, n in enumerate(NODES)]


PLAN = {"moves": [{"guest_id": "g1", "name": "web", "from": "nb", "to": "na", "reason": "drain"},
                  {"guest_id": "g2", "name": "db", "from": "nb", "to": "nc", "reason": "drain"}],
        "unplaced": []}
RETURN_PLAN = {"moves": [dict(m, **{"from": m["to"], "to": "nb", "reason": "return"}) for m in PLAN["moves"]], "unplaced": []}


class Cluster:
    def __init__(self):
        self.enabled = False
        self.posts = []

    def view(self, node):
        on = node == "nb" and self.enabled
        origins = [{"guest_id": m["guest_id"], "name": m["name"], "origin": "nb", "moved_to": m["to"], "at": "2026-11-25T10:00:00Z"}
                   for m in PLAN["moves"]] if on else []
        drain = {"kind": "drain", "started_at": "2026-11-25T10:00:00Z", "finished_at": "2026-11-25T10:01:00Z", "status": "done",
                 "plan": PLAN, "results": [dict(m, status="done", progress=100, job_id=None, error=None)
                                           for m in PLAN["moves"]]} if on else None
        return {"enabled": on, "since": "2026-11-25T10:00:00Z" if on else None, "reason": "maintenance" if on else None,
                "drain": drain, "exit": None, "origins": origins, "return_policy": None, "clustered": True, "node": node,
                "effective_return_policy": "origin"}

    def all(self):
        return {"settings": {"return_policy": "origin"},
                "nodes": [{"node": n, "online": True, "local": n == "na", "maintenance": self.view(n), "error": None} for n in NODES]}


def fulfil(route, body, status=200):
    route.fulfill(status=status, content_type="application/json", body=json.dumps(body))


def main():
    c = Cluster()
    fails = []

    def check(name, ok, detail=""):
        print(f"{'ok' if ok else 'FAIL'} - {name} {detail}", flush=True)
        if not ok:
            fails.append(name)

    with sync_playwright() as p:
        browser = p.firefox.launch()
        page = browser.new_page(ignore_https_errors=True, viewport={"width": 1440, "height": 1000})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.route("**/api/cluster/nodes", lambda r: fulfil(r, cluster_nodes()) if r.request.method == "GET" else r.continue_())
        page.route("**/api/cluster/maintenance", lambda r: fulfil(r, c.all()))

        def node_route(route):
            url = route.request.url.split("/api/nodes/", 1)[1]
            node = url.split("/", 1)[0]
            if "/plan" in url:
                return fulfil(route, RETURN_PLAN if "action=exit" in url else PLAN)
            if route.request.method == "POST":
                body = json.loads(route.request.post_data or "{}")
                c.posts.append((node, body))
                c.enabled = bool(body.get("enabled"))
            return fulfil(route, c.view(node))
        # a regex: Playwright globs have no character classes
        page.route(re.compile(r".*/api/nodes/n[abc]/maintenance.*"), node_route)

        page.goto(f"{BASE}/login")
        page.locator("input[type=text], input:not([type])").first.fill("admin")
        page.locator("input[type=password]").fill(os.environ.get("ADMIN_PASSWORD", "admin"))
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/")

        page.goto(f"{BASE}/ha/cluster")
        toggle = page.locator(".node-maint-toggle[data-node=nb]")
        expect(toggle).to_be_visible(timeout=30000)
        check("no-badge-before", page.locator(".node-maint-badge").count() == 0)
        check("enter-offered", "Enter maintenance" in toggle.inner_text(), toggle.inner_text())
        toggle.click()
        panel = page.locator("#node-maint-dialog .node-maint-panel[data-node=nb]")
        expect(panel).to_be_visible(timeout=10000)
        try:
            expect(panel.locator("#node-maint-plan tbody tr")).to_have_count(2, timeout=10000)
        except AssertionError:
            err = page.locator("#node-maint-error")
            print("panel error:", err.inner_text() if err.count() else "-", flush=True)
            page.screenshot(path=os.environ.get("SHOT", "/tmp/node-maint-fail.png"), full_page=True)
            raise
        moves = panel.locator(".plan-moves-count")
        check("plan-count-is-a-link", moves.get_attribute("href") == "#node-maint-plan" and "2 guest(s)" in moves.inner_text(),
              moves.inner_text())
        panel.locator("#node-maint-enter").click()
        expect(page.locator("#node-maint-notice")).to_contain_text("nb is entering maintenance", timeout=10000)
        check("enter-request", c.posts and c.posts[-1] == ("nb", {"enabled": True, "drain": True, "force": False}), repr(c.posts))
        expect(panel.locator("#node-maint-state")).to_have_attribute("data-enabled", "true", timeout=10000)
        expect(panel.locator("#node-maint-drain")).to_contain_text("done", timeout=10000)
        badge = page.locator(".node-maint-badge")
        expect(badge).to_have_count(1, timeout=15000)
        check("badge-text", "maintenance (2 moved away)" in badge.inner_text(), badge.inner_text())
        check("badge-is-a-link", badge.get_attribute("href") == "/ha/maintenance?node=nb", badge.get_attribute("href") or "")
        expect(toggle).to_contain_text("Exit maintenance", timeout=10000)
        expect(panel.locator("#node-maint-exit")).to_be_enabled(timeout=10000)
        panel.locator("#node-maint-return").select_option("origin")
        expect(panel.locator("#node-maint-plan tbody tr").first).to_contain_text("nb", timeout=10000)
        panel.locator("#node-maint-exit").click()
        expect(page.locator("#node-maint-notice")).to_contain_text("nb left maintenance (return policy origin)", timeout=10000)
        check("exit-request", c.posts[-1] == ("nb", {"enabled": False, "drain": False, "force": False, "return": "origin"}),
              repr(c.posts[-1]))
        expect(page.locator(".node-maint-badge")).to_have_count(0, timeout=15000)
        check("badge-gone", True)

        # the HA > Maintenance page with the node picker
        c.enabled = True
        page.goto(f"{BASE}/ha/maintenance?node=nb")
        expect(page.locator(".node-maint-panel[data-node=nb]")).to_be_visible(timeout=30000)
        row = page.locator("#maint-members tr.maint-member[data-node=nb]")
        expect(row).to_have_attribute("data-state", "maintenance", timeout=15000)
        check("member-count-is-a-link", row.locator("a.maint-moved").get_attribute("href") == "/ha/maintenance?node=nb#node-maint-plan")
        page.locator("#maint-members tr.maint-member[data-node=nc] a").first.click()
        expect(page.locator(".node-maint-panel[data-node=nc]")).to_be_visible(timeout=15000)
        check("picker-switches-node", page.locator("#maint-node").input_value() == "nc", page.locator("#maint-node").input_value())

        check("no-js-errors", not errors, "; ".join(errors)[:500])
        browser.close()
    if fails:
        print(f"FAILED: {fails}")
        sys.exit(1)
    print("node maintenance UI e2e: all passed")


if __name__ == "__main__":
    main()
