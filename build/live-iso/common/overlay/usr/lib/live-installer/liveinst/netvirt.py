"""Bonds (LAGG / LACP), VLAN-tagged WAN / LAN and a dedicated management network, chosen at install time.

Only for products whose profile sets `net_bonds_vlans = true` (Patronus, network_mode "wan-lan"); the box must be reachable on
the right network at its FIRST boot, with nobody at the console. The installer does not configure these devices itself: it writes
them into the product's first-boot bootstrap file (`bootstrap_path`, Patronus /etc/patronus/patronus.toml: `[network]` roles,
`[[network.bond]]`, `[[network.vlan]]`, `[firstboot]` addressing) and the product's own network stack creates them on the first
boot (Patronus: `patronus apply` seeds its config store from that file once, the interfaces applier creates bond + VLANs).

Config model (system.default_config + this module):
    c["bonds"]    = [{"name", "members": [...], "mode", "hash", "lacp_rate"}]
    c["wan"]      += {"vlan": 0, "pppoe_user", "pppoe_password", "pppoe_service"}   mode: dhcp | static | pppoe
    c["lan"]      += {"vlan": 0}
    c["mgmt_net"] = {"iface", "vlan", "mode": static | dhcp, "address", "gateway", "only": True}

A section's device is `iface` (a NIC or a bond name) plus `.VLAN` when tagged. Answer file:

    [[network.bond]]                  # explicit bond, shared by several sections (trunk)
    name = "bond0"
    members = ["enp1s0", "enp2s0"]
    mode = "802.3ad"                  # 802.3ad (LACP) | active-backup | balance-xor
    hash = "layer2+3"                 # layer2 | layer2+3 | layer3+4 (802.3ad / balance-xor)
    lacp_rate = "fast"                # slow | fast (802.3ad)

    [network.wan]
    iface = "bond0"                   # or: bond = ["enp1s0", "enp2s0"] (+ bond_mode, bond_hash, lacp_rate, bond_name)
    vlan = 100                        # 1-4094; 0 / absent = untagged
    mode = "dhcp"                     # dhcp | static | pppoe (pppoe_user, pppoe_password, pppoe_service)

    [network.mgmt]                    # optional management network: anti-lockout (SSH + web UI) lives here
    iface = "bond0"
    vlan = 20
    address = "10.20.0.1/24"          # mode static (default) | dhcp
    only = true                       # SSH + web UI answer ONLY here (default true)

Everything here is pure (no I/O) so the plan, the forms and the tests share it.
"""
import ipaddress
import re

BOND_MODES = [("802.3ad", "LACP (802.3ad): needs a switch with LACP on these ports"),
              ("active-backup", "Active-backup: failover, works with any switch"),
              ("balance-xor", "Balance-XOR: static aggregation (switch port-channel without LACP)")]
HASH_POLICIES = [("layer2", "layer2 (MAC addresses)"), ("layer2+3", "layer2+3 (MAC + IP)"), ("layer3+4", "layer3+4 (IP + port)")]
LACP_RATES = [("slow", "slow (every 30 s)"), ("fast", "fast (every second)")]
WAN_MODES = [("dhcp", "DHCP (from the ISP / upstream)"), ("static", "Static address"), ("pppoe", "PPPoE (DSL / fibre login)")]
DEFAULT_BOND_MODE = "802.3ad"
DEFAULT_HASH = "layer2+3"
DEFAULT_LACP_RATE = "slow"
IFNAME_MAX = 15
SECTIONS = (("wan", "WAN"), ("lan", "LAN"), ("mgmt_net", "Management"))
IFNAME_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._@-]*")


def enabled(profile):
    return bool(profile.get("net_bonds_vlans")) and profile.get("network_mode") == "wan-lan"


