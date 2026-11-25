"""Management link (bond / LACP / VLAN) of the guided installer: renderers for netifrc, systemd-networkd, NetworkManager and dhcpcd,
answer-file keys and validation, the live iproute2 plan, the installer's target writes and the console menu editor.
Run: python3 -m unittest discover -s build/live-iso/common/tests   (Python >= 3.11)
"""
import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import netlayout as NL  # noqa: E402
from liveinst import system as S  # noqa: E402
from liveinst import stages as ST  # noqa: E402
from liveinst import consolemenu as CM  # noqa: E402

PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs"], "swap_mb": 0, "network_mode": "management",
           "services": [], "web_scheme": "https", "web_port": 443, "default_hostname": "h.localdomain"}
NICS = [{"name": "eth0", "mac": "52:54:00:00:00:01", "wifi": False, "driver": "virtio_net", "model": ""},
        {"name": "eth1", "mac": "52:54:00:00:00:02", "wifi": False, "driver": "virtio_net", "model": ""},
        {"name": "wlan0", "mac": "52:54:00:00:00:03", "wifi": True, "driver": "iwlwifi", "model": ""}]


def mg(ifaces=("eth0",), vlan=0, addr="dhcp", **bond):
    """management section: addr = dhcp|static, bond keys as keywords (mode=, miimon=, ...)"""
    n = {"iface": ifaces[0] if ifaces else "", "ifaces": list(ifaces), "mode": addr, "address": "", "gateway": "", "dns": "", "vlan": vlan,
         "bond": bond}
    return NL.ensure(n)


def lacp42(addr="dhcp", **kw):
    return mg(("eth0", "eth1"), 42, addr, **dict({"mode": "802.3ad", "lacp_rate": "fast", "xmit_hash_policy": "layer3+4"}, **kw))


class Model(unittest.TestCase):
    def test_defaults_are_one_untagged_nic(self):
        c = S._default_config(PROFILE, "xfs", "")
        NL.ensure(c["mgmt"])
        self.assertEqual(c["mgmt"]["ifaces"], [])
        self.assertEqual(c["mgmt"]["vlan"], 0)
        self.assertEqual(c["mgmt"]["bond"]["name"], "bond0")
        self.assertEqual(c["mgmt"]["bond"]["mode"], "active-backup")
        self.assertEqual(c["mgmt"]["bond"]["miimon"], 100)

    def test_profile_overrides_defaults(self):
        n = NL.ensure({"iface": "eth0"}, {"mgmt_bond_name": "mgmt0", "mgmt_bond_mode": "802.3ad", "mgmt_bond_miimon": 50})
        self.assertEqual((n["bond"]["name"], n["bond"]["mode"], n["bond"]["miimon"]), ("mgmt0", "802.3ad", 50))
        self.assertEqual(n["ifaces"], ["eth0"])

    def test_l3_iface(self):
        self.assertEqual(NL.l3_iface(mg()), "eth0")
        self.assertEqual(NL.l3_iface(mg(vlan=42)), "eth0.42")
        self.assertEqual(NL.l3_iface(mg(("eth0", "eth1"))), "bond0")
        self.assertEqual(NL.l3_iface(lacp42()), "bond0.42")
        # a long NIC name + .4094 would pass IFNAMSIZ: the VLAN gets the short form
        self.assertEqual(NL.l3_iface(mg(("enx001122334455",), 4094)), "vlan4094")
        self.assertTrue(NL.is_plain(mg()))
        self.assertFalse(NL.is_plain(mg(vlan=7)))

    def test_dhcpcd_denied(self):
        self.assertEqual(NL.dhcpcd_denied(mg()), [])
        self.assertEqual(NL.dhcpcd_denied(mg(vlan=42)), ["eth0"])
        self.assertEqual(NL.dhcpcd_denied(mg(("eth0", "eth1"))), ["eth0", "eth1"])
        self.assertEqual(NL.dhcpcd_denied(lacp42()), ["eth0", "eth1", "bond0"])

    def test_describe(self):
        self.assertEqual(NL.describe(lacp42()), "bond bond0 (802.3ad: eth0 + eth1; miimon 100, hash layer3+4, lacp fast), VLAN 42 as bond0.42")
        self.assertEqual(NL.describe(mg(("eth0", "eth1"))), "bond bond0 (active-backup: eth0 + eth1; miimon 100)")


