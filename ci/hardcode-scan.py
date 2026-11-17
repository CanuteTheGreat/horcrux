#!/usr/bin/env python3
"""Find hard-coded operational values (hostnames, addresses, ports, interface
names, Wi-Fi parameters, URLs, timeouts, sizes, uids, fixed lists, paths).

Product rule: nothing operational is hard-coded. Every such value is a typed,
documented config key with a default (see docs/design/configurable-everything.md).

Modes:
  check (default)   fail on hits that are not in the baseline, not allowed
                    inline and not a recognised legit constant
  --update          rewrite the baseline from the current tree (use when a
                    batch converted values, or with a reviewed reason)
  --inventory FILE  write the full markdown inventory (file:line, class, key)
  --summary         print counts per category x class

Inline allow: a comment containing `hardcode-ok: <reason>` on the same line
or the line above (reason required), or a block between
`hardcode-ok-begin: <reason>` and `hardcode-ok-end` comments.

Config: ci/hardcode-scan.json (include/exclude globs, test globs).
Baseline: ci/hardcode-baseline.tsv (fingerprints of known hits; line
numbers are not part of the fingerprint, so edits elsewhere never trip it).
Stdlib only, no build needed.
"""
import argparse
import fnmatch
import hashlib
import json
import os
import re
import subprocess
import sys
from collections import Counter, defaultdict

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
CFG = os.path.join(ROOT, "ci", "hardcode-scan.json")
BASELINE = os.path.join(ROOT, "ci", "hardcode-baseline.tsv")

IPV4 = re.compile(r"(?<![\w.$-])((?:\d{1,3}\.){3}\d{1,3})(/\d{1,2})?(?![\w.])")
PORT = re.compile(r"(?i)\b(?:port|listen|bind)[a-z_]*\s*(?:=|:|=>)\s*\"?(\d{2,5})\b")
NFT_PORT = re.compile(r"\b(?:dport|sport)\s+(\d{2,5})\b|redirect to :(\d{2,5})\b")
DUR_RS = re.compile(r"Duration::from_(secs|millis|mins|micros)\(\s*(\d[\d_]*)\s*\)")
DUR_SH = re.compile(r"(?:^|[;&|(\s])(?:sleep|timeout)\s+(\d+(?:\.\d+)?)\b")
NAMED_CONST = re.compile(
    r"\bconst\s+([A-Z][A-Z0-9_]*(?:TIMEOUT|INTERVAL|RETRIES|RETRY|ATTEMPTS|DELAY|BACKOFF|TTL|PERIOD|WINDOW|LIMIT|MAX|MIN|SIZE|CAP|BYTES|SECS|MS|PORT|ADDR|HOST|URL|DIR|PATH|FILE|USER|GROUP|UID|GID|COUNT|KEEP|RETENTION|THRESHOLD)[A-Z0-9_]*)\s*:\s*[^=]+=\s*([^;]+);")
IFACE = re.compile(r"[\"'= ](eth\d+|wlan\d+|wl\d[\w-]*|br-lan|br-wan|enp\d+s\d+\w*|ens\d+|lan\d|wan\d|phy\d)[\"' ]")
WIFI = re.compile(r"(?i)\b(ssid|country_code|country|channel|hw_mode|htmode|vht_oper_chwidth)(?:=|\s*[:=]\s*\")([A-Za-z0-9_.-]+)")
HOSTNAME = re.compile(r"(?:^|[;&|])\s*hostname\s+([a-z][a-z0-9-]*)\b|set_hostname\(\s*\"([^\"]+)\"")
URL = re.compile(r"https?://[^\s\"'<>`)\]},]+")
PATH_RS = re.compile(r"\"(/(?:etc|var|run|opt|srv|data|mnt|tmp|usr/share|usr/lib|usr/libexec|home|root)/[^\"{}\s]*)\"")
UIDGID = re.compile(r"\b(?:uid|gid|UID|GID)\s*[:=]\s*(\d{3,})\b|:x:(\d{3,}):(\d{3,}):")
SIZE = re.compile(r"\b(\d+)\s*\*\s*1024(?:\s*\*\s*1024)?\b")
TZ = re.compile(r"\"((?:Africa|America|Antarctica|Asia|Atlantic|Australia|Europe|Indian|Pacific)/[A-Za-z_]+(?:/[A-Za-z_]+)?)\"")
LOCALE = re.compile(r"\"([a-z]{2,3}_[A-Z]{2}(?:\.UTF-8)?)\"")

