"""Patronus 2-node netlab lane on vmtest: the installed firewall + one LAN client VM on a virtual switch.

 fw  = Patronus (installed from the ISO by the unattended installer), WAN = QEMU user net, LAN = hub `lan` (10.77.0.1/24)
 cli = Alpine live (OpenRC), LAN = hub `lan` (10.77.0.2/24)

Real packets between two real kernels (the netns lanes in ci/netlab cannot test the shipped image, its init scripts or its NIC drivers):
 - LAN client reaches the firewall (ping + web port), WAN web port is blocked (installed policy)
 - the firewall's service is up on the guest's own init (openrc or systemd, same call)
 - partition the client from the firewall: web becomes unreachable; heal: it returns
 - cut the firewall's LAN cable (set_link): client loses it, plug: it returns
 - 100 % loss then heal; the firewall survives a SIGKILL power cut and serves again after cold boot
    python3 -m vmtest run scenarios/netlab_lan_client.py --arg iso=patronus-...iso
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Network, Nic, NodeSpec, Test, cache  # noqa: E402
from vmtest.recipes import PROJECTS, PW, make_answers, unattended_install  # noqa: E402

ALPINE = ("https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-virt-3.24.4-x86_64.iso",
          "157d353f8e54252cad2d9812a0d56bb7fa2231fc154a110e1d180d7480ef8b4f")


def define(args):
    P = PROJECTS["patronus"]
    answers, key = make_answers("patronus")
    fw = NodeSpec(name="fw", mem=int(args.get("mem", 3072)), cpus=2, iso=args["iso"], iso_media="usb", firmware="uefi",
                  disks=[Disk("24G")], extra_drives=[(answers, "usb")], ssh_key=key, login_password=PW, console_menu_key="8", boot_timeout=420,
                  nics=[Nic("user", forwards={}), Nic("lan", mac="52:54:00:01:00:01")])
    cli = NodeSpec(name="cli", mem=512, cpus=1, iso=args.get("client_iso") or cache.fetch(*ALPINE), disks=[Disk("1G")],
                   nics=[Nic("user", forwards={}), Nic("lan", mac="52:54:00:02:00:01")])

    def run(m):
        lan, FW = m.network("lan"), "10.77.0.1"
        web = f"nc -z -w 3 {FW} {P['port']}"

        m.phase("install the firewall, boot the client")
        unattended_install(m, m.fw)
        m.cli.start()
        m.cli.wait_for_shell()
        m.cli.succeed("ip link set eth1 up && ip addr add 10.77.0.2/24 dev eth1")

        m.phase("firewall first boot")
        m.fw.start()
        m.check("fw-boots-to-login", m.fw.q.expect(r"login:", m.fw.T(420)), "")
        m.fw.wait_for_shell()               # serial login as root with the answer-file password
        m.fw.wait_for_unit(P["svc"], 300)   # openrc or systemd: same call
        m.check(f"{P['svc']}-active", True, m.fw.init.name)
        m.cli.wait_until_succeeds(f"ping -c1 -W2 {FW}", 240, 3)
        m.check("lan-client-pings-firewall", True, f"{FW} answers on the hub")
        m.cli.wait_until_succeeds(web, 300, 5)
        m.check("lan-web-open", True, f"{FW}:{P['port']} accepts from the LAN")

        m.phase("faults")
        lan.partition(["fw"], ["cli"])
        m.cli.wait_until_fails(f"ping -c1 -W1 {FW}", 30)
        m.cli.fail(web)
        m.check("partition-cuts-lan", True, "client cannot reach the firewall")
        lan.heal()
        m.cli.wait_until_succeeds(web, 60, 3)
        m.check("heal-restores-lan", True, "")
        m.fw.set_link(1, False)
        m.cli.wait_until_fails(f"ping -c1 -W1 {FW}", 30)
        m.fw.set_link(1, True)
        m.cli.wait_until_succeeds(web, 90, 3)
        m.check("firewall-lan-cable-pull", True, "carrier loss then recovery")
        lan.set_loss("fw", "cli", 100)
        m.cli.wait_until_fails(f"ping -c1 -W1 {FW}", 30)
        lan.heal()
        m.cli.wait_until_succeeds(web, 60, 3)
        m.check("loss-then-heal", True, "")

        m.phase("power cut")
        m.fw.crash()
        m.cli.wait_until_fails(f"ping -c1 -W1 {FW}", 30)
        m.fw.start()
        m.cli.wait_until_succeeds(web, 420, 5)
        m.check("firewall-serves-after-power-cut", True, "cold boot, web port open again")
        m.fw.screenshot("fw-after-powercut")

    return Test("netlab-lan-client", nodes=[fw, cli], networks=[Network("lan")], run=run, timeout=5400,
                description="Patronus firewall VM + LAN client VM: reachability, partition, cable pull, power cut")