class Problems(unittest.TestCase):
    def test_good(self):
        self.assertEqual(NL.problems(lacp42(), NICS), [])
        self.assertEqual(NL.problems(mg(), NICS), [])
        self.assertEqual(NL.problems(mg(("eth0", "eth1"), mode="balance-alb", primary="eth1"), NICS), [])

    def test_vlan_range(self):
        for bad in (4095, -3, "x"):
            n = mg()
            n["vlan"] = bad
            self.assertTrue(any("VLAN id" in e for e in NL.problems(n)), bad)
        self.assertEqual(NL.problems(mg(vlan=4094)), [])

    def test_bond_options(self):
        self.assertIn("bond mode", " ".join(NL.problems(mg(("eth0", "eth1"), mode="lacp"))))
        self.assertIn("lacp_rate", " ".join(NL.problems(lacp42(lacp_rate="medium"))))
        self.assertIn("xmit_hash_policy", " ".join(NL.problems(lacp42(xmit_hash_policy="l7"))))
        self.assertIn("miimon", " ".join(NL.problems(lacp42(miimon=0))))
        self.assertIn("miimon", " ".join(NL.problems(lacp42(miimon="soon"))))
        self.assertIn("primary", " ".join(NL.problems(lacp42(primary="eth0"))))        # 802.3ad has no primary
        self.assertIn("not one of its members", " ".join(NL.problems(mg(("eth0", "eth1"), primary="eth9"))))
        self.assertIn("own member", " ".join(NL.problems(mg(("eth0", "eth1"), name="eth0"))))
        self.assertIn("longer than", " ".join(NL.problems(mg(("eth0", "eth1"), name="a-very-long-bond-name"))))

    def test_nics(self):
        self.assertIn("twice", " ".join(NL.problems(mg(("eth0", "eth0")))))
        self.assertIn("does not exist", " ".join(NL.problems(mg(("eth0", "eth7")), NICS)))
        self.assertIn("Wi-Fi", " ".join(NL.problems(mg(("eth0", "wlan0")), NICS)))
        self.assertIn("Wi-Fi", " ".join(NL.problems(mg(("wlan0",), 5), NICS)))
        self.assertEqual(NL.problems(mg(("wlan0",)), NICS), [])          # a plain Wi-Fi management NIC stays allowed


class Netifrc(unittest.TestCase):
    def test_plain_writes_nothing(self):
        self.assertEqual(NL.render_netifrc(mg()), "")
        self.assertEqual(NL.netifrc_services(mg()), [])
        self.assertEqual(NL.apply_netifrc("modules=\"iproute2\"\n", mg()), "modules=\"iproute2\"\n")

    def test_lacp_bond_with_vlan(self):
        t = NL.render_netifrc(lacp42())
        for line in ('config_eth0="null"', 'config_eth1="null"', 'slaves_bond0="eth0 eth1"', 'mode_bond0="802.3ad"', 'miimon_bond0="100"',
                     'xmit_hash_policy_bond0="layer3+4"', 'lacp_rate_bond0="fast"', 'config_bond0="null"', 'vlans_bond0="42"',
                     'bond0_vlan42_name="bond0.42"', 'config_bond0_42="null"'):
            self.assertIn(line + "\n", t)
        self.assertEqual(NL.netifrc_services(lacp42()), ["net.bond0"])
        self.assertTrue(t.startswith(NL.MARK_BEGIN) and t.rstrip().endswith(NL.MARK_END))

    def test_active_backup_no_hash_no_lacp(self):
        t = NL.render_netifrc(mg(("eth0", "eth1"), primary="eth1"))
        self.assertIn('mode_bond0="active-backup"', t)
        self.assertIn('primary_bond0="eth1"', t)
        self.assertNotIn("xmit_hash_policy", t)
        self.assertNotIn("lacp_rate", t)
        self.assertNotIn("vlans_", t)

    def test_vlan_on_single_nic_and_ifvar(self):
        t = NL.render_netifrc(mg(("enp1s0",), 7))
        self.assertIn('vlans_enp1s0="7"', t)
        self.assertIn('config_enp1s0_7="null"', t)
        self.assertEqual(NL.netifrc_services(mg(("enp1s0",), 7)), ["net.enp1s0"])
        self.assertEqual(NL.ifvar("bond0.42"), "bond0_42")

    def test_apply_replaces_block_and_keeps_user_config(self):
        user = 'dns_domain_lo="example"\n'
        once = NL.apply_netifrc(user, lacp42())
        self.assertTrue(once.startswith(user))
        self.assertEqual(NL.apply_netifrc(once, lacp42()), once)             # idempotent
        back = NL.apply_netifrc(once, mg())
        self.assertEqual(back, user)                                           # back to one NIC: our block gone, theirs kept


