"""Management network link layout: one NIC, a bond (link aggregation, incl. 802.3ad LACP) and/or a tagged VLAN.

The installer's management section (`c["mgmt"]`) carries, next to the address keys (mode/address/gateway/dns):

    iface    the first (or only) NIC                       e.g. "eth0"
    ifaces   every NIC of the management link            e.g. ["eth0", "eth1"]; more than one = a bond
    bond     {"name", "mode", "miimon", "xmit_hash_policy", "lacp_rate", "primary"}   (used when len(ifaces) > 1)
    vlan     802.1Q VLAN id 1-4094 on top of the NIC/bond, 0 = untagged

Layer 2 (bond, VLAN) is written for the init system's own network stack: netifrc (/etc/conf.d/net, net.<parent> in the default
runlevel) on OpenRC, systemd-networkd (.netdev/.network in /etc/systemd/network) on systemd, NetworkManager keyfiles on desktop installs.
Layer 3 stays where it always was: dhcpcd (both inits; DHCP, or the static block in /etc/dhcpcd.conf) on the one interface that carries
the address (`l3_iface`), and dhcpcd is told to leave the bond members / VLAN parent alone (`denyinterfaces`). So the console menu, the
DHCP/static switch and DNS (resolvconf) behave the same with or without a bond.

The live environment applies the same layout with iproute2 (`live_commands`) before the install, so DHCP on a tagged-only network works
while installing. Everything here is pure (no root, no files) except `apply_live` and `write_target`; tests/test_netlayout.py covers it.
"""
import json
import re

BOND_MODES = ("active-backup", "802.3ad", "balance-alb", "balance-tlb", "balance-xor", "balance-rr", "broadcast")
BOND_MODE_TEXT = {
    "active-backup": "active-backup (failover, works with any switch)",
    "802.3ad": "802.3ad / LACP (switch must have an LACP group)",
    "balance-alb": "balance-alb (adaptive load balancing, no switch setup)",
    "balance-tlb": "balance-tlb (transmit load balancing)",
    "balance-xor": "balance-xor (static aggregation, switch group)",
    "balance-rr": "balance-rr (round robin, switch group)",
    "broadcast": "broadcast (every frame on every NIC)",
}
HASH_POLICIES = ("layer2", "layer2+3", "layer3+4", "encap2+3", "encap3+4", "vlan+srcmac")
HASH_MODES = ("802.3ad", "balance-xor", "balance-tlb")          # modes whose transmit hash policy matters
LACP_RATES = ("slow", "fast")
PRIMARY_MODES = ("active-backup", "balance-alb", "balance-tlb")
# documented defaults (installer screens, answer file, console menu); the profile may override every one (profile key in brackets)
DEFAULTS = {
    "bond_name": "bond0",            # [mgmt_bond_name]
    "bond_mode": "active-backup",    # [mgmt_bond_mode]   works on any switch; LACP needs switch configuration
    "miimon": 100,                   # [mgmt_bond_miimon] link check interval in ms
    "xmit_hash_policy": "layer3+4",  # [mgmt_bond_xmit_hash_policy]
    "lacp_rate": "fast",             # [mgmt_bond_lacp_rate]
    "live_wait_secs": 40,            # [mgmt_live_wait_secs] how long the live system waits for a DHCP address on the new link
    "record": "/etc/{name}/management-network.json",   # [mgmt_network_record] what the installed product reads (API, declarative export)
}
MIIMON_MAX = 60000
IFNAMSIZ = 15                         # kernel limit for interface names (IFNAMSIZ - 1)
NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]*$")
MARK_BEGIN = "# BEGIN management link (written by the installer / console menu; change it there)"
MARK_END = "# END management link"
NETWORKD_PREFIX = "10-mgmt-"
NM_PREFIX = "mgmt-"


def default(profile, key):
    return (profile or {}).get({"bond_name": "mgmt_bond_name", "bond_mode": "mgmt_bond_mode", "miimon": "mgmt_bond_miimon",
                                "xmit_hash_policy": "mgmt_bond_xmit_hash_policy", "lacp_rate": "mgmt_bond_lacp_rate",
                                "live_wait_secs": "mgmt_live_wait_secs", "record": "mgmt_network_record"}[key], DEFAULTS[key])


