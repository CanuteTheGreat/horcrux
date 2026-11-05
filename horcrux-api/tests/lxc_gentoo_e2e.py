#!/usr/bin/env python3
"""Real Gentoo (OpenRC/netifrc) LXC create through the horcrux-api (run via
`LXC_E2E_PY=lxc_gentoo_e2e.py lxc_e2e.sh test <horcrux-api>`).

Downloads gentoo/current/openrc (~250 MB) from images.linuxcontainers.org, creates a
container with NO template (the default), a static IP + gateway + DNS and a DHCP NIC,
starts it and checks the guest's real network state. Not part of CI (size/network).
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lxc_e2e import (LXC_PATH, TOKEN, check, exec_ct, fail, lxc_state, req, sh,  # noqa: E402
                     step, wait_for)


def main():
    step("login")
    _, body = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN[0] = body.get("ticket") or body.get("token")
    check(bool(TOKEN[0]), "got an API token")

    step("download gentoo/current/openrc (pveam download, sha256 verified)")
    _, t = req("POST", "/lxc/templates", {"distro": "gentoo", "release": "current", "variant": "openrc"})
    check(len(t["sha256"]) == 64, f"template {t['id']} sha256 {t['sha256'][:12]}")

    step("create with NO template: static IP + gateway + DNS on eth0, DHCP on eth1")
    ct = {"id": "gt1", "name": "gt1", "runtime": "lxc", "memory": 512, "cpus": 1,
          "rootfs": "", "status": "stopped",
          "lxc": {"unprivileged": False, "disk_gb": 3, "nameserver": "10.77.0.1",
                  "nets": [{"bridge": "hxbr0", "ip": "10.77.0.60/24", "gateway": "10.77.0.1"},
                           {"bridge": "hxbr0", "ip": "dhcp"}]}}
    _, c = req("POST", "/containers", ct)
    check(c["rootfs"] == "gentoo/current/openrc", f"empty template became {c['rootfs']}")
    meta = open(f"{LXC_PATH}/gt1/horcrux.json").read() if os.path.exists(f"{LXC_PATH}/gt1/horcrux.json") else ""
    log_meta = sh(f"ls {LXC_PATH}/gt1")
    print("ct dir:", log_meta, "meta mentions netifrc:", "netifrc" in meta, flush=True)
    req("POST", "/containers/gt1/start", None)
    wait_for("running", lambda: "RUNNING" in lxc_state("gt1"), 60)
    def softexec(cmd):
        # exec answers 500 until the guest finished booting
        st, body = req("POST", "/containers/gt1/exec", {"command": ["sh", "-c", cmd]}, expect=None)
        return body["output"].strip() if st == 200 else ""
    wait_for("openrc up", lambda: softexec("rc-status -r 2>&1 | head -1 | grep -q default && echo y") == "y", 180)

    step("guest sees what Horcrux wrote")
    conf = exec_ct("gt1", "cat /etc/conf.d/net")
    print(conf, flush=True)
    check('config_eth0="10.77.0.60/24"' in conf and 'routes_eth0="default via 10.77.0.1"' in conf, "conf.d/net static + route")
    check('config_eth1="dhcp"' in conf, "conf.d/net dhcp")
    wait_for("static address up", lambda: "10.77.0.60/24" in exec_ct("gt1", "ip -4 addr show eth0"), 90)
    check(True, "eth0 has 10.77.0.60/24")
    route = exec_ct("gt1", "ip -4 route show default")
    print(route, flush=True)
    check("via 10.77.0.1" in route, "default route via the gateway")
    wait_for("dhcp address on eth1", lambda: "inet 10.77.0." in exec_ct("gt1", "ip -4 addr show eth1"), 90)
    check(True, "eth1 got a DHCP address")
    check("nameserver 10.77.0.1" in exec_ct("gt1", "cat /etc/resolv.conf"), "resolv.conf has the configured DNS")
    check(exec_ct("gt1", "ping -c1 -W3 10.77.0.1 >/dev/null && echo y") == "y", "gateway pingable from the guest")
    check("gentoo" in exec_ct("gt1", ". /etc/os-release; echo $ID"), "guest is Gentoo")

    step("cleanup")
    req("POST", "/containers/gt1/stop", None, expect=None)
    req("DELETE", "/containers/gt1", None, expect=None)
    req("DELETE", "/lxc/templates/" + t["id"], None, expect=204)
    log("done")


def log(m):
    print(f"[lxc-gentoo-e2e] {m}", flush=True)


if __name__ == "__main__":
    main()