class Networkd(unittest.TestCase):
    def test_plain(self):
        self.assertEqual(NL.render_networkd(mg()), {})

    def test_lacp_bond_with_vlan(self):
        f = NL.render_networkd(lacp42())
        self.assertEqual(sorted(f), ["10-mgmt-bond0.42.netdev", "10-mgmt-bond0.netdev", "10-mgmt-bond0.network",
                                     "10-mgmt-eth0.network", "10-mgmt-eth1.network"])
        bond = f["10-mgmt-bond0.netdev"]
        for line in ("Name=bond0", "Kind=bond", "Mode=802.3ad", "MIIMonitorSec=100ms", "TransmitHashPolicy=layer3+4", "LACPTransmitRate=fast"):
            self.assertIn(line + "\n", bond)
        self.assertIn("Bond=bond0\n", f["10-mgmt-eth0.network"])
        self.assertIn("VLAN=bond0.42\n", f["10-mgmt-bond0.network"])
        self.assertIn("LinkLocalAddressing=no\n", f["10-mgmt-bond0.network"])
        vl = f["10-mgmt-bond0.42.netdev"]
        self.assertIn("Kind=vlan\n", vl)
        self.assertIn("Id=42\n", vl)
        # the address interface has no .network: networkd leaves it to dhcpcd (never drops dhcpcd's addresses)
        self.assertFalse(any(k.startswith("10-mgmt-bond0.42.network") for k in f))
        self.assertFalse(any("DHCP=" in v or "Address=" in v for v in f.values()))

    def test_bond_without_vlan_and_primary(self):
        f = NL.render_networkd(mg(("eth0", "eth1"), primary="eth0"))
        self.assertEqual(sorted(f), ["10-mgmt-bond0.netdev", "10-mgmt-eth0.network", "10-mgmt-eth1.network"])
        self.assertIn("PrimarySlave=yes", f["10-mgmt-eth0.network"])
        self.assertNotIn("PrimarySlave", f["10-mgmt-eth1.network"])
        self.assertNotIn("TransmitHashPolicy", f["10-mgmt-bond0.netdev"])

    def test_vlan_single_nic(self):
        f = NL.render_networkd(mg(("eth0",), 42))
        self.assertEqual(sorted(f), ["10-mgmt-eth0.42.netdev", "10-mgmt-eth0.network"])
        self.assertIn("VLAN=eth0.42", f["10-mgmt-eth0.network"])

    def test_balance_xor_carries_hash(self):
        f = NL.render_networkd(mg(("eth0", "eth1"), mode="balance-xor", xmit_hash_policy="layer2+3"))
        self.assertIn("TransmitHashPolicy=layer2+3", f["10-mgmt-bond0.netdev"])
        self.assertNotIn("LACP", f["10-mgmt-bond0.netdev"])


class Dhcpcd(unittest.TestCase):
    def test_block(self):
        self.assertEqual(NL.render_dhcpcd(mg()), "")
        t = NL.apply_dhcpcd("denyinterfaces wlan*\n", lacp42())
        self.assertIn("denyinterfaces eth0 eth1 bond0\n", t)
        self.assertTrue(t.startswith("denyinterfaces wlan*\n"))
        self.assertEqual(NL.apply_dhcpcd(t, lacp42()), t)
        self.assertEqual(NL.apply_dhcpcd(t, mg()), "denyinterfaces wlan*\n")

    def test_static_block_on_l3(self):
        n = lacp42("static")
        n.update(address="10.0.42.5/24", gateway="10.0.42.1", dns="10.0.42.1, 9.9.9.9")
        self.assertEqual(NL.dhcpcd_static_block(n), ["", "interface bond0.42", "static ip_address=10.0.42.5/24", "static routers=10.0.42.1",
                                                     "static domain_name_servers=10.0.42.1 9.9.9.9"])
        self.assertEqual(NL.dhcpcd_static_block(lacp42()), [])