def default_bond(profile=None):
    return {"name": default(profile, "bond_name"), "mode": default(profile, "bond_mode"), "miimon": int(default(profile, "miimon")),
            "xmit_hash_policy": default(profile, "xmit_hash_policy"), "lacp_rate": default(profile, "lacp_rate"), "primary": ""}


def ensure(n, profile=None):
    """fill the link keys of a management section in place (old configs / machine profiles only carry iface + address keys)"""
    n.setdefault("iface", "")
    if not isinstance(n.get("ifaces"), list) or not n.get("ifaces"):
        n["ifaces"] = [n["iface"]] if n.get("iface") else []
    b = default_bond(profile)
    b.update({k: v for k, v in (n.get("bond") or {}).items() if v not in (None, "")})
    n["bond"] = b
    n["vlan"] = vlan_int(n.get("vlan"))
    if n["ifaces"] and n["iface"] != n["ifaces"][0]:
        n["iface"] = n["ifaces"][0]
    return n


def vlan_int(v):
    if v in (None, "", False):
        return 0
    try:
        return int(str(v).strip())
    except ValueError:
        return -1           # reported by problems()


def is_bond(n):
    return len(n.get("ifaces") or []) > 1


def link_parent(n):
    """the interface the VLAN sits on (or that carries the address when untagged): the bond, else the single NIC"""
    return n["bond"]["name"] if is_bond(n) else (n.get("ifaces") or [n.get("iface", "")])[0]


def vlan_name(n):
    p = link_parent(n)
    name = f"{p}.{n['vlan']}"
    return name if len(name) <= IFNAMSIZ else f"vlan{n['vlan']}"


def l3_iface(n):
    """the interface that carries the management address (dhcpcd runs DHCP / the static block here)"""
    if not n.get("ifaces") and not n.get("iface"):
        return ""
    return vlan_name(n) if n.get("vlan", 0) > 0 else link_parent(n)


def is_plain(n):
    """one NIC, no VLAN: nothing to write for layer 2 (the pre-bond installer behaviour)"""
    return not is_bond(n) and not n.get("vlan", 0) > 0


def dhcpcd_denied(n):
    """interfaces dhcpcd must not touch: bond members, and the VLAN parent (untagged DHCP there would be wrong)"""
    if is_plain(n):
        return []
    out = list(n["ifaces"]) if is_bond(n) else []
    if n.get("vlan", 0) > 0:
        out.append(link_parent(n))
    return out


def name_problem(name, what):
    if not name:
        return f"{what}: empty interface name"
    if len(name) > IFNAMSIZ:
        return f"{what}: interface name {name!r} is longer than {IFNAMSIZ} characters"
    if not NAME_RE.match(name) or name in (".", ".."):
        return f"{what}: {name!r} is not a valid interface name"
    return None


def problems(n, nics=None):
    """everything wrong with the link part of a management section; nics = the machine's NIC list (list_nics) to check names against"""
    errs = []
    ifaces = n.get("ifaces") or ([n["iface"]] if n.get("iface") else [])
    if not ifaces:
        return errs           # no NIC: installs without networking (warned by the screen)
    for x in ifaces:
        e = name_problem(x, "management interface")
        if e:
            errs.append(e)
    if len(set(ifaces)) != len(ifaces):
        errs.append("management interfaces: the same NIC is listed twice")
    if nics is not None:
        known = {x["name"]: x for x in nics}
        for x in ifaces:
            if x not in known:
                errs.append(f"management interface {x!r} does not exist on this machine (found: {', '.join(sorted(known)) or 'none'})")
            elif known[x].get("wifi") and (len(ifaces) > 1 or vlan_int(n.get("vlan")) > 0):
                errs.append(f"{x} is Wi-Fi: Wi-Fi cannot be bonded or carry a VLAN tag; use wired NICs")
    v = vlan_int(n.get("vlan"))
    if v < 0 or v > 4094:
        errs.append(f"vlan: {n.get('vlan')!r} is not a VLAN id (1-4094, empty or 0 = untagged)")
    if len(ifaces) > 1:
        b = n.get("bond") or {}
        e = name_problem(b.get("name", ""), "bond name")
        if e:
            errs.append(e)
        elif b.get("name") in ifaces:
            errs.append(f"bond name {b['name']!r} is one of its own member NICs")
        if b.get("mode") not in BOND_MODES:
            errs.append(f"bond mode {b.get('mode')!r}: use one of {', '.join(BOND_MODES)}")
        try:
            mi = int(b.get("miimon", 0))
            if not 1 <= mi <= MIIMON_MAX:
                errs.append(f"bond miimon {mi}: link check interval in ms, 1-{MIIMON_MAX}")
        except (TypeError, ValueError):
            errs.append(f"bond miimon {b.get('miimon')!r} is not a number (ms)")
        if b.get("mode") in HASH_MODES and b.get("xmit_hash_policy") not in HASH_POLICIES:
            errs.append(f"bond xmit_hash_policy {b.get('xmit_hash_policy')!r}: use one of {', '.join(HASH_POLICIES)}")
        if b.get("mode") == "802.3ad" and b.get("lacp_rate") not in LACP_RATES:
            errs.append(f"bond lacp_rate {b.get('lacp_rate')!r}: use slow or fast")
        if b.get("primary"):
            if b["primary"] not in ifaces:
                errs.append(f"bond primary {b['primary']!r} is not one of its members ({', '.join(ifaces)})")
            elif b.get("mode") not in PRIMARY_MODES:
                errs.append(f"bond primary only applies to {', '.join(PRIMARY_MODES)}")
    return errs