WELL_KNOWN_PORTS = {"20", "21", "22", "53", "67", "68", "69", "123", "161", "162",
                    "179", "443", "500", "514", "546", "547", "853", "1701",
                    "1900", "4500", "5353", "5355", "80", "8443"}
# protocol-defined ports: only these are "legit constants"; 22/80/443/8443 as
# *listen* ports of Patronus/Horcrux services are operational (config).
PROTOCOL_PORTS = {"53", "67", "68", "69", "123", "161", "162", "179", "500",
                  "546", "547", "853", "1701", "1900", "4500", "5353", "5355"}


SPECIAL_CIDRS = {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10", "169.254.0.0/16",
                 "127.0.0.0/8", "0.0.0.0/8", "224.0.0.0/4", "240.0.0.0/4", "192.0.0.0/24", "192.0.2.0/24",
                 "198.18.0.0/15", "198.51.100.0/24", "203.0.113.0/24", "255.255.255.255/32", "0.0.0.0/0"}


def legit_ipv4(ip, cidr):
    if cidr and ip + cidr in SPECIAL_CIDRS:
        return "special-purpose range (IANA/RFC)"
    o = [int(x) for x in ip.split(".")]
    if any(x > 255 for x in o):
        return "not-an-address"
    if ip in ("0.0.0.0",) or o[0] == 127:
        return "loopback/any"
    if o[0] == 255 or ip.startswith("255.255.") or (o[0] in (128, 192, 224, 240, 248, 252, 254) and o[1:] == [0, 0, 0] and cidr is None and o[0] != 192):
        return "netmask"
    if o[0] >= 224:
        return "multicast/reserved (protocol)"
    if ip.startswith(("192.0.2.", "198.51.100.", "203.0.113.", "198.18.", "198.19.")):
        return "documentation/benchmark range"
    if o[0] == 169 and o[1] == 254:
        return "link-local (protocol)"
    return None


def path_legit(p, cfg):
    """System/package layout is a contract with the OS or a third-party
    package (where the distro's own init script reads smb.conf, /proc
    files), not an operational choice. Product data/state/config locations
    (cfg "product_path_prefixes") and storage roots stay config."""
    if any(p.startswith(x) for x in cfg.get("product_path_prefixes", [])):
        return None
    if p.startswith(("/proc/", "/sys/", "/dev/", "/usr/", "/bin/", "/sbin/", "/lib")):
        return "OS/package layout"
    if p.startswith("/etc/") or p.startswith("/run/") or p.startswith("/var/run/"):
        return "OS/package layout (file read by a third-party daemon or the init system)"
    return None


class Hit:
    __slots__ = ("path", "line", "cat", "match", "text", "cls", "reason", "key", "fp")

    def __init__(self, path, line, cat, match, text):
        self.path, self.line, self.cat, self.match, self.text = path, line, cat, match, text
        self.cls, self.reason, self.key = "config", "", ""


def load_cfg():
    with open(CFG) as f:
        return json.load(f)


def git_files():
    out = subprocess.run(["git", "-C", ROOT, "ls-files", "-z"], capture_output=True, check=True).stdout
    return [p for p in out.decode().split("\0") if p]


def match_any(path, globs):
    return any(fnmatch.fnmatch(path, g) for g in globs)


def is_text(path):
    try:
        with open(os.path.join(ROOT, path), "rb") as f:
            return b"\0" not in f.read(4096)
    except OSError:
        return False


