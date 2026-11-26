"""Installer management link: bond (802.3ad LACP by default) + VLAN 42 on a trunk that carries ONLY tagged VLAN 42 frames.

    sw = Alpine live (OpenRC): eth0 = QEMU user net (fetches dnsmasq + curl), eth1/eth2 = the two trunks, bonded 802.3ad (a real LACP
         partner) with bond0.42 = 10.42.0.1/24 and a DHCP server on VLAN 42 only
    hx = Horcrux, two NICs (one per trunk), unattended install from the ISO under test with
         interfaces = "all", bond 802.3ad (layer3+4, lacp fast), vlan = 42, DHCP

Both trunks are hubs with vlans=(42,): untagged frames and other VLANs are dropped (LACPDUs pass), so a node that is not tagged
cannot get an address. Checks: the live system builds the link and gets a VLAN 42 lease during the install (MGMT-LINK-LIVE), the
installed node comes up on bond0.42 at first boot, LACP is negotiated with the partner, the web UI answers over HTTPS on the VLAN
address (443 with standard ports, else the port the node serves; --arg port=N pins it),
and the node stays reachable with either NIC unplugged. Init-agnostic: run it for the OpenRC and the systemd ISO.

    python3 -m vmtest run scenarios/install_bond_vlan.py --arg iso=/path/horcrux-server-openrc-daily-x86_64.iso \
        [--arg bond=802.3ad|active-backup|balance-alb] [--arg vlan=42] [--arg mem=4096]
"""
import os
import re
import shutil
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Network, Nic, NodeSpec, Test, cache  # noqa: E402
from vmtest.recipes import PW, make_answers, unattended_install  # noqa: E402

ALPINE = ("https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-virt-3.24.4-x86_64.iso",
          "157d353f8e54252cad2d9812a0d56bb7fa2231fc154a110e1d180d7480ef8b4f")
SW_IP = "10.42.0.1"