def describe(n):
    """one line for summaries and logs"""
    if not (n.get("ifaces") or n.get("iface")):
        return "-"
    parts = []
    if is_bond(n):
        b = n["bond"]
        extra = [f"miimon {b['miimon']}"]
        if b["mode"] in HASH_MODES:
            extra.append(f"hash {b['xmit_hash_policy']}")
        if b["mode"] == "802.3ad":
            extra.append(f"lacp {b['lacp_rate']}")
        parts.append(f"bond {b['name']} ({b['mode']}: {' + '.join(n['ifaces'])}; {', '.join(extra)})")
    else:
        parts.append(n["ifaces"][0] if n.get("ifaces") else n["iface"])
    if n.get("vlan", 0) > 0:
        parts.append(f"VLAN {n['vlan']} as {vlan_name(n)}")
    return ", ".join(parts)


# --------------------------------------------------------------------------
# answer file
# --------------------------------------------------------------------------
def from_answers(src, n, profile=None):
    """[network.management] interfaces / bond / vlan keys -> management section (in place); unknown bond keys are errors"""
    errs = []
    if "interfaces" in src:
        v = src["interfaces"]
        if isinstance(v, str):
            v = [v] if v != "all" else ["all"]
        if not isinstance(v, list) or not all(isinstance(x, str) for x in v):
            errs.append("network.management.interfaces: a list of interface names (or \"all\" = every wired NIC)")
        else:
            n["ifaces"] = list(v)
            n["iface"] = v[0] if v and v != ["all"] else ("auto" if v == ["all"] else n.get("iface", ""))
    bond = src.get("bond")
    if bond is not None:
        if not isinstance(bond, dict):
            errs.append("network.management.bond: a table (mode, miimon, xmit_hash_policy, lacp_rate, name, primary)")
        else:
            bad = [k for k in bond if k not in ("name", "mode", "miimon", "xmit_hash_policy", "lacp_rate", "primary")]
            if bad:
                errs.append("network.management.bond: unknown key(s) " + ", ".join(bad))
            b = default_bond(profile)
            b.update(bond)
            n["bond"] = b
    if "vlan" in src:
        n["vlan"] = src["vlan"]
    return errs


def resolve_all(n, wired):
    """interfaces = ["all"] -> every wired NIC (sorted by name, carrier first)"""
    if n.get("ifaces") == ["all"]:
        n["ifaces"] = [x["name"] for x in wired]
        n["iface"] = n["ifaces"][0] if n["ifaces"] else ""
    return n


# --------------------------------------------------------------------------
# renderers (pure)
# --------------------------------------------------------------------------
def ifvar(name):
    """netifrc variable suffix of an interface name (every non-alphanumeric character -> _)"""
    return re.sub(r"[^A-Za-z0-9]", "_", name)