def area_of(path):
    m = re.match(r"crates/([^/]+)/", path) or re.match(r"(horcrux-[a-z]+)/", path)
    if m:
        return m.group(1).replace("patronus-", "").replace("horcrux-", "")
    if "/libexec/" in path or "/overlay" in path:
        return "embedded." + os.path.basename(path).replace(".sh", "")
    if path.startswith("build/"):
        return "build." + path.split("/")[1]
    if path.startswith("gentoo/"):
        return "pkg." + os.path.basename(os.path.dirname(path.split("/files/")[0]))
    return os.path.basename(path).split(".")[0]


def scan_file(path, cfg, text=None):
    hits = []
    if text is None:
        try:
            with open(os.path.join(ROOT, path), encoding="utf-8", errors="replace") as f:
                text = f.read()
        except OSError:
            return hits
    lines = text.split("\n")
    is_rs = path.endswith(".rs")
    test_file = match_any(path, cfg.get("test_globs", []))
    in_test = False
    block = None  # reason of an open `hardcode-ok-begin: <reason>` ... `hardcode-ok-end` block
    prev = ""
    for i, raw in enumerate(lines, 1):
        s = raw.strip()
        if is_rs and re.match(r"#\[cfg\((all\()?test", s):
            in_test = True  # convention: the test module is at the end of the file
        mb = re.search(r"hardcode-ok-begin:\s*(.+?)\s*(?:\*/|-->|$)", raw)
        if mb:
            block = mb.group(1)
        elif "hardcode-ok-end" in raw:
            block = None
        if s.startswith(("//", "#", "*", "/*", "<!--")) and not s.startswith(("#!", "#[")):
            prev = raw
            continue
        found = []
        for m in IPV4.finditer(raw):
            ver = raw[max(0, m.start() - 9):m.start()].lower()
            if re.search(r"(\bversion|\bver)\s*[=:\"' ]*$|\bv$", ver):
                continue
            found.append(("address", m.group(0), legit_ipv4(m.group(1), m.group(2))))
        for m in PORT.finditer(raw):
            p = m.group(1)
            found.append(("port", p, "protocol port" if p in PROTOCOL_PORTS else None))
        for m in NFT_PORT.finditer(raw):
            # a rule matching a protocol's own traffic (HTTP on 80, HTTPS on 443, SSH on 22)
            p = next(g for g in m.groups() if g)
            legit = p in PROTOCOL_PORTS or p in ("22", "80", "443")
            found.append(("port", p, "protocol port" if legit else None))
        if is_rs:
            for m in DUR_RS.finditer(raw):
                found.append(("timing", m.group(0), None))
            m = NAMED_CONST.search(raw)
            if m and not any(c == "timing" for c, _, _ in found):
                n = m.group(1)
                leg = None
                if re.search(r"(NONCE|TAG_SIZE|KEY_SIZE|BLOCK_SIZE|_ENV$|_PATH_PREFIX$|^NL|^NFULNL|^ATTR_)", n):
                    leg = "format/protocol constant or identifier"
                found.append(("constant", n, leg))
            for m in PATH_RS.finditer(raw):
                found.append(("path", m.group(1), path_legit(m.group(1), cfg)))
        else:
            for m in DUR_SH.finditer(raw):
                found.append(("timing", m.group(0).strip(), None))
        for m in IFACE.finditer(raw):
            found.append(("interface", m.group(1), None))
        for m in WIFI.finditer(raw):
            if "$" in m.group(2) or m.group(2).lower() in ("none", "auto"):
                continue
            found.append(("wifi", m.group(0).strip(), None))
        for m in HOSTNAME.finditer(raw):
            v = m.group(1) or m.group(2)
            if v and v not in ("-f", "-s", "-i", "-F"):
                found.append(("hostname", v, None))
        for m in URL.finditer(raw):
            u = m.group(0)
            if re.match(r"https?://(localhost|127\.|\[::1\]|0\.0\.0\.0|example\.|[^/]*\.example\b|[^/]*\.test\b|\$|\{)", u):
                continue
            leg = None
            if re.search(r"(schemas?\.|www\.w3\.org|json-schema\.org|spdx\.org|purl\.org|xmlns)", u):
                leg = "schema/namespace identifier"
            elif re.search(r"(github\.com|wiki\.|docs?\.|\.md\b|rfc-editor|datatracker|gentoo\.org/wiki|man7\.org)", u) and not re.search(r"(api\.github|releases/download|raw\.githubusercontent)", u):
                leg = "documentation link"
            found.append(("url", u, leg))
        for m in UIDGID.finditer(raw):
            found.append(("uid/gid", m.group(0), None))
        if re.search(r"(?i)\b\w*(max|limit|cap|size|bytes|budget|quota)\w*\s*(:[^=]*)?=\s*[^=]", raw):
            # only limits; unit conversions (`bytes / (1024 * 1024)`) are not operational values
            for m in SIZE.finditer(raw):
                found.append(("size", m.group(0), None))
        tzs = TZ.findall(raw)
        if tzs:
            found.append(("fixed-list", "timezone " + tzs[0], None))
        locs = LOCALE.findall(raw)
        if locs and not tzs:
            found.append(("fixed-list", "locale " + locs[0], None))

        allow = block
        for src in (raw, prev):
            m = re.search(r"hardcode-ok:\s*(.+?)\s*(?:\*/|-->|$)", src)
            if m:
                allow = m.group(1)
                break
        seen = set()
        for cat, val, legit in found:
            if (cat, val) in seen:
                continue
            seen.add((cat, val))
            h = Hit(path, i, cat, val, s)
            if test_file or in_test:
                h.cls, h.reason = "test", "test fixture"
            elif match_any(path, cfg.get("default_config_globs", [])):
                h.cls, h.reason = "legit", "shipped default in a config file (user-editable)"
            elif match_any(path, cfg.get("build_input_globs", [])) and "/overlay" not in path:
                h.cls, h.reason = "legit", "build input, not on the running system"
            elif "placeholder=" in raw or "e.g." in raw:
                h.cls, h.reason = "legit", "UI example text"
            elif legit:
                h.cls, h.reason = "legit", legit
            elif allow:
                h.cls, h.reason = "legit", "inline: " + allow
            if cat == "port" and val in WELL_KNOWN_PORTS and h.cls == "config":
                h.reason = "service port"
            h.key = suggest_key(path, cat, val, s)
            norm = re.sub(r"\s+", " ", s)
            h.fp = hashlib.sha1(f"{path}\0{cat}\0{val}\0{norm}".encode()).hexdigest()[:16]
            hits.append(h)
        prev = raw
    return hits