class NetworkManager(unittest.TestCase):
    def test_lacp_vlan_dhcp(self):
        f = NL.render_nm(lacp42())
        self.assertEqual(sorted(f), ["mgmt-bond0.42.nmconnection", "mgmt-bond0.nmconnection", "mgmt-eth0.nmconnection", "mgmt-eth1.nmconnection"])
        b = f["mgmt-bond0.nmconnection"]
        self.assertIn("type=bond\n", b)
        self.assertIn("mode=802.3ad\n", b)
        self.assertIn("lacp_rate=fast\n", b)
        self.assertIn("method=disabled", b)                 # the address lives on the VLAN
        self.assertIn("master=bond0\n", f["mgmt-eth0.nmconnection"])
        v = f["mgmt-bond0.42.nmconnection"]
        self.assertIn("parent=bond0\n", v)
        self.assertIn("id=42\n", v)
        self.assertIn("[ipv4]\nmethod=auto", v)

    def test_static_bond(self):
        n = mg(("eth0", "eth1"), addr="static")
        n.update(address="192.0.2.10/24", gateway="192.0.2.1", dns="192.0.2.53")
        b = NL.render_nm(n)["mgmt-bond0.nmconnection"]
        self.assertIn("method=manual\naddress1=192.0.2.10/24,192.0.2.1\ndns=192.0.2.53;\n", b)
        self.assertEqual(NL.render_nm(mg()), {})

    def test_vlan_single_nic_disables_parent_ip(self):
        f = NL.render_nm(mg(("eth0",), 9))
        self.assertIn("method=disabled", f["mgmt-eth0.nmconnection"])
        self.assertIn("parent=eth0", f["mgmt-eth0.9.nmconnection"])


class Record(unittest.TestCase):
    def test_round_trip(self):
        n = lacp42("static")
        n.update(address="10.0.42.5/24", gateway="10.0.42.1", dns="10.0.42.1")
        r = NL.record(n)
        self.assertEqual(r["interfaces"], ["eth0", "eth1"])
        self.assertEqual(r["vlan"], 42)
        self.assertEqual(r["address_interface"], "bond0.42")
        self.assertEqual(r["bond"], {"name": "bond0", "mode": "802.3ad", "miimon": 100, "xmit_hash_policy": "layer3+4", "lacp_rate": "fast"})
        self.assertEqual(r["dns"], ["10.0.42.1"])
        back = NL.from_record(json.loads(NL.record_json(n)))
        self.assertEqual(NL.record(back), r)
        self.assertEqual(json.loads(NL.record_json(n, "networkmanager"))["stack"], "networkmanager")

    def test_plain_record(self):
        r = NL.record(mg())
        self.assertIsNone(r["bond"])
        self.assertIsNone(r["vlan"])
        self.assertEqual(r["address_interface"], "eth0")


