"""Bonds (LAGG/LACP), VLAN-tagged WAN/LAN and the management network chosen at install time (liveinst/netvirt.py).

Run: python3 -m unittest discover -s build/live-iso/common/tests   (Python >= 3.11)
"""
import os
import sys
import tempfile
import textwrap
import tomllib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import netvirt as NV  # noqa: E402
from liveinst import stages as ST  # noqa: E402
from liveinst import system as S  # noqa: E402

FW = {"name": "patronus", "title": "Patronus", "filesystems": ["xfs", "ext4"], "swap_mb": 0, "network_mode": "wan-lan",
      "services": [], "web_scheme": "https", "web_port": 443, "default_hostname": "fw.localdomain", "net_bonds_vlans": True}
HX = dict(FW, name="horcrux", network_mode="management", net_bonds_vlans=False)
NICS = ["enp0s2", "enp0s3", "enp0s4"]

LAGG = """
[global]
hostname = "fw1.example.com"
[disk]
filesystem = "xfs"
disks = ["/dev/vda"]
[[network.bond]]
name = "bond0"
members = ["enp0s3", "enp0s4"]
mode = "802.3ad"
hash = "layer2+3"
lacp_rate = "fast"
[network.wan]
iface = "bond0"
vlan = 100
mode = "dhcp"
[network.lan]
iface = "none"
[network.mgmt]
iface = "bond0"
vlan = 20
address = "10.20.0.1/24"
"""


def answers(text, profile=FW):
    with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
        f.write(textwrap.dedent(text))
    try:
        return S.config_from_answers(profile, f.name)
    finally:
        os.unlink(f.name)


def net_errs(c, profile=FW, nics=NICS):
    return NV.problems(profile, c, nics)


class AnswerFile(unittest.TestCase):
    def test_trunk_bond_with_wan_and_mgmt_vlans(self):
        c = answers(LAGG)
        self.assertEqual(c["bonds"], [{"name": "bond0", "members": ["enp0s3", "enp0s4"], "mode": "802.3ad", "hash": "layer2+3",
                                       "lacp_rate": "fast"}])
        self.assertEqual(NV.device(c["wan"]), "bond0.100")
        self.assertEqual(NV.device(c["mgmt_net"]), "bond0.20")
        self.assertEqual(c["lan"]["iface"], "")
        self.assertTrue(c["lan"]["_none"])
        self.assertTrue(NV.in_use(c))
        self.assertEqual(net_errs(c), [])

    def test_section_shorthand_creates_the_bond(self):
        c = answers("""
            [network.wan]
            bond = ["enp0s3", "enp0s4"]
            bond_mode = "active-backup"
            vlan = 7
            mode = "pppoe"
            pppoe_user = "me@isp"
            pppoe_password = "pw"
            [network.lan]
            iface = "enp0s2"
            address = "192.168.1.1/24"
        """)
        self.assertEqual(c["wan"]["iface"], "bond0")
        self.assertEqual(c["bonds"][0]["mode"], "active-backup")
        self.assertEqual(c["bonds"][0]["hash"], "")     # no hash / LACP for active-backup
        self.assertEqual(net_errs(c), [])
        self.assertIn("WAN bond0.7 (VLAN 7 on bond0): PPPoE as me@isp", NV.plan_lines(c))

    def test_plain_install_is_untouched(self):
        c = answers("""
            [network.wan]
            iface = "enp0s2"
            [network.lan]
            iface = "enp0s3"
            address = "192.168.1.1/24"
        """)
        self.assertFalse(NV.in_use(c))
        self.assertEqual(net_errs(c), [])