def suggest_key(path, cat, val, text):
    area = area_of(path)
    m = NAMED_CONST.search(text)
    if m:
        return f"{area}.{m.group(1).lower()}"
    m = re.search(r"\b(?:let|const|static)\s+(?:mut\s+)?([a-zA-Z_][a-zA-Z0-9_]*)", text) or re.search(r"\b([a-zA-Z_][a-zA-Z0-9_]*)\s*[:=]", text)
    name = m.group(1).lower() if m else cat
    return f"{area}.{cat.replace('/', '_').replace('-', '_')}.{name}"[:80]


def scan(cfg):
    files = [p for p in git_files()
             if match_any(p, cfg["include"]) and not match_any(p, cfg.get("exclude", [])) and is_text(p)]
    hits = []
    for p in sorted(files):
        hits.extend(scan_file(p, cfg))
    # duplicate identical lines in one file: make fingerprints unique by occurrence
    occ = Counter()
    for h in hits:
        occ[h.fp] += 1
        if occ[h.fp] > 1:
            h.fp = f"{h.fp}#{occ[h.fp]}"
    return files, hits


def read_baseline():
    base = {}
    if os.path.exists(BASELINE):
        with open(BASELINE) as f:
            for ln in f:
                if ln.startswith("#") or not ln.strip():
                    continue
                parts = ln.rstrip("\n").split("\t")
                base[parts[0]] = parts
    return base