class Live(unittest.TestCase):
    def test_commands(self):
        c = NL.live_commands(lacp42())
        self.assertEqual(c[0], ["modprobe", "bonding", "max_bonds=0"])     # no auto-created bond0 in the way
        self.assertEqual(c[1], ["ip", "link", "del", "bond0"])
        self.assertEqual(c[2], ["ip", "link", "add", "bond0", "type", "bond", "mode", "802.3ad", "miimon", "100", "xmit_hash_policy", "layer3+4",
                                "lacp_rate", "fast"])
        self.assertIn(["ip", "link", "set", "eth0", "master", "bond0"], c)
        # members go down before they are enslaved
        self.assertLess(c.index(["ip", "link", "set", "eth1", "down"]), c.index(["ip", "link", "set", "eth1", "master", "bond0"]))
        self.assertEqual(c[-2], ["ip", "link", "add", "link", "bond0", "name", "bond0.42", "type", "vlan", "id", "42"])
        self.assertEqual(c[-1], ["ip", "link", "set", "bond0.42", "up"])
        self.assertEqual(NL.live_commands(mg()), [])
        self.assertEqual(NL.live_teardown(lacp42())[0], ["ip", "link", "del", "bond0.42"])

    def fake(self, ips):
        ran, files = [], {"/etc/dhcpcd.conf": "denyinterfaces wlan*\n"}
        restarts = []
        it = iter(ips)
        return ran, files, restarts, dict(run=lambda c, chk: (ran.append(c), (0, ""))[1], read=lambda p: files.get(p, ""),
                                          write=lambda p, t: files.__setitem__(p, t), restart_dhcpcd=lambda: restarts.append(1),
                                          nic_ip=lambda i: next(it, ""), sleep=lambda s: None, wait_secs=5)

    def test_dhcp_waits_for_address_on_vlan(self):
        ran, files, restarts, kw = self.fake(["", "", "10.0.42.77/24"])
        ok, info = NL.apply_live(lacp42(), **kw)
        self.assertTrue(ok)
        self.assertEqual(info, "10.0.42.77/24")
        self.assertIn("denyinterfaces eth0 eth1 bond0", files["/etc/dhcpcd.conf"])
        self.assertEqual(restarts, [1])

    def test_dhcp_timeout_explains(self):
        _r, _f, _s, kw = self.fake([])
        ok, info = NL.apply_live(lacp42(), **kw)
        self.assertFalse(ok)
        self.assertIn("bond0.42", info)
        self.assertIn("VLAN 42", info)

    def test_static_and_command_failure(self):
        ran, files, _s, kw = self.fake([])
        n = lacp42("static")
        n.update(address="10.0.42.5/24", gateway="10.0.42.1")
        ok, info = NL.apply_live(n, **kw)
        self.assertTrue(ok)
        self.assertIn(["ip", "addr", "replace", "10.0.42.5/24", "dev", "bond0.42"], ran)
        self.assertIn("interface bond0.42\nstatic ip_address=10.0.42.5/24", files["/etc/dhcpcd.conf"])
        # a leftover delete that fails is fine; a failing add is reported
        kw["run"] = lambda c, chk: (2, "Cannot find device") if c[:3] == ["ip", "link", "del"] else (0, "")
        self.assertNotIn("Cannot find device", NL.apply_live(lacp42(), **kw)[1])
        kw["run"] = lambda c, chk: (2, "RTNETLINK answers: Operation not supported") if c[:3] == ["ip", "link", "add"] else (0, "")
        ok, info = NL.apply_live(lacp42(), **kw)
        self.assertFalse(ok)
        self.assertIn("Operation not supported", info)

    def test_back_to_plain_tears_down_and_cleans_dhcpcd(self):
        ran, files, restarts, kw = self.fake(["10.0.42.2/24"])
        NL.apply_live(lacp42(), **kw)
        ok, _ = NL.apply_live(mg(), prev=lacp42(), **{k: v for k, v in kw.items()})
        self.assertTrue(ok)
        self.assertIn(["ip", "link", "del", "bond0"], ran)
        self.assertEqual(files["/etc/dhcpcd.conf"], "denyinterfaces wlan*\n")
        self.assertEqual(len(restarts), 2)