class Problems(unittest.TestCase):
    def errs(self, text, nics=NICS, profile=FW):
        return " | ".join(net_errs(answers(text, profile), profile, nics))

    def test_plain_words(self):
        self.assertIn("1 to 4094", self.errs('[network.wan]\niface = "enp0s2"\nvlan = 4095\n'))
        self.assertIn("1 to 4094", self.errs('[network.wan]\niface = "enp0s2"\nvlan = "ten"\n'))
        self.assertIn("member of bond bond0", self.errs('[network.wan]\nbond = ["enp0s3", "enp0s4"]\n[network.lan]\niface = "enp0s3"\n'))
        self.assertIn("does not exist on this machine", self.errs('[network.wan]\nbond = ["enp0s3", "eth9"]\n'))
        self.assertIn("is not offered", self.errs('[network.wan]\nbond = ["enp0s3", "enp0s4"]\nbond_mode = "lacp"\n'))
        self.assertIn("only applies to 802.3ad", self.errs('[network.wan]\nbond = ["enp0s3"]\nbond_mode = "active-backup"\nlacp_rate = "fast"\n'))
        self.assertIn("same network", self.errs('[network.wan]\niface = "enp0s2"\nvlan = 5\n[network.mgmt]\niface = "enp0s2"\nvlan = 5\naddress = "10.0.0.1/24"\n'))
        self.assertIn("user name your ISP", self.errs('[network.wan]\niface = "enp0s2"\nmode = "pppoe"\n'))
        self.assertIn("Management address", self.errs('[network.mgmt]\niface = "enp0s2"\nvlan = 3\naddress = "10.0.0.1"\n'))
        self.assertIn("overlaps the LAN", self.errs('[network.lan]\niface = "enp0s3"\naddress = "10.0.0.1/24"\n'
                                                    '[network.mgmt]\niface = "enp0s2"\nvlan = 3\naddress = "10.0.0.2/24"\n'))
        self.assertIn("not used by", self.errs('[[network.bond]]\nname = "bond1"\nmembers = ["enp0s4"]\n'))
        self.assertIn("longer than 15", self.errs('[network.wan]\niface = "enp0s31f6abc"\nvlan = 4000\n', nics=["enp0s31f6abc"]))
        self.assertIn("two bonds", self.errs('[[network.bond]]\nname = "bond0"\nmembers = ["enp0s3"]\n[[network.bond]]\nname = "bond1"\n'
                                             'members = ["enp0s3"]\n[network.wan]\niface = "bond0"\n[network.lan]\niface = "bond1"\n'))

    def test_other_products_refuse_it(self):
        self.assertIn("does not set up bonds", self.errs('[network.wan]\nbond = ["enp0s3", "enp0s4"]\n', profile=HX))
        self.assertEqual(net_errs(answers('[network.management]\niface = "enp0s2"\n', HX), HX), [])

    def test_validate_config_carries_them(self):
        c = answers('[network.wan]\niface = "enp0s2"\nvlan = 9000\n')
        self.assertTrue(any("4094" in e for e in S.validate_config(FW, c, nic_names=NICS)))


class Bootstrap(unittest.TestCase):
    SHIPPED = textwrap.dedent("""\
        [system]
        hostname = "patronus"

        [network]
        # Interface roles used on FIRST BOOT only
        wan_interface = "eth0"
        lan_interface = "eth1"

        [firewall]
        default_policy = "drop"

        [firstboot]
        lan_address = "192.168.1.1/24"
        """)

    def test_written_toml_is_what_the_first_boot_reads(self):
        c = answers(LAGG)
        t = NV.apply_bootstrap(self.SHIPPED, c)
        d = tomllib.loads(t)        # must stay valid TOML
        n = d["network"]
        self.assertEqual(n["wan_interface"], "bond0.100")
        self.assertNotIn("lan_interface", n)
        self.assertEqual(n["mgmt_interface"], "bond0.20")
        self.assertEqual(n["bond"], [{"name": "bond0", "members": ["enp0s3", "enp0s4"], "mode": "802.3ad",
                                      "xmit_hash_policy": "layer2+3", "lacp_rate": "fast"}])
        self.assertEqual(sorted((v["name"], v["parent"], v["tag"]) for v in n["vlan"]),
                         [("bond0.100", "bond0", 100), ("bond0.20", "bond0", 20)])
        self.assertEqual(d["firstboot"], {"wan_mode": "dhcp", "mgmt_address": "10.20.0.1/24", "mgmt_only": True})
        self.assertEqual(d["firewall"], {"default_policy": "drop"})
        # idempotent: writing it again (re-run of the stage) gives the same file
        self.assertEqual(NV.apply_bootstrap(t, c), t)

    def test_static_wan_and_pppoe_keys(self):
        c = answers('[network.wan]\niface = "enp0s2"\nvlan = 35\nmode = "static"\naddress = "198.51.100.2/24"\n'
                    'gateway = "198.51.100.1"\ndns = ["9.9.9.9", "1.1.1.1"]\n[network.lan]\niface = "enp0s3"\naddress = "192.168.1.1/24"\n')
        d = tomllib.loads(NV.apply_bootstrap(self.SHIPPED, c))
        self.assertEqual(d["firstboot"], {"lan_address": "192.168.1.1/24", "wan_mode": "static", "wan_address": "198.51.100.2/24",
                                          "wan_gateway": "198.51.100.1", "wan_dns": ["9.9.9.9", "1.1.1.1"]})
        self.assertEqual(d["network"]["wan_interface"], "enp0s2.35")
        c = answers('[network.wan]\niface = "enp0s2"\nmode = "pppoe"\npppoe_user = "a\\"b"\npppoe_password = "p\\\\w"\n')
        d = tomllib.loads(NV.apply_bootstrap(self.SHIPPED, c))
        self.assertEqual((d["firstboot"]["pppoe_username"], d["firstboot"]["pppoe_password"]), ('a"b', "p\\w"))