def render_netifrc(n):
    """the managed block for /etc/conf.d/net (empty for a plain single NIC)"""
    if is_plain(n):
        return ""
    lines = [MARK_BEGIN, "# layer 2 only: dhcpcd configures the address of " + l3_iface(n)]
    parent = link_parent(n)
    if is_bond(n):
        b = n["bond"]
        bv = ifvar(b["name"])
        for m in n["ifaces"]:
            lines.append(f'config_{ifvar(m)}="null"')
        lines.append(f'slaves_{bv}="{" ".join(n["ifaces"])}"')
        lines.append(f'mode_{bv}="{b["mode"]}"')
        lines.append(f'miimon_{bv}="{int(b["miimon"])}"')
        if b["mode"] in HASH_MODES:
            lines.append(f'xmit_hash_policy_{bv}="{b["xmit_hash_policy"]}"')
        if b["mode"] == "802.3ad":
            lines.append(f'lacp_rate_{bv}="{b["lacp_rate"]}"')
        if b.get("primary"):
            lines.append(f'primary_{bv}="{b["primary"]}"')
    lines.append(f'config_{ifvar(parent)}="null"')
    if n.get("vlan", 0) > 0:
        vid = n["vlan"]
        lines.append(f'vlans_{ifvar(parent)}="{vid}"')
        lines.append(f'{ifvar(parent)}_vlan{vid}_name="{vlan_name(n)}"')
        lines.append(f'config_{ifvar(vlan_name(n))}="null"')
    lines.append(MARK_END)
    return "\n".join(lines) + "\n"


def netifrc_services(n):
    """net.<x> services to add to the default runlevel (symlinks to net.lo); the VLAN starts with its parent"""
    return [] if is_plain(n) else ["net." + link_parent(n)]


def strip_block(text, begin=MARK_BEGIN, end=MARK_END):
    out, skip = [], False
    for line in text.splitlines():
        if line == begin:
            skip = True
            continue
        if skip:
            if line == end:
                skip = False
            continue
        out.append(line)
    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out) + ("\n" if out else "")


def apply_netifrc(text, n):
    """/etc/conf.d/net text with our block replaced (other config is kept)"""
    base = strip_block(text)
    blk = render_netifrc(n)
    return (base + ("\n" if base else "") + blk) if blk else base


def render_networkd(n):
    """{file name: content} for /etc/systemd/network (empty for a plain single NIC); the address interface stays unmanaged by
    networkd (dhcpcd owns layer 3), so networkd never drops dhcpcd's addresses"""
    if is_plain(n):
        return {}
    files = {}
    head = "# written by the installer / console menu (management link); dhcpcd configures the address\n"
    parent = link_parent(n)
    vid = n.get("vlan", 0)
    if is_bond(n):
        b = n["bond"]
        bond = [head, "[NetDev]", f"Name={b['name']}", "Kind=bond", "", "[Bond]", f"Mode={b['mode']}", f"MIIMonitorSec={int(b['miimon'])}ms"]
        if b["mode"] in HASH_MODES:
            bond.append(f"TransmitHashPolicy={b['xmit_hash_policy']}")
        if b["mode"] == "802.3ad":
            bond.append(f"LACPTransmitRate={b['lacp_rate']}")
        files[f"{NETWORKD_PREFIX}{b['name']}.netdev"] = "\n".join(bond) + "\n"
        for m in n["ifaces"]:
            mem = [head, "[Match]", f"Name={m}", "", "[Network]", f"Bond={b['name']}"]
            if b.get("primary") == m:
                mem.append("PrimarySlave=yes")
            mem += ["", "[Link]", "RequiredForOnline=no"]
            files[f"{NETWORKD_PREFIX}{m}.network"] = "\n".join(mem) + "\n"
    if vid > 0:
        vn = vlan_name(n)
        files[f"{NETWORKD_PREFIX}{vn}.netdev"] = "\n".join([head, "[NetDev]", f"Name={vn}", "Kind=vlan", "", "[VLAN]", f"Id={vid}"]) + "\n"
        par = [head, "[Match]", f"Name={parent}", "", "[Network]", f"VLAN={vn}", "LinkLocalAddressing=no", "IPv6AcceptRA=no", "",
               "[Link]", "RequiredForOnline=no"]
        key = f"{NETWORKD_PREFIX}{parent}.network"
        files[key] = "\n".join(par) + "\n"
    return files