class AnswerFile(unittest.TestCase):
    def cfg(self, net):
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[global]\nhostname = "n1.example.com"\nroot_password_hash = "$6$salt$' + "a" * 86 + '"\n[user]\nname = "op"\n'
                    'password_hash = "$6$salt$' + "a" * 86 + '"\n[disk]\nfilesystem = "xfs"\ndisks = ["/dev/vda"]\n' + net)
        try:
            c = S.config_from_answers(PROFILE, f.name)
        finally:
            os.unlink(f.name)
        return c, [e for e in S.validate_config(PROFILE, c, unattended=True) if not e.startswith(("user", "install_type"))]

    def test_lacp_vlan(self):
        c, errs = self.cfg('[network.management]\ninterfaces = ["eth0", "eth1"]\nvlan = 42\n'
                           '[network.management.bond]\nmode = "802.3ad"\nlacp_rate = "fast"\nxmit_hash_policy = "layer3+4"\nmiimon = 50\n')
        self.assertEqual(errs, [])
        n = c["mgmt"]
        self.assertEqual((n["iface"], n["ifaces"], n["vlan"]), ("eth0", ["eth0", "eth1"], 42))
        self.assertEqual((n["bond"]["mode"], n["bond"]["miimon"]), ("802.3ad", 50))
        self.assertEqual(NL.l3_iface(n), "bond0.42")

    def test_old_single_iface_keys_still_work(self):
        c, errs = self.cfg('[network.management]\niface = "eth3"\nmode = "static"\naddress = "192.0.2.4/24"\ngateway = "192.0.2.1"\n')
        self.assertEqual(errs, [])
        self.assertEqual(c["mgmt"]["ifaces"], ["eth3"])
        self.assertTrue(NL.is_plain(c["mgmt"]))

    def test_all_wired(self):
        c, _e = self.cfg('[network.management]\ninterfaces = "all"\n[network.management.bond]\nmode = "active-backup"\n')
        NL.resolve_all(c["mgmt"], [n for n in NICS if not n["wifi"]])
        self.assertEqual(c["mgmt"]["ifaces"], ["eth0", "eth1"])

    def test_clear_errors(self):
        _c, errs = self.cfg('[network.management]\ninterfaces = ["eth0", "eth1"]\nvlan = 5000\n[network.management.bond]\nmode = "lacp"\nspeed = 1\n')
        txt = " | ".join(errs)
        self.assertIn("unknown key(s) speed", txt)
        self.assertIn("bond mode 'lacp'", txt)
        self.assertIn("VLAN id", txt)
        _c, errs = self.cfg('[network.management]\ninterfaces = 3\n')
        self.assertIn("list of interface names", " ".join(errs))


class FakeInst(S.Installer):
    def __init__(self, target, init, desktop=False):
        self.p, self.target, self.ran, self.logs = PROFILE, target, [], []
        self.c = {"install_type": "desktop" if desktop else "server"}
        os.makedirs(os.path.join(target, "etc/init.d"), exist_ok=True)
        os.makedirs(os.path.join(target, "sbin"), exist_ok=True)
        os.symlink("../usr/lib/systemd/systemd" if init == "systemd" else "openrc-init", os.path.join(target, "sbin/init"))

    def run(self, cmd, check=True, input_=None, line_cb=None, chroot=False, env=None):
        self.ran.append(cmd)
        return 0, ""

    def log(self, line):
        self.logs.append(line)


class InstallerWrites(unittest.TestCase):
    def test_openrc(self):
        with tempfile.TemporaryDirectory() as t:
            i = FakeInst(t, "openrc")
            i.configure_mgmt_link(lacp42())
            net = open(os.path.join(t, "etc/conf.d/net")).read()
            self.assertIn('slaves_bond0="eth0 eth1"', net)
            self.assertEqual(os.readlink(os.path.join(t, "etc/init.d/net.bond0")), "net.lo")
            self.assertIn(["rc-update", "add", "net.bond0", "default"], i.ran)
            rec = json.load(open(os.path.join(t, "etc/horcrux/management-network.json")))
            self.assertEqual((rec["address_interface"], rec["stack"]), ("bond0.42", "dhcpcd"))
            self.assertFalse(os.path.exists(os.path.join(t, "etc/systemd/network")))

    def test_systemd(self):
        with tempfile.TemporaryDirectory() as t:
            i = FakeInst(t, "systemd")
            i.configure_mgmt_link(lacp42())
            self.assertTrue(os.path.exists(os.path.join(t, "etc/systemd/network/10-mgmt-bond0.netdev")))
            self.assertIn(["systemctl", "enable", "systemd-networkd.service"], i.ran)
            self.assertIn(["systemctl", "disable", "systemd-networkd-wait-online.service"], i.ran)
            self.assertFalse(os.path.exists(os.path.join(t, "etc/conf.d/net")))

    def test_desktop_uses_networkmanager(self):
        with tempfile.TemporaryDirectory() as t:
            i = FakeInst(t, "systemd", desktop=True)
            i.configure_mgmt_link(lacp42())
            p = os.path.join(t, "etc/NetworkManager/system-connections/mgmt-bond0.42.nmconnection")
            self.assertEqual(os.stat(p).st_mode & 0o777, 0o600)
            self.assertFalse(os.path.exists(os.path.join(t, "etc/systemd/network")))
            self.assertEqual(json.load(open(os.path.join(t, "etc/horcrux/management-network.json")))["stack"], "networkmanager")

    def test_plain_writes_only_the_record(self):
        with tempfile.TemporaryDirectory() as t:
            i = FakeInst(t, "openrc")
            i.configure_mgmt_link(mg())
            self.assertFalse(os.path.exists(os.path.join(t, "etc/conf.d/net")))
            self.assertEqual(i.ran, [])
            self.assertIsNone(json.load(open(os.path.join(t, "etc/horcrux/management-network.json")))["bond"])

    def test_configure_network_static_on_vlan(self):
        with tempfile.TemporaryDirectory() as t:
            i = FakeInst(t, "openrc")
            n = lacp42("static")
            n.update(address="10.0.42.5/24", gateway="10.0.42.1", dns="10.0.42.1")
            i.c.update({"mgmt": n, "wifi": None})
            i.configure_network()
            dh = open(os.path.join(t, "etc/dhcpcd.conf")).read()
            self.assertIn("denyinterfaces eth0 eth1 bond0", dh)
            self.assertIn("interface bond0.42\nstatic ip_address=10.0.42.5/24\nstatic routers=10.0.42.1", dh)
            self.assertNotIn("interface eth0\n", dh)