def write_baseline(hits):
    rows = sorted((h for h in hits if h.cls == "config"), key=lambda h: (h.path, h.cat, h.match))
    with open(BASELINE + ".tmp", "w") as f:
        f.write("# Known hard-coded operational values awaiting conversion to config keys.\n")
        f.write("# Generated by ci/hardcode-scan.py --update. Shrinks as values become settings;\n")
        f.write("# a NEW hard-coded value must become a setting (or carry `hardcode-ok: <reason>`).\n")
        f.write("# fingerprint\tpath\tcategory\tvalue\tproposed key\n")
        for h in rows:
            f.write(f"{h.fp}\t{h.path}\t{h.cat}\t{h.match[:60]}\t{h.key}\n")
    os.replace(BASELINE + ".tmp", BASELINE)
    return len(rows)


def write_inventory(path, hits, files):
    by = defaultdict(list)
    for h in hits:
        by[h.cls].append(h)
    head = subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    with open(path, "w") as f:
        f.write(f"# Hard-coded value inventory (generated)\n\n")
        f.write(f"Generated by `ci/hardcode-scan.py --inventory` at commit `{head}` over {len(files)} files "
                f"(scope: `ci/hardcode-scan.json`). Line numbers are a snapshot; the gate uses line-independent "
                f"fingerprints (`ci/hardcode-baseline.tsv`). Classes: **config** = must become a setting "
                f"(proposed key shown, final key named in configurable-everything.md when converted), "
                f"**legit** = protocol/format constant or inline-justified, **test** = test fixture.\n\n")
        f.write(summary_table(hits) + "\n")
        for cls in ("config", "legit", "test"):
            rows = by.get(cls, [])
            f.write(f"\n## {cls} ({len(rows)})\n\n")
            if cls == "test":
                per = Counter(h.path for h in rows)
                f.write("| file | hits |\n|---|---|\n")
                for p, n in sorted(per.items()):
                    f.write(f"| `{p}` | {n} |\n")
                continue
            f.write("| file:line | category | value | " + ("proposed key" if cls == "config" else "reason") + " |\n|---|---|---|---|\n")
            for h in sorted(rows, key=lambda h: (h.path, h.line)):
                v = h.match.replace("|", "\\|")[:70]
                extra = h.key if cls == "config" else h.reason
                f.write(f"| `{h.path}:{h.line}` | {h.cat} | `{v}` | {extra.replace('|', '/')} |\n")


def summary_table(hits):
    cats = sorted({h.cat for h in hits})
    c = Counter((h.cat, h.cls) for h in hits)
    out = "| category | config | legit | test | total |\n|---|---|---|---|---|\n"
    for k in cats:
        a, b, t = c[(k, "config")], c[(k, "legit")], c[(k, "test")]
        out += f"| {k} | {a} | {b} | {t} | {a + b + t} |\n"
    tot = Counter(h.cls for h in hits)
    out += f"| **all** | {tot['config']} | {tot['legit']} | {tot['test']} | {len(hits)} |\n"
    return out