def render_dhcpcd(n):
    """managed dhcpcd.conf block: deny the layer-2-only interfaces (members, VLAN parent)"""
    deny = dhcpcd_denied(n)
    if not deny:
        return ""
    return "\n".join([MARK_BEGIN, "# bond members / VLAN parent carry no address of their own", "denyinterfaces " + " ".join(deny), MARK_END]) + "\n"


def apply_dhcpcd(text, n):
    base = strip_block(text)
    blk = render_dhcpcd(n)
    return (base + ("\n" if base else "") + blk) if blk else base


def _nm_ip(n, method_on):
    if not method_on:
        return ["[ipv4]", "method=disabled", "", "[ipv6]", "method=ignore"]
    if n.get("mode") == "static":
        out = ["[ipv4]", "method=manual", f"address1={n['address']}" + (f",{n['gateway']}" if n.get("gateway") else "")]
        if n.get("dns"):
            out.append("dns=" + ";".join(re.split(r"[\s,]+", n["dns"].strip())) + ";")
        return out + ["", "[ipv6]", "method=auto"]
    return ["[ipv4]", "method=auto", "", "[ipv6]", "method=auto"]


def render_nm(n):
    """{file name: keyfile} for /etc/NetworkManager/system-connections (desktop installs: NetworkManager owns the whole network,
    layer 3 included); empty for a plain single NIC"""
    if is_plain(n):
        return {}
    files = {}
    vid = n.get("vlan", 0)
    parent = link_parent(n)
    if is_bond(n):
        b = n["bond"]
        opts = [f"mode={b['mode']}", f"miimon={int(b['miimon'])}"]
        if b["mode"] in HASH_MODES:
            opts.append(f"xmit_hash_policy={b['xmit_hash_policy']}")
        if b["mode"] == "802.3ad":
            opts.append(f"lacp_rate={b['lacp_rate']}")
        if b.get("primary"):
            opts.append(f"primary={b['primary']}")
        files[f"{NM_PREFIX}{b['name']}.nmconnection"] = "\n".join(
            ["[connection]", f"id={NM_PREFIX}{b['name']}", "type=bond", f"interface-name={b['name']}", "autoconnect=true", "",
             "[bond]"] + opts + [""] + _nm_ip(n, vid <= 0)) + "\n"
        for m in n["ifaces"]:
            files[f"{NM_PREFIX}{m}.nmconnection"] = "\n".join(
                ["[connection]", f"id={NM_PREFIX}{m}", "type=ethernet", f"interface-name={m}", f"controller={b['name']}",
                 "port-type=bond", f"master={b['name']}", "slave-type=bond", "autoconnect=true"]) + "\n"
    elif vid > 0:
        files[f"{NM_PREFIX}{parent}.nmconnection"] = "\n".join(
            ["[connection]", f"id={NM_PREFIX}{parent}", "type=ethernet", f"interface-name={parent}", "autoconnect=true", ""]
            + _nm_ip(n, False)) + "\n"
    if vid > 0:
        vn = vlan_name(n)
        files[f"{NM_PREFIX}{vn}.nmconnection"] = "\n".join(
            ["[connection]", f"id={NM_PREFIX}{vn}", "type=vlan", f"interface-name={vn}", "autoconnect=true", "",
             "[vlan]", f"parent={parent}", f"id={vid}", ""] + _nm_ip(n, True)) + "\n"
    return files


def record(n):
    """what the installed product reads (horcrux-api GET /api/network/management, declarative export): no secrets in here"""
    out = {"version": 1, "interfaces": list(n.get("ifaces") or ([n["iface"]] if n.get("iface") else [])),
           "vlan": n.get("vlan", 0) if n.get("vlan", 0) > 0 else None,
           "address_interface": l3_iface(n), "mode": n.get("mode", "dhcp")}
    if is_bond(n):
        b = n["bond"]
        out["bond"] = {"name": b["name"], "mode": b["mode"], "miimon": int(b["miimon"])}
        if b["mode"] in HASH_MODES:
            out["bond"]["xmit_hash_policy"] = b["xmit_hash_policy"]
        if b["mode"] == "802.3ad":
            out["bond"]["lacp_rate"] = b["lacp_rate"]
        if b.get("primary"):
            out["bond"]["primary"] = b["primary"]
    else:
        out["bond"] = None
    if n.get("mode") == "static":
        out.update({"address": n.get("address", ""), "gateway": n.get("gateway", "") or None,
                    "dns": re.split(r"[\s,]+", n["dns"].strip()) if n.get("dns") else []})
    return out