def define(args):
    bond = args.get("bond", "802.3ad")
    vlan = int(args.get("vlan", 42))
    mgmt = ['interfaces = "all"', f"vlan = {vlan}",
            f'bond = {{ mode = "{bond}", miimon = 100' + (', xmit_hash_policy = "layer3+4", lacp_rate = "fast"' if bond == "802.3ad" else "") + " }"]
    answers, key = make_answers("horcrux", mgmt=mgmt)

    def trunk(net):
        # 1000/full: 802.3ad keeps a member whose speed/duplex is unknown (the virtio-net default) down, on both ends
        return Nic(net, speed=1000, duplex="full")
    hx = NodeSpec(name="hx", mem=int(args.get("mem", 4096)), cpus=2, iso=args["iso"], iso_media=args.get("media", "cdrom"),
                  firmware=args.get("mode", "bios"), disks=[Disk("24G")], extra_drives=[(answers, "usb")],
                  nics=[trunk("trunk-a"), trunk("trunk-b")], console="serial-root", login_password=PW, console_menu_key="8",
                  ssh_key=key, boot_timeout=420)
    sw = NodeSpec(name="sw", mem=512, cpus=1, iso=args.get("alpine") or cache.fetch(*ALPINE), disks=[Disk("1G")],
                  nics=[Nic("user", forwards={}), trunk("trunk-a"), trunk("trunk-b")], boot_timeout=240)

    def run(m):
        s, n = m.sw, m.hx
        m.phase("switch side: LACP partner + DHCP on VLAN only")
        s.start()
        s.wait_for_shell()
        s.succeed("ip link set eth0 up && (udhcpc -q -n -i eth0 -t 10 >/dev/null 2>&1 || true)",
                  "printf 'https://dl-cdn.alpinelinux.org/alpine/v3.24/main\\nhttps://dl-cdn.alpinelinux.org/alpine/v3.24/community\\n' > /etc/apk/repositories")
        s.wait_until_succeeds("apk add -q dnsmasq curl iptables iproute2", 240, 10)
        s.succeed("modprobe bonding max_bonds=0 && modprobe 8021q && (ip link del bond0 2>/dev/null; true)",
                  f"ip link add bond0 type bond mode {bond} miimon 100" + (" lacp_rate fast xmit_hash_policy layer3+4" if bond == "802.3ad" else ""),
                  "for i in eth1 eth2; do ip link set $i down; ip link set $i master bond0; ip link set $i up; done; ip link set bond0 up",
                  # busybox ip ignores bond options (the bond would silently be balance-rr): iproute2 is installed above
                  f"grep -q '^{bond} ' /sys/class/net/bond0/bonding/mode",
                  f"ip link add link bond0 name bond0.{vlan} type vlan id {vlan} && ip addr add {SW_IP}/24 dev bond0.{vlan} && ip link set bond0.{vlan} up",
                  # the VLAN is the node's only network: route + DNS through the helper (the install fetches packages)
                  "sysctl -qw net.ipv4.ip_forward=1 && iptables -t nat -A POSTROUTING -o eth0 -j MASQUERADE",
                  f"dnsmasq --interface=bond0.{vlan} --bind-interfaces --dhcp-range=10.42.0.100,10.42.0.150,12h "
                  f"--dhcp-option=3,{SW_IP} --dhcp-option=6,{SW_IP} "
                  "--dhcp-leasefile=/tmp/leases --log-dhcp --log-facility=/tmp/dnsmasq.log")
        m.check("switch-ready", True, f"{bond} bond on eth1+eth2, VLAN {vlan} {SW_IP}/24, DHCP 10.42.0.100-150 on VLAN {vlan} only")

        m.phase("unattended install with bond + VLAN")
        unattended_install(m, n)
        log = n.q.serial_text()
        live = re.search(r"MGMT-LINK-LIVE (ok|failed)[^\n]*", log)
        m.check("live-system-link-up-during-install", bool(live) and live.group(1) == "ok" and "10.42.0." in live.group(0),
                live.group(0) if live else "no MGMT-LINK-LIVE line")
        plan = re.search(r"AUTOINSTALL-PLAN[^\n]*", log)
        m.note("install-plan", bool(plan), plan.group(0) if plan else "")

        m.phase("first boot: management on the VLAN")
        n.start()
        m.check("installed-boots-to-login", n.q.expect(r"login:|Enter an option:", n.T(600)), "")
        n.wait_for_shell(600)
        m.check("installed-shell", True, n.init.name)
        bondinfo = n.succeed("cat /proc/net/bonding/bond0")
        slaves = re.findall(r"Slave Interface: (\S+)", bondinfo)
        m.check("bond-has-both-nics", len(slaves) == 2 and bondinfo.count("MII Status: up") >= 3, bondinfo[:900])
        want = {"802.3ad": "IEEE 802.3ad Dynamic link aggregation", "active-backup": "fault-tolerance (active-backup)",
                "balance-alb": "adaptive load balancing"}.get(bond, bond)
        m.check("bond-mode", want in bondinfo, want)
        if bond == "802.3ad":
            partner = re.findall(r"Partner Mac Address: (\S+)", bondinfo)
            m.check("lacp-negotiated-with-partner", bool(partner) and any(p != "00:00:00:00:00:00" for p in partner), str(partner))
        ip = ""
        end = time.time() + n.T(180)
        while time.time() < end and not ip:
            mm = re.search(r"inet (10\.42\.0\.\d+)/", n.execute(f"ip -4 -o addr show dev bond0.{vlan}", 20)[1])
            ip = mm.group(1) if mm else ""
            if not ip:
                time.sleep(3)
        m.check("dhcp-address-on-vlan", bool(ip), ip or n.execute("ip -o addr; cat /etc/dhcpcd.conf | tail -20", 20)[1][-900:])
        rec = n.succeed("cat /etc/horcrux/management-network.json")
        m.check("layout-recorded-for-the-product", f'"address_interface": "bond0.{vlan}"' in rec and f'"mode": "{bond}"' in rec, rec[:600])
        if n.init.name == "systemd":
            files = n.succeed("ls /etc/systemd/network/")
            m.check("networkd-files", "10-mgmt-bond0.netdev" in files and f"10-mgmt-bond0.{vlan}.netdev" in files, files)
        else:
            net = n.succeed("cat /etc/conf.d/net; ls /etc/runlevels/default/")
            m.check("netifrc-config", f'vlans_bond0="{vlan}"' in net and "net.bond0" in net, net[-900:])
        m.note("untagged-frames-dropped-by-trunk", True, f"trunk-a {m.network('trunk-a').stats} trunk-b {m.network('trunk-b').stats}")

        m.phase("HTTPS on the VLAN address")
        # the web UI port the node serves: 443 once standard ports are the product default, else the configured port (8006).
        # --arg port=N pins it; the check below then requires that port.
        port = args.get("port")
        if not port:
            listen = ""
            end = time.time() + n.T(300)
            while time.time() < end and "horcrux-api" not in listen:
                listen = n.execute("ss -Htlnp 2>/dev/null | grep horcrux-api", 20)[1]
                if "horcrux-api" not in listen:
                    time.sleep(5)
            ports = sorted({int(p) for p in re.findall(r":(\d+)\s", listen)})
            port = 443 if 443 in ports else (8006 if 8006 in ports else (ports[0] if ports else 443))
            m.note("web-ui-on-standard-port-443", port == 443, f"horcrux-api listens on {ports or 'nothing'}")
        port = int(port)
        base = f"https://{ip}" + ("" if port == 443 else f":{port}")

        def https(path="/"):
            rc, out = s.execute(f"curl -sk -o /dev/null -w '%{{http_code}}' --max-time 8 {base}{path}", 20)
            return out.strip()[-3:]
        s.wait_until(lambda: https("/api/health") == "200", 420, 5, f"{base}/api/health 200")
        m.check(f"https-{port}-on-vlan-address", https("/") == "200", f"{base}/ from the switch side")

        m.phase("unplug one NIC at a time")
        for i in (0, 1):
            n.set_link(i, False)
            time.sleep(3)
            s.wait_until(lambda: https("/api/health") == "200", 60, 2, f"health with NIC {i} unplugged")
            st = n.succeed("cat /proc/net/bonding/bond0")
            m.check(f"reachable-with-nic{i}-unplugged", "MII Status: down" in st, re.sub(r"\s+", " ", st)[:400])
            n.set_link(i, True)
            n.wait_until(lambda: n.execute("cat /proc/net/bonding/bond0", 20)[1].count("MII Status: up") >= 3, 60, 2, "both slaves up again")
        m.check("both-nics-back", True, "")
        n.power_off()
        shutil.rmtree(os.path.dirname(key), ignore_errors=True)

    return Test(f"install-bond-vlan-{bond}", nodes=[sw, hx], run=run, timeout=5400,
                networks=[Network("trunk-a", vlans=(vlan,)), Network("trunk-b", vlans=(vlan,))],
                description="installer: bond + VLAN management link on a tagged-only trunk, HTTPS on the VLAN, NIC failover")