def defaults(c):
    """Add this module's keys to a fresh config (idempotent)."""
    c.setdefault("bonds", [])
    for k in ("wan", "lan"):
        c.setdefault(k, {"iface": "", "mode": "static" if k == "lan" else "dhcp", "address": ""}).setdefault("vlan", 0)
    for k in ("pppoe_user", "pppoe_password", "pppoe_service"):
        c["wan"].setdefault(k, "")
    c.setdefault("mgmt_net", {"iface": "", "vlan": 0, "mode": "static", "address": "", "gateway": "", "only": True})
    return c


def _vlan(v):
    try:
        return int(v or 0)
    except (TypeError, ValueError):
        return -1


def device(sec):
    """The kernel device a section ends up on ('' = not used)."""
    i = (sec or {}).get("iface") or ""
    if not i:
        return ""
    v = _vlan(sec.get("vlan"))
    return f"{i}.{v}" if v > 0 else i


def bond(c, name):
    return next((b for b in c.get("bonds", []) if b["name"] == name), None)


def members(c):
    return [m for b in c.get("bonds", []) for m in b.get("members", [])]


def in_use(c):
    """True when the store (not the installer's dhcpcd.conf) must own the network: any bond, VLAN, management network or PPPoE."""
    return bool(c.get("bonds") or _vlan(c["wan"].get("vlan")) > 0 or _vlan(c["lan"].get("vlan")) > 0
                or (c.get("mgmt_net") or {}).get("iface") or c["wan"].get("mode") == "pppoe")


def next_bond_name(c):
    n = 0
    while bond(c, f"bond{n}"):
        n += 1
    return f"bond{n}"


def add_bond(c, members_, mode=None, hash_=None, lacp_rate=None, name=None):
    """Create (or replace) a bond and return its name."""
    name = name or next_bond_name(c)
    b = {"name": name, "members": list(members_), "mode": mode or DEFAULT_BOND_MODE, "hash": hash_ or "", "lacp_rate": lacp_rate or ""}
    if b["mode"] == "802.3ad":
        b["hash"] = b["hash"] or DEFAULT_HASH
        b["lacp_rate"] = b["lacp_rate"] or DEFAULT_LACP_RATE
    c["bonds"] = [x for x in c.get("bonds", []) if x["name"] != name] + [b]
    return name


def prune_bonds(c):
    """Drop bonds no section uses any more (a GUI/TUI user switched a section back to a plain port)."""
    used = {(c.get(k) or {}).get("iface") for k, _ in SECTIONS}
    c["bonds"] = [b for b in c.get("bonds", []) if b["name"] in used]


def from_answers(c, n):
    """Read [[network.bond]] and the bond / vlan / pppoe / mgmt keys of the [network.*] tables (n = the parsed [network])."""
    defaults(c)
    for b in n.get("bond", []) or []:
        add_bond(c, b.get("members", []), b.get("mode"), b.get("hash", b.get("xmit_hash_policy")), b.get("lacp_rate"),
                 b.get("name") or None)
    for key, tbl in (("wan", "wan"), ("lan", "lan"), ("mgmt_net", "mgmt")):
        src = n.get(tbl, {}) or {}
        sec = c[key]
        if key == "mgmt_net":
            for k in ("iface", "mode", "address", "gateway"):
                if k in src:
                    sec[k] = src[k]
            if "only" in src:
                sec["only"] = bool(src["only"])
        if src.get("iface") == "none":
            sec["iface"] = ""
        if "bond" in src:
            m = src["bond"]
            sec["iface"] = add_bond(c, [m] if isinstance(m, str) else list(m), src.get("bond_mode"), src.get("bond_hash"),
                                    src.get("lacp_rate"), src.get("bond_name") or None)
        if "vlan" in src:
            sec["vlan"] = src["vlan"]
        if key == "wan":
            for k_in, k_out in (("pppoe_user", "pppoe_user"), ("pppoe_username", "pppoe_user"), ("pppoe_password", "pppoe_password"),
                                ("pppoe_service", "pppoe_service")):
                if k_in in src:
                    sec[k_out] = src[k_in]
    return c