class Plan(unittest.TestCase):
    def test_review_page_and_plan_show_the_network(self):
        c = answers(LAGG)
        c["disks"] = ["/dev/vda"]
        ctx = ST.Ctx(FW, c, nics=[], disks=[], efi=False, ram_mb=4096)
        lines = dict(ST.full_plan(ctx))["netconfig"]
        self.assertIn("Bond bond0 = enp0s3 + enp0s4 (802.3ad, hash layer2+3, LACP fast)", lines)
        self.assertIn("WAN bond0.100 (VLAN 100 on bond0): DHCP", lines)
        self.assertIn("Management bond0.20 (VLAN 20 on bond0): 10.20.0.1/24 (SSH + web UI only here)", lines)


class Screen(unittest.TestCase):
    """The optional 'Bonds, VLANs and management network' screen (GUI form page and text form share this definition)."""
    def ctx(self):
        c = S._default_config(FW, "xfs", "") if hasattr(S, "_default_config") else answers("")
        NV.defaults(c)
        c["wan"]["iface"], c["lan"]["iface"] = "enp0s2", "enp0s3"
        nics = [{"name": n, "mac": "", "wifi": False, "driver": "virtio_net", "model": "Virtio"} for n in NICS]
        return ST.Ctx(FW, c, nics=nics, disks=[], efi=False, ram_mb=4096)

    def values(self, ctx, **kw):
        v = ST.form_values(ST.BY_ID["netadv"].fields(ctx))
        v.update(kw)
        return v

    def test_off_by_default_and_hidden_for_other_products(self):
        ctx = self.ctx()
        st = ST.BY_ID["netadv"]
        self.assertTrue(st.applies(ctx))
        self.assertFalse(st.applies(ST.Ctx(HX, ctx.c, efi=False, ram_mb=4096)))
        v = self.values(ctx)
        self.assertFalse(v["use"])
        self.assertIsNone(st.validate(ctx, v))
        st.commit(ctx, v)
        self.assertFalse(NV.in_use(ctx.c))

    def test_lacp_trunk_from_the_screen(self):
        ctx = self.ctx()
        st = ST.BY_ID["netadv"]
        v = self.values(ctx, use=True, bond_members=["enp0s3", "enp0s4"], bond_mode="802.3ad", wan_on="port", wan_vlan="0",
                        lan_on="bond", lan_vlan="30", mgmt_on="bond", mgmt_vlan="20", mgmt_addr="10.20.0.1/24", mgmt_only=True)
        self.assertIsNone(st.validate(ctx, v))
        st.commit(ctx, v)
        c = ctx.c
        self.assertEqual((c["bonds"][0]["members"], NV.device(c["lan"]), NV.device(c["mgmt_net"]), c["wan"]["iface"]),
                         (["enp0s3", "enp0s4"], "bond0.30", "bond0.20", "enp0s2"))
        self.assertEqual(net_errs(c), [])
        # untick: back to the plain ports
        st.commit(ctx, self.values(ctx, use=False))
        self.assertEqual((c["bonds"], c["lan"]["iface"], c["lan"]["vlan"]), ([], "enp0s3", 0))

    def test_screen_errors_are_plain_words(self):
        ctx = self.ctx()
        st = ST.BY_ID["netadv"]
        self.assertIn("at least two", st.validate(ctx, self.values(ctx, use=True, bond_members=["enp0s3"])))
        self.assertIn("Nothing to set up", st.validate(ctx, self.values(ctx, use=True)))
        self.assertIn("member of bond", st.validate(ctx, self.values(ctx, use=True, bond_members=["enp0s2", "enp0s4"], wan_on="port")))


if __name__ == "__main__":
    unittest.main()