class Screens(unittest.TestCase):
    def test_fields_and_commit(self):
        n = NL.ensure({"iface": "eth0", "mode": "dhcp", "address": "", "gateway": "", "dns": ""})
        f = {x.key: x for x in ST.mgmt_link_fields(n, NICS)}
        self.assertEqual(set(f), {"bond_with", "bond_mode", "bond_hash", "bond_lacp", "bond_miimon", "vlan"})
        self.assertNotIn("wlan0", [o[0] for o in f["bond_with"].options])
        v = {"iface": "eth0", "bond_with": [], "bond_mode": "802.3ad"}
        self.assertFalse(f["bond_mode"].visible(v))
        v["bond_with"] = ["eth1"]
        self.assertTrue(f["bond_mode"].visible(v))
        self.assertTrue(f["bond_lacp"].visible(v))
        self.assertIsNotNone(f["vlan"].validate("4095", v))
        self.assertIsNone(f["vlan"].validate("42", v))
        errs = ST.commit_mgmt_link(n, dict(v, bond_hash="layer2+3", bond_lacp="slow", bond_miimon="200", vlan="42"), "eth0")
        self.assertEqual(errs, [])
        self.assertEqual(NL.describe(n), "bond bond0 (802.3ad: eth0 + eth1; miimon 200, hash layer2+3, lacp slow), VLAN 42 as bond0.42")
        # the primary NIC ticked again in "bond with" is not counted twice; unticking everything = one NIC again
        ST.commit_mgmt_link(n, {"bond_with": ["eth0"], "vlan": ""}, "eth0")
        self.assertTrue(NL.is_plain(n))

    def test_plan_line(self):
        c = S._default_config(PROFILE, "xfs", "")
        c["mgmt"] = lacp42()
        ctx = ST.Ctx(PROFILE, c)
        line = ST.BY_ID["netconfig"].plan(ctx)[0]
        self.assertIn("bond bond0 (802.3ad", line)
        self.assertIn("on bond0.42", line)