def ifname_problem(n):
    if not n:
        return "no interface name"
    if len(n) > IFNAME_MAX:
        return f"{n!r} is longer than {IFNAME_MAX} characters (Linux limit)"
    if not IFNAME_RE.fullmatch(n):
        return f"{n!r} is not a valid interface name"
    return None


def vlan_problem(v):
    if isinstance(v, bool) or _vlan(v) < 0 or (v not in (0, "", None) and not (1 <= _vlan(v) <= 4094)):
        return f"VLAN tag {v!r} must be a number from 1 to 4094 (0 = untagged)"
    return None


def bond_problems(c, nic_names=None):
    errs, seen = [], {}
    names = [b["name"] for b in c.get("bonds", [])]
    for b in c.get("bonds", []):
        nm = b["name"]
        p = ifname_problem(nm)
        if p:
            errs.append(f"bond name: {p}")
        if names.count(nm) > 1:
            errs.append(f"two bonds are named {nm}")
        if b["mode"] not in [m for m, _ in BOND_MODES]:
            errs.append(f"bond {nm}: mode {b['mode']!r} is not offered (use {', '.join(m for m, _ in BOND_MODES)})")
        if b.get("hash") and b["hash"] not in [h for h, _ in HASH_POLICIES]:
            errs.append(f"bond {nm}: hash {b['hash']!r} is not offered (use {', '.join(h for h, _ in HASH_POLICIES)})")
        if b.get("hash") and b["mode"] == "active-backup":
            errs.append(f"bond {nm}: a hash policy only applies to 802.3ad and balance-xor")
        if b.get("lacp_rate") and b["lacp_rate"] not in [r for r, _ in LACP_RATES]:
            errs.append(f"bond {nm}: lacp_rate {b['lacp_rate']!r} must be slow or fast")
        if b.get("lacp_rate") and b["mode"] != "802.3ad":
            errs.append(f"bond {nm}: lacp_rate only applies to 802.3ad (LACP)")
        ms = b.get("members") or []
        if len(ms) < 1:
            errs.append(f"bond {nm} has no member ports")
        if len(set(ms)) != len(ms):
            errs.append(f"bond {nm} lists the same port twice")
        for m in ms:
            if m in names:
                errs.append(f"bond {nm}: a bond cannot be a member of another bond ({m})")
            elif m in seen and seen[m] != nm:
                errs.append(f"port {m} is in two bonds ({seen[m]} and {nm})")
            seen[m] = nm
            if nic_names is not None and m not in nic_names:
                errs.append(f"bond {nm}: port {m} does not exist on this machine (found: {', '.join(nic_names) or 'none'})")
    return errs


