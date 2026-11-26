#!/usr/bin/env python3
"""Infrastructure > Management Link (/network/management): empty state, live-state warnings, NIC picker and save.

The API is mocked in the browser (Playwright routes), so the page is tested against every state a node can be in without
touching the runner's network:
  1. no recorded link -> 424 empty state that says what is missing and what to do, with "Check again";
  2. a recorded 802.3ad bond + VLAN 42 whose second NIC is missing, no LACP partner and no address -> "Needs attention"
     and a warning that names each problem;
  3. the NIC picker lists the node's physical NICs (bonds/VLANs left out) plus the recorded missing one; choosing NICs,
     bond mode, VLAN and saving sends exactly that layout in the PUT body (apply_now false) and shows the result.
"""
import json
import os
import sys

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")

NOT_RECORDED = {"status": 424, "error": "NOT_AVAILABLE", "reason": "management_network_not_recorded",
                "message": "This node has no recorded management link",
                "details": "Set it once with the console menu (Management link: bond / VLAN); the page then shows and edits it"}
RECORD = {"version": 1, "interfaces": ["eth0", "eth1"], "vlan": 42, "address_interface": "bond0.42", "mode": "dhcp",
          "bond": {"name": "bond0", "mode": "802.3ad", "miimon": 100, "xmit_hash_policy": "layer3+4", "lacp_rate": "fast"},
          "dns": [], "stack": "dhcpcd"}
VIEW = {"record": RECORD,
        "live": {"bond_mode": "802.3ad", "lacp_partner": "00:00:00:00:00:00", "addresses": [], "address_interface_up": True,
                 "members": [{"name": "eth0", "link": "up"}, {"name": "eth1", "link": "missing"}]},
        "bond_modes": ["active-backup", "802.3ad", "balance-alb", "balance-tlb", "balance-xor", "balance-rr", "broadcast"],
        "hash_policies": ["layer2", "layer2+3", "layer3+4", "encap2+3", "encap3+4", "vlan+srcmac"]}
NICS = {"nics": [{"name": "eth0", "up": True, "speed_mbps": 1000, "mtu": 1500},
                 {"name": "eth2", "up": True, "speed_mbps": 10000, "mtu": 1500},
                 {"name": "bond0", "up": True, "mtu": 1500, "is_bond": True, "slaves": ["eth0"]},
                 {"name": "bond0.42", "up": True, "mtu": 1500}],
        "warnings": [], "peer_warnings": []}


def main():
    state = {"recorded": False, "puts": []}

    def mgmt(route):
        req = route.request
        if req.method == "PUT":
            body = json.loads(req.post_data or "{}")
            state["puts"].append(body)
            rec = dict(body["record"], address_interface="bond0.7")
            route.fulfill(status=200, content_type="application/json",
                          body=json.dumps({"record": rec, "applied": False, "log": ["netifrc: /etc/conf.d/net written"]}))
        elif not state["recorded"]:
            route.fulfill(status=424, content_type="application/json", body=json.dumps(NOT_RECORDED))
        else:
            route.fulfill(status=200, content_type="application/json", body=json.dumps(VIEW))

    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(ignore_https_errors=True)
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.route("**/api/network/management", mgmt)
        page.route("**/api/cluster/nics", lambda r: r.fulfill(status=200, content_type="application/json",
                                                              body=json.dumps(NICS)))
        page.goto(f"{BASE}/login")
        page.locator("input[type=text], input:not([type])").first.fill("admin")
        page.locator("input[type=password]").fill(os.environ.get("ADMIN_PASSWORD", "admin"))
        page.locator("button[type=submit]").click()
        page.wait_for_url(f"{BASE}/")

        # 1. empty state
        page.goto(f"{BASE}/network/management")
        expect(page.get_by_text("no recorded management link").first).to_be_visible(timeout=30000)
        expect(page.get_by_text("console menu").first).to_be_visible()
        assert "Management Link" in page.title(), page.title()
        state["recorded"] = True
        page.get_by_role("button", name="Check again").click()

        # 2. live state + warnings
        expect(page.locator("#mgmt-state")).to_be_visible(timeout=30000)
        expect(page.locator("#mgmt-health-badge")).to_have_text("Needs attention")
        health = page.locator("#mgmt-health").inner_text()
        for want in ("no DHCP answer on VLAN 42", "eth1 does not exist", "No LACP partner"):
            assert want in health, f"warning lacks {want!r}: {health}"
        assert "bond0 (802.3ad)" in page.locator("#mgmt-bond").inner_text()
        assert page.locator("#mgmt-vlan").inner_text().strip() == "42"

        # 3. NIC picker: physical NICs + the recorded missing one, no bond / VLAN devices
        names = sorted(page.locator("input.mgmt-nic").evaluate_all("els => els.map(e => e.value)"))
        assert names == ["eth0", "eth1", "eth2"], names
        assert page.locator("input.mgmt-nic[value=eth0]").is_checked()
        assert page.locator("input.mgmt-nic[value=eth1]").is_checked()
        assert "missing" in page.locator("label.nic-choice", has_text="eth1").inner_text()
        page.locator("input.mgmt-nic[value=eth1]").uncheck()
        expect(page.locator("#mgmt-bond-mode")).to_have_count(0)  # one NIC: no bond fields
        page.locator("input.mgmt-nic[value=eth2]").check()
        expect(page.locator("#mgmt-bond-mode")).to_be_visible()
        page.select_option("#mgmt-bond-mode", "active-backup")
        expect(page.locator("#mgmt-lacp-rate")).to_have_count(0)
        page.fill("#mgmt-vlan-id", "7")
        page.click("#mgmt-save")
        expect(page.locator("#mgmt-notice")).to_contain_text("Saved for the next boot", timeout=30000)
        assert len(state["puts"]) == 1, state["puts"]
        put = state["puts"][0]
        rec = put["record"]
        assert put["apply_now"] is False, put
        assert rec["interfaces"] == ["eth0", "eth2"], rec
        assert rec["vlan"] == 7 and rec["mode"] == "dhcp", rec
        assert rec["bond"]["mode"] == "active-backup" and rec["bond"]["name"] == "bond0", rec
        assert rec["bond"].get("lacp_rate") is None, rec

        # a bad VLAN never reaches the API
        page.fill("#mgmt-vlan-id", "5000")  # max 4094: the browser refuses the submit
        page.click("#mgmt-save")
        assert len(state["puts"]) == 1, state["puts"]
        assert not errors, errors
        browser.close()
    print("ok - management link page: empty state, warnings, NIC picker, save")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as e:
        print(f"FAIL: {e}")
        sys.exit(1)