def self_test():
    """The classifier on known lines (run in CI before the scan)."""
    cfg = {"test_globs": ["*/tests/*"], "default_config_globs": ["*.toml"], "build_input_globs": [],
           "product_path_prefixes": ["/var/lib/prod"]}

    def one(path, text):
        return [(h.cat, h.match, h.cls) for h in scan_file(path, cfg, text)]

    cases = [
        ("src/a.rs", 'let t = Duration::from_secs(30);', [("timing", "Duration::from_secs(30)", "config")]),
        ("src/a.rs", 'const LOGIN_TIMEOUT: u64 = 5;', [("constant", "LOGIN_TIMEOUT", "config")]),
        ("src/a.rs", 'let lan = "192.168.1.1";', [("address", "192.168.1.1", "config")]),
        ("src/a.rs", 'let any = "0.0.0.0"; let net = "10.0.0.0/8";', [("address", "0.0.0.0", "legit"), ("address", "10.0.0.0/8", "legit")]),
        ("src/a.rs", 'let v = "1.2.3.4"; // hardcode-ok: test vector from RFC 1234', [("address", "1.2.3.4", "legit")]),
        ("src/a.rs", 'let p = "/var/lib/prod/x.db"; let q = "/etc/samba/smb.conf";', [("path", "/var/lib/prod/x.db", "config"), ("path", "/etc/samba/smb.conf", "legit")]),
        ("x/early", "hostname slate", [("hostname", "slate", "config")]),
        ("x/initd", "need hostname net", []),
        ("x/ap", "ssid=HomeNet", [("wifi", "ssid=HomeNet", "config")]),
        ("x/ap", 'echo "ssid=$SSID"', []),
        ("a.toml", 'lan = "192.168.1.1/24"', [("address", "192.168.1.1/24", "legit")]),
        ("x/tests/t.rs", 'let a = "192.168.9.9";', [("address", "192.168.9.9", "test")]),
        ("src/a.rs", '#[cfg(test)]\nmod tests { let a = "192.168.9.9"; }', [("address", "192.168.9.9", "test")]),
        ("src/a.py", '# hardcode-ok-begin: shortlist\nX = ["Europe/Berlin"]\n# hardcode-ok-end\nY = ["Asia/Tokyo"]',
         [("fixed-list", "timezone Europe/Berlin", "legit"), ("fixed-list", "timezone Asia/Tokyo", "config")]),
        ("src/a.rs", 'let u = "https://updates.vendor.net/feed";', [("url", "https://updates.vendor.net/feed", "config")]),
        ("src/a.rs", 'let v = format!("version 1.2.3.4");', []),
        ("src/a.rs", '"add rule t input tcp dport 3129 accept"', [("port", "3129", "config")]),
        ("src/a.rs", '"add rule t input udp dport 53 accept"', [("port", "53", "legit")]),
    ]
    bad = 0
    for path, text, want in cases:
        got = one(path, text)
        if got != want:
            bad += 1
            print(f"self-test FAIL {path}: {text!r}\n  want {want}\n  got  {got}")
    print(f"hardcode-scan self-test: {len(cases) - bad}/{len(cases)} ok")
    return 1 if bad else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--inventory")
    ap.add_argument("--summary", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    cfg = load_cfg()
    files, hits = scan(cfg)
    if a.summary:
        print(summary_table(hits))
    if a.inventory:
        write_inventory(a.inventory, hits, files)
        print(f"inventory: {a.inventory}")
    if a.update:
        n = write_baseline(hits)
        print(f"baseline: {n} known config hits")
        return 0
    if a.summary or a.inventory:
        return 0
    base = read_baseline()
    new = [h for h in hits if h.cls == "config" and h.fp not in base]
    live = {h.fp for h in hits}
    gone = [k for k in base if k not in live]
    if gone:
        print(f"note: {len(gone)} baseline entries no longer present (converted or moved); "
              f"run ci/hardcode-scan.py --update to shrink the baseline")
    if new:
        print(f"FAIL: {len(new)} new hard-coded operational value(s). Make each a typed, documented "
              f"setting (docs/design/configurable-everything.md), or mark a real constant with "
              f"`hardcode-ok: <reason>` on the line or the line above:")
        for h in new[:200]:
            print(f"  {h.path}:{h.line}: [{h.cat}] {h.match}   (proposed key: {h.key})")
        return 1
    print(f"hardcode-scan: ok ({len(files)} files, {sum(1 for h in hits if h.cls == 'config')} known, 0 new)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