def problems(profile, c, nic_names=None):
    """Plain-words problems with the bonds / VLANs / management network / PPPoE (empty = ok)."""
    from . import system as S
    defaults(c)
    if not enabled(profile):
        if in_use(c):
            return ["this installer does not set up bonds, VLANs, PPPoE or a separate management network for this product"]
        return []
    errs = bond_problems(c, nic_names)
    mem = members(c)
    bonds = {b["name"] for b in c.get("bonds", [])}
    devs = {}
    for key, label in SECTIONS:
        sec = c[key]
        p = vlan_problem(sec.get("vlan"))
        if p:
            errs.append(f"{label}: {p}")
            continue
        i = sec.get("iface") or ""
        if not i:
            if _vlan(sec.get("vlan")) > 0:
                errs.append(f"{label}: VLAN {sec['vlan']} needs a port or bond to ride on (iface or bond)")
            continue
        if i in mem:
            errs.append(f"{label}: {i} is a member of bond {next(b['name'] for b in c['bonds'] if i in b['members'])}; "
                        "use the bond (optionally with a VLAN) instead")
        if nic_names is not None and i not in bonds and i not in nic_names:
            errs.append(f"{label}: interface {i} does not exist on this machine (found: {', '.join(nic_names) or 'none'})")
        d = device(sec)
        p = ifname_problem(d)
        if p:
            errs.append(f"{label}: the VLAN device name {p}")
        if d in devs:
            errs.append(f"{label} and {devs[d]} are the same network ({d}): give one of them its own port or VLAN tag")
        devs[d] = label
    w = c["wan"]
    if w.get("mode") not in [m for m, _ in WAN_MODES]:
        errs.append(f"WAN: mode {w.get('mode')!r} (use dhcp, static or pppoe)")
    if w.get("mode") == "pppoe" and not (w.get("pppoe_user") or "").strip():
        errs.append("WAN: PPPoE needs the user name your ISP gave you (pppoe_user)")
    m = c["mgmt_net"]
    if m.get("iface"):
        if m.get("mode") not in ("static", "dhcp"):
            errs.append(f"Management: mode {m.get('mode')!r} (use static or dhcp)")
        elif m.get("mode") == "static":
            p = S.cidr_problem(m.get("address", ""))
            if p:
                errs.append("Management address: " + p)
            elif m.get("gateway") and S.ip_problem(m["gateway"], m["address"]):
                errs.append(f"Management gateway: {S.ip_problem(m['gateway'], m['address'])}")
            else:
                for key, label in (("lan", "LAN"), ("wan", "WAN")):
                    o = c[key]
                    if device(o) and o.get("mode", "static") == "static" and o.get("address") and not S.cidr_problem(o["address"]):
                        if ipaddress.ip_interface(o["address"]).network.overlaps(ipaddress.ip_interface(m["address"]).network):
                            errs.append(f"Management network {m['address']} overlaps the {label} network {o['address']}")
    used = {(c.get(k) or {}).get("iface") for k, _ in SECTIONS}
    for b in c.get("bonds", []):
        if b["name"] not in used:
            errs.append(f"bond {b['name']} is not used by the WAN, LAN or management network")
    return errs


def describe_bond(b):
    extra = []
    if b.get("hash"):
        extra.append("hash " + b["hash"])
    if b.get("lacp_rate"):
        extra.append("LACP " + b["lacp_rate"])
    return f"{b['name']} = {' + '.join(b['members'])} ({b['mode']}{', ' + ', '.join(extra) if extra else ''})"


def plan_lines(c):
    """The network part of the review page / --plan when bonds / VLANs / mgmt are used."""
    out = [f"Bond {describe_bond(b)}" for b in c.get("bonds", [])]
    w, l, m = c["wan"], c["lan"], c["mgmt_net"]
    tag = lambda s: f" (VLAN {s['vlan']} on {s['iface']})" if _vlan(s.get("vlan")) > 0 else ""
    if device(w):
        how = {"static": f"{w.get('address')}" + (f" gw {w['gateway']}" if w.get("gateway") else ""),
               "pppoe": f"PPPoE as {w.get('pppoe_user')}"}.get(w.get("mode"), "DHCP")
        out.append(f"WAN {device(w)}{tag(w)}: {how}")
    if device(l):
        out.append(f"LAN {device(l)}{tag(l)}: {l.get('address')}")
    if device(m):
        a = m.get("address") if m.get("mode") == "static" else "DHCP"
        only = "SSH + web UI only here" if m.get("only", True) else "SSH + web UI here and on the LAN"
        out.append(f"Management {device(m)}{tag(m)}: {a} ({only})")
    out.append("Created by the firewall at its first boot (no console needed); the installer only writes the bootstrap file")
    return out


def _q(s):
    return '"' + str(s).replace("\\", "\\\\").replace('"', '\\"') + '"'