def from_record(rec, profile=None):
    """record() -> management section (console menu edits the installed layout from it)"""
    n = {"iface": "", "ifaces": list(rec.get("interfaces") or []), "mode": rec.get("mode", "dhcp"), "address": rec.get("address", ""),
         "gateway": rec.get("gateway") or "", "dns": " ".join(rec.get("dns") or []), "vlan": rec.get("vlan") or 0,
         "bond": rec.get("bond") or {}}
    return ensure(n, profile)


# --------------------------------------------------------------------------
# live system (iproute2)
# --------------------------------------------------------------------------
def live_commands(n):
    """argv list that builds the layout on the running kernel (members flushed and downed first, as the kernel wants)"""
    if is_plain(n):
        return []
    cmds = []
    vid = n.get("vlan", 0)
    if is_bond(n):
        b = n["bond"]
        cmds.append(["modprobe", "bonding"])
        opts = ["mode", b["mode"], "miimon", str(int(b["miimon"]))]
        if b["mode"] in HASH_MODES:
            opts += ["xmit_hash_policy", b["xmit_hash_policy"]]
        if b["mode"] == "802.3ad":
            opts += ["lacp_rate", b["lacp_rate"]]
        cmds.append(["ip", "link", "add", b["name"], "type", "bond"] + opts)
        for m in n["ifaces"]:
            cmds += [["ip", "addr", "flush", "dev", m], ["ip", "link", "set", m, "down"], ["ip", "link", "set", m, "master", b["name"]]]
        if b.get("primary"):
            cmds.append(["ip", "link", "set", b["name"], "type", "bond", "primary", b["primary"]])
        for m in n["ifaces"]:
            cmds.append(["ip", "link", "set", m, "up"])
    else:
        cmds.append(["ip", "addr", "flush", "dev", link_parent(n)])
    cmds.append(["ip", "link", "set", link_parent(n), "up"])
    if vid > 0:
        cmds.append(["modprobe", "8021q"])
        cmds.append(["ip", "link", "add", "link", link_parent(n), "name", vlan_name(n), "type", "vlan", "id", str(vid)])
        cmds.append(["ip", "link", "set", vlan_name(n), "up"])
    return cmds


def live_teardown(n):
    """undo live_commands (Back on the screen, or a second try with other settings); errors are ignored by the caller"""
    if is_plain(n):
        return []
    cmds = []
    if n.get("vlan", 0) > 0:
        cmds.append(["ip", "link", "del", vlan_name(n)])
    if is_bond(n):
        for m in n["ifaces"]:
            cmds.append(["ip", "link", "set", m, "nomaster"])
        cmds.append(["ip", "link", "del", n["bond"]["name"]])
        for m in n["ifaces"]:
            cmds.append(["ip", "link", "set", m, "up"])
    return cmds


def dhcpcd_static_block(n):
    """the static dhcpcd stanza for the address interface (same text system.configure_network writes)"""
    if n.get("mode") != "static":
        return []
    out = ["", f"interface {l3_iface(n)}", f"static ip_address={n['address']}"]
    if n.get("gateway"):
        out.append(f"static routers={n['gateway']}")
    if n.get("dns"):
        out.append("static domain_name_servers=" + " ".join(re.split(r"[\s,]+", n["dns"].strip())))
    return out