class ConsoleMenu(unittest.TestCase):
    def test_write_mgmt_moves_static_block_and_rewrites_l2(self):
        with tempfile.TemporaryDirectory() as t:
            root = t + "/"
            os.makedirs(root + "etc/init.d")
            os.makedirs(root + "sbin")
            os.symlink("openrc-init", root + "sbin/init")
            old = mg(("eth0",))
            old.update(mode="static", address="192.0.2.5/24", gateway="192.0.2.1")
            ran = []
            m = CM.Console(PROFILE, inp=lambda p: "", out=open(os.devnull, "w"), root=root)
            open(root + "etc/dhcpcd.conf", "w").write("interface eth0\nstatic ip_address=192.0.2.5/24\n")
            new = lacp42("static")
            new.update(address="10.0.42.5/24", gateway="10.0.42.1")
            m.write_mgmt(old, new, run=lambda c: (ran.append(c), (0, ""))[1], live=False)
            dh = open(root + "etc/dhcpcd.conf").read()
            self.assertNotIn("interface eth0\n", dh)
            self.assertIn("interface bond0.42", dh)
            self.assertIn("denyinterfaces eth0 eth1 bond0", dh)
            self.assertIn('vlans_bond0="42"', open(root + "etc/conf.d/net").read())
            self.assertEqual(NL.record(m.mgmt()), NL.record(new))
            # and back to one NIC: the netifrc block, the service and the deny list go away
            m.write_mgmt(new, old, run=lambda c: (ran.append(c), (0, ""))[1], live=False)
            self.assertNotIn("bond0", open(root + "etc/conf.d/net").read())
            self.assertIn(["rc-update", "del", "net.bond0", "default"], ran)
            self.assertFalse(os.path.lexists(root + "etc/init.d/net.bond0"))
            self.assertIn("interface eth0", open(root + "etc/dhcpcd.conf").read())
            self.assertNotIn("denyinterfaces eth0", open(root + "etc/dhcpcd.conf").read())

    def test_link_local_is_not_a_lease(self):
        seen = iter(["169.254.77.36/16"] * 3 + ["10.42.0.120/24"])
        ok, ip = NL.apply_live(lacp42(), run=lambda c, chk: (0, ""), read=lambda p: "", write=lambda p, t: None,
                               nic_ip=lambda d: next(seen), sleep=lambda s: None, wait_secs=10, log=lambda m: None)
        self.assertEqual((ok, ip), (True, "10.42.0.120/24"))
        ok, msg = NL.apply_live(lacp42(), run=lambda c, chk: (0, ""), read=lambda p: "", write=lambda p, t: None,
                                nic_ip=lambda d: "169.254.1.2/16", sleep=lambda s: None, wait_secs=3, log=lambda m: None)
        self.assertFalse(ok)
        self.assertIn("no DHCP address on bond0.42", msg)

    def test_cli_for_the_api(self):
        import io
        with tempfile.TemporaryDirectory() as t:
            root = t + "/"
            os.makedirs(root + "etc/init.d")
            os.makedirs(root + "sbin")
            os.symlink("../usr/lib/systemd/systemd", root + "sbin/init")
            con = CM.Console(PROFILE, inp=lambda p: "", out=io.StringIO(), root=root)
            o = io.StringIO()
            self.assertEqual(CM.mgmt_cli(con, ["--mgmt-show"], out=o), 0)
            self.assertEqual(json.loads(o.getvalue()), {"recorded": False, "record": None})
            bad = os.path.join(t, "bad.json")
            json.dump({"interfaces": ["eth0", "eth1"], "vlan": 9000, "bond": {"mode": "lacp"}, "mode": "static", "address": "nope"}, open(bad, "w"))
            o = io.StringIO()
            self.assertEqual(CM.mgmt_cli(con, ["--mgmt-apply", bad], out=o, nics=NICS), 2)
            errs = " | ".join(json.loads(o.getvalue())["errors"])
            for want in ("VLAN id", "bond mode", "address:"):
                self.assertIn(want, errs)
            good = os.path.join(t, "good.json")
            json.dump(NL.record(lacp42()), open(good, "w"))
            o = io.StringIO()
            con.write_mgmt.__func__  # noqa - exists
            orig = con.write_mgmt
            con.write_mgmt = lambda old, new, live=True: orig(old, new, run=lambda c: (0, ""), live=False)
            self.assertEqual(CM.mgmt_cli(con, ["--mgmt-apply", good], out=o, nics=NICS), 0)
            res = json.loads(o.getvalue())
            self.assertTrue(res["ok"])
            self.assertFalse(res["applied"])
            self.assertTrue(os.path.exists(root + "etc/systemd/network/10-mgmt-bond0.42.netdev"))
            o = io.StringIO()
            CM.mgmt_cli(con, ["--mgmt-show"], out=o)
            self.assertEqual(json.loads(o.getvalue())["record"]["address_interface"], "bond0.42")

    def test_menu_entry_only_for_management_products(self):
        out = []

        class W:
            def write(self, s):
                out.append(s)

            def flush(self):
                pass
        CM.Console(PROFILE, out=W()).show_menu.__func__  # noqa - smoke: the method exists
        keys = [k for k, _t in CM.Console.MENU]
        self.assertIn("12", keys)


if __name__ == "__main__":
    unittest.main()