def bootstrap_tables(c):
    """TOML text: [[network.bond]], [[network.vlan]] and [firstboot] for the product's first boot."""
    out = []
    for b in c.get("bonds", []):
        out += ["", "[[network.bond]]", f"name = {_q(b['name'])}", "members = [" + ", ".join(_q(x) for x in b["members"]) + "]",
                f"mode = {_q(b['mode'])}"]
        if b.get("hash"):
            out.append(f"xmit_hash_policy = {_q(b['hash'])}")
        if b.get("lacp_rate"):
            out.append(f"lacp_rate = {_q(b['lacp_rate'])}")
    seen = set()
    for key, _ in SECTIONS:
        s = c[key]
        if s.get("iface") and _vlan(s.get("vlan")) > 0 and device(s) not in seen:
            seen.add(device(s))
            out += ["", "[[network.vlan]]", f"parent = {_q(s['iface'])}", f"tag = {_vlan(s['vlan'])}", f"name = {_q(device(s))}"]
    fb = ["", "[firstboot]", "# written by live-installer: applied once at the first boot, then the config store is authoritative"]
    w, l, m = c["wan"], c["lan"], c["mgmt_net"]
    if device(l) and l.get("address"):
        fb.append(f"lan_address = {_q(l['address'])}")
    if device(w):
        fb.append(f"wan_mode = {_q(w.get('mode') or 'dhcp')}")
        if w.get("mode") == "static":
            fb.append(f"wan_address = {_q(w['address'])}")
            if w.get("gateway"):
                fb.append(f"wan_gateway = {_q(w['gateway'])}")
            dns = [x for x in re.split(r"[\s,]+", (w.get("dns") or "").strip()) if x]
            if dns:
                fb.append("wan_dns = [" + ", ".join(_q(x) for x in dns) + "]")
        if w.get("mode") == "pppoe":
            fb.append(f"pppoe_username = {_q(w['pppoe_user'])}")
            fb.append(f"pppoe_password = {_q(w.get('pppoe_password', ''))}")
            if w.get("pppoe_service"):
                fb.append(f"pppoe_service = {_q(w['pppoe_service'])}")
    if device(m):
        if m.get("mode") == "static" and m.get("address"):
            fb.append(f"mgmt_address = {_q(m['address'])}")
            if m.get("gateway"):
                fb.append(f"mgmt_gateway = {_q(m['gateway'])}")
        fb.append(f"mgmt_only = {'true' if m.get('only', True) else 'false'}")
    return "\n".join(out + fb) + "\n"


_TABLE_RE = re.compile(r"^\s*\[\[?\s*([A-Za-z0-9_.\-]+)\s*\]\]?\s*(#.*)?$")


def strip_tables(text, names):
    """Remove whole TOML tables / array-of-table entries whose header is in `names` (e.g. 'network.bond', 'firstboot')."""
    out, skip = [], False
    for line in text.splitlines():
        m = _TABLE_RE.match(line)
        if m:
            skip = m.group(1) in names
        if not skip:
            out.append(line)
    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out) + ("\n" if out else "")


def apply_bootstrap(text, c):
    """patronus.toml text with the roles (wan/lan/mgmt devices), bonds, VLANs and [firstboot] of this install."""
    from .consolemenu import apply_roles
    w = device(c["wan"]) or ((c.get("wifi") or {}).get("iface") or "")
    if c["wan"].get("mode") == "pppoe":
        w = device(c["wan"])                # the parent; the first boot moves the role to the ppp device
    t = apply_roles(strip_tables(text, {"network.bond", "network.vlan", "firstboot"}), w, device(c["lan"]))
    t = "\n".join(x for x in t.splitlines() if not re.match(r"\s*mgmt_interfaces?\s*=", x)) + "\n"
    mg = device(c["mgmt_net"])
    if mg:
        lines, done = t.splitlines(), False
        for i, x in enumerate(lines):
            if x.strip() == "[network]" and not done:
                # right after the role lines apply_roles() put under the header (stable position: re-running gives the same file)
                j = i + 1
                if j < len(lines) and lines[j].startswith("# written by live-installer"):
                    j += 1
                while j < len(lines) and re.match(r"\s*(wan_interface|lan_interface)\s*=", lines[j]):
                    j += 1
                lines.insert(j, f"mgmt_interface = {_q(mg)}")
                done = True
                break
        t = "\n".join(lines) + "\n"
    return t.rstrip("\n") + "\n" + bootstrap_tables(c)
