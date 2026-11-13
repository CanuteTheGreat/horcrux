"""Framework self-test: 3 Alpine (OpenRC) guests on one virtual LAN.

Proves: boot + serial shell, init-agnostic unit API on OpenRC, hub network, partition / heal / loss
fault injection, link flap, savevm/loadvm snapshot, SIGKILL power loss + reboot, screenshot.
`--arg fail=1` makes it fail on purpose so CI can check that failure artifacts are collected.
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Network, Nic, NodeSpec, Test, cache  # noqa: E402

ALPINE = ("https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-virt-3.24.4-x86_64.iso",
          "157d353f8e54252cad2d9812a0d56bb7fa2231fc154a110e1d180d7480ef8b4f")


def define(args):
    iso = args.get("iso") or cache.fetch(*ALPINE)
    nodes = [NodeSpec(name=f"n{i}", mem=512, cpus=1, iso=iso, disks=[Disk("1G")], nics=[Nic("user", forwards={}), Nic("lan")],
                      boot_timeout=240) for i in (1, 2, 3)]

    def run(m):
        ip = {"n1": "10.9.0.1", "n2": "10.9.0.2", "n3": "10.9.0.3"}
        lan = m.network("lan")
        m.phase("boot")
        m.start_all()
        for n in m.nodes.values():
            n.wait_for_shell()
        m.check("all-nodes-have-a-shell", True, ", ".join(m.nodes))
        for n in m.nodes.values():
            m.check(f"{n.name}-init-is-openrc", n.init.name == "openrc", n.init.name)

        m.phase("init-agnostic service API (OpenRC)")
        n1 = m.n1
        # busybox crond ships with Alpine; stop/start it through the API only (no rc-service in the test)
        n1.execute(n1.init.start("crond"))
        n1.wait_for_unit("crond")
        n1.stop_unit("crond")
        n1.wait_for_unit_stopped("crond")
        n1.start_unit("crond")
        n1.wait_for_unit("crond")
        m.check("unit-start-stop-wait", True, "crond stop -> stopped, start -> active")
        n1.succeed("mkdir -p /etc/init.d && printf '#!/sbin/openrc-run\\ncommand=/bin/false\\n' > /etc/init.d/vmt-bad && chmod +x /etc/init.d/vmt-bad")
        n1.execute(n1.init.start("vmt-bad"))
        try:
            n1.wait_for_unit("vmt-bad", timeout=15)
            m.check("failed-unit-detected", False, "wait_for_unit returned for a unit that cannot start")
        except Exception as e:   # Timeout or UnitFailed: both mean it never became active
            m.check("failed-unit-detected", True, type(e).__name__)

        m.phase("virtual network")
        for n in m.nodes.values():
            n.succeed(f"ip link set eth1 up && ip addr add {ip[n.name]}/24 dev eth1")
        for a in ip:
            for b in ip:
                if a != b:
                    m[a].wait_until_succeeds(f"ping -c1 -W2 {ip[b]}", 30)
        m.check("full-mesh-ping", True, "3 nodes reach each other over the hub")

        lan.partition(["n1"], ["n2", "n3"])
        m["n1"].wait_until_fails(f"ping -c1 -W1 {ip['n2']}", 20)
        m["n2"].succeed(f"ping -c2 -W2 {ip['n3']}")
        m.check("partition-n1-cut-off", True, "n1 -> n2 fails, n2 -> n3 still works")
        lan.heal()
        m["n1"].wait_until_succeeds(f"ping -c1 -W2 {ip['n2']}", 30)
        m.check("heal-restores-traffic", True, "")
        lan.set_loss("n2", "n3", 100)
        m["n2"].wait_until_fails(f"ping -c1 -W1 {ip['n3']}", 20)
        lan.heal()
        m["n2"].wait_until_succeeds(f"ping -c1 -W2 {ip['n3']}", 30)
        m.check("loss-injection", True, "100% loss n2<->n3, then healed")
        m["n3"].set_link(1, False)
        m["n3"].wait_until_fails("grep -q 1 /sys/class/net/eth1/carrier", 15)   # carrier file reads 0 / errors with the cable out
        m["n3"].set_link(1, True)
        m["n3"].wait_until_succeeds("grep -q 1 /sys/class/net/eth1/carrier", 15)
        m.check("link-flap", True, "set_link down/up seen as carrier change")

        m.phase("snapshot / restore")
        m.snapshot_all("clean")
        m["n2"].succeed("echo dirty > /tmp/marker")
        m.restore_all("clean")
        m["n2"].fail("test -e /tmp/marker")
        m.check("snapshot-restore", True, "state written after the snapshot is gone after loadvm")

        m.phase("power loss")
        m["n2"].crash()
        m["n2"].start()
        m["n2"].wait_for_shell()
        m.check("crash-and-reboot", True, "SIGKILL then cold boot gives a shell again")

        m.phase("screen")
        png = m["n1"].screenshot("selftest")
        m.check("screenshot", os.path.getsize(png) > 200, png)

        if args.get("fail"):
            m["n1"].succeed("false")   # deliberate: CI asserts that the artifacts of this failure exist

    return Test("selftest-alpine", nodes=nodes, networks=[Network("lan")], run=run, timeout=900,
                description="3 OpenRC guests on a hub: boot, units, partition, snapshot, crash")