def apply_live(n, run, read=None, write=None, restart_dhcpcd=None, nic_ip=None, sleep=None, wait_secs=40, prev=None, log=print):
    """build the layout on the live system and get the address on it. All side effects are injected (tests use fakes).
    -> (ok, address or error text)"""
    import time as _t
    sleep = sleep or _t.sleep
    if prev is not None:
        for c in live_teardown(prev):
            run(c, False)
    if is_plain(n):
        if prev is not None and not is_plain(prev) and read and write:
            write("/etc/dhcpcd.conf", strip_block(read("/etc/dhcpcd.conf")))
            if restart_dhcpcd:
                restart_dhcpcd()
        return True, ""
    for c in live_commands(n):
        rc, out = run(c, True)
        if rc != 0:
            return False, f"{' '.join(c)}: {out.strip()[-160:]}"
    if read and write:
        path = "/etc/dhcpcd.conf"
        txt = strip_block(read(path))
        blk = render_dhcpcd(n)
        st = dhcpcd_static_block(n)
        if st:
            blk = blk + "\n".join([MARK_BEGIN] + st[1:] + [MARK_END]) + "\n"
        write(path, txt + ("\n" if txt else "") + blk)
    if restart_dhcpcd:
        restart_dhcpcd()
    l3 = l3_iface(n)
    if n.get("mode") == "static":
        run(["ip", "addr", "replace", n["address"], "dev", l3], False)
        if n.get("gateway"):
            run(["ip", "route", "replace", "default", "via", n["gateway"], "dev", l3], False)
        return True, n["address"]
    if nic_ip is None:
        return True, ""
    for _ in range(int(wait_secs)):
        ip = nic_ip(l3)
        if ip:
            return True, ip
        sleep(1)
    return False, f"no DHCP address on {l3} after {int(wait_secs)} s (is VLAN {n.get('vlan') or 'untagged'} served by a DHCP server " \
                  "on this switch port?)"


def _write(root, rel, text, mode=0o644):
    import os
    path = os.path.join(root, rel.lstrip("/"))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    os.chmod(path, mode)


def write_target(root, n, init, run, record_path, nm=False, old=None):
    """write the layer-2 config of the management link into the system at `root` for its init system (`run(argv)` runs a command
    inside it), drop the files of an `old` layout that are no longer used, and write the record. -> log lines"""
    import os
    log = []
    _write(root, record_path, record_json(n, "networkmanager" if nm else "dhcpcd"))
    if old is not None:
        stale = set(render_networkd(old)) - set(render_networkd(n))
        for fn in stale:
            p = os.path.join(root, "etc/systemd/network", fn)
            if os.path.exists(p):
                os.remove(p)
        for fn in set(render_nm(old)) - set(render_nm(n)):
            p = os.path.join(root, "etc/NetworkManager/system-connections", fn)
            if os.path.exists(p):
                os.remove(p)
        if init == "openrc" and not nm:
            for svc in set(netifrc_services(old)) - set(netifrc_services(n)):
                run(["rc-update", "del", svc, "default"])
                p = os.path.join(root, "etc/init.d", svc)
                if os.path.islink(p):
                    os.remove(p)
    if nm:
        for fn, txt in render_nm(n).items():
            _write(root, "/etc/NetworkManager/system-connections/" + fn, txt, 0o600)
        log.append("NetworkManager keyfiles: " + (", ".join(sorted(render_nm(n))) or "none"))
        return log
    if init == "systemd":
        files = render_networkd(n)
        for fn, txt in files.items():
            _write(root, "/etc/systemd/network/" + fn, txt)
        if files:
            run(["systemctl", "enable", "systemd-networkd.service"])
            # dhcpcd decides when the address is up; networkd's wait-online would wait for links networkd does not configure
            run(["systemctl", "disable", "systemd-networkd-wait-online.service"])
        log.append("systemd-networkd: " + (", ".join(sorted(files)) or "nothing (one untagged NIC)"))
    else:
        path = os.path.join(root, "etc/conf.d/net")
        txt = open(path).read() if os.path.exists(path) else ""
        new = apply_netifrc(txt, n)
        if new != txt:
            _write(root, "/etc/conf.d/net", new)
        for svc in netifrc_services(n):
            p = os.path.join(root, "etc/init.d", svc)
            if not os.path.lexists(p):
                os.symlink("net.lo", p)
            run(["rc-update", "add", svc, "default"])
        log.append("netifrc: " + (", ".join(netifrc_services(n)) or "nothing (one untagged NIC)"))
    return log


def record_json(n, stack="dhcpcd"):
    """stack: who configures the address (dhcpcd with netifrc/networkd for layer 2, or NetworkManager on desktop installs)"""
    return json.dumps(dict(record(n), stack=stack), indent=2, sort_keys=True) + "\n"
