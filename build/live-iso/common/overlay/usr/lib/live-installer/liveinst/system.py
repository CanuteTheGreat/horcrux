"""System probing, validation helpers and the install engine (no UI here)."""
import ipaddress
import json
import os
import re
import shutil
import subprocess
import time

LOG = "/var/log/live-installer.log"


# --------------------------------------------------------------------------
# small helpers
# --------------------------------------------------------------------------
def fmt_size(n):
    n = float(n)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1000 or unit == "TB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1000.0


def sh(cmd, timeout=60):
    try:
        r = subprocess.run(cmd, shell=isinstance(cmd, str), capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout or "") + (r.stderr or "")
    except Exception as e:  # noqa
        return 1, str(e)


def read(path, default=""):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return default


def efi_boot():
    return os.path.isdir("/sys/firmware/efi")


# --------------------------------------------------------------------------
# profile (per project) - /etc/live-installer/profile.toml
# --------------------------------------------------------------------------
def load_profile(path="/etc/live-installer/profile.toml"):
    import tomllib
    with open(path, "rb") as f:
        p = tomllib.load(f)
    p.setdefault("filesystems", ["ext4", "xfs"])
    p.setdefault("swap_mb", 0)
    p.setdefault("network_mode", "management")
    p.setdefault("services", [])
    p.setdefault("web_scheme", "https")
    p.setdefault("default_hostname", p["name"] + ".localdomain")
    return p


# --------------------------------------------------------------------------
# validation
# --------------------------------------------------------------------------
COMMON_BAD = {"password", "passw0rd", "123456", "12345678", "qwerty", "letmein", "admin", "administrator",
              "changeme", "horcrux", "patronus", "welcome", "iloveyou", "root", "toor"}


def password_problem(pw, hostname="", min_len=12):
    """Return None when acceptable, else a sentence."""
    if len(pw) < min_len:
        return f"Password must be at least {min_len} characters ({len(pw)} so far)."
    low = pw.lower()
    for bad in COMMON_BAD:
        if bad in low and len(pw) < 20:
            return f"Password contains the guessable word '{bad}'."
    if len(set(pw)) < 5:
        return "Password uses too few different characters."
    classes = sum(bool(re.search(p, pw)) for p in (r"[a-z]", r"[A-Z]", r"[0-9]", r"[^A-Za-z0-9]"))
    if classes < 3 and len(pw) < 20:
        return "Mix at least 3 of: lower case, upper case, digits, symbols (or use 20+ characters)."
    seq = "abcdefghijklmnopqrstuvwxyz0123456789"
    if any(seq[i:i + 6] in low for i in range(len(seq) - 5)):
        return "Password contains a long keyboard/alphabet sequence."
    h = hostname.split(".")[0].lower()
    if h and len(h) >= 4 and h in low:
        return "Password must not contain the host name."
    return None


def password_strength(pw):
    score = min(len(pw), 20) * 3
    score += 8 * sum(bool(re.search(p, pw)) for p in (r"[a-z]", r"[A-Z]", r"[0-9]", r"[^A-Za-z0-9]"))
    score = min(score, 100)
    word = "weak" if score < 60 else "ok" if score < 80 else "strong"
    return f"{word} ({score}/100)"


def hostname_problem(h):
    if not h or len(h) > 253:
        return "Enter a host name (FQDN preferred, e.g. node1.example.com)."
    for label in h.split("."):
        if not re.fullmatch(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?", label):
            return "Use lower-case letters, digits and '-' in each dot-separated label."
    return None


def cidr_problem(v):
    try:
        i = ipaddress.ip_interface(v)
    except ValueError:
        return "Use CIDR notation, e.g. 192.168.1.10/24"
    if i.version != 4:
        return "Only IPv4 static addresses are supported here."
    if i.network.prefixlen > 30:
        return "Prefix length must be 30 or smaller."
    if i.ip == i.network.network_address or i.ip == i.network.broadcast_address:
        return "That is the network/broadcast address."
    return None


def ip_problem(v, inside=None):
    try:
        a = ipaddress.ip_address(v)
    except ValueError:
        return "Not a valid IP address."
    if inside:
        try:
            if a not in ipaddress.ip_interface(inside).network:
                return f"Gateway must be inside {ipaddress.ip_interface(inside).network}."
        except ValueError:
            pass
    return None


def dns_problem(v):
    for part in re.split(r"[\s,]+", v.strip()):
        if part and ip_problem(part):
            return f"'{part}' is not a valid IP address."
    return None if v.strip() else "Enter at least one DNS server."


def email_problem(v):
    return None if re.fullmatch(r"[^@\s]+@[^@\s]+\.[^@\s]+", v or "") else "Enter an e-mail address like admin@example.com"


def ssh_key_problem(v):
    v = (v or "").strip()
    if not v:
        return None
    lines = [l for l in v.splitlines() if l.strip() and not l.startswith("#")]
    for l in lines:
        if not re.match(r"^(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp\d+|sk-ssh-ed25519@openssh\.com|sk-ecdsa-sha2-nistp256@openssh\.com) [A-Za-z0-9+/=]+( .*)?$", l.strip()):
            return "Not an OpenSSH public key line (ssh-ed25519 AAAA... comment)."
        rc, _o = sh(f"echo {shquote(l.strip())} | ssh-keygen -l -f /dev/stdin")
        if rc != 0:
            return "ssh-keygen rejects this public key."
    return None


def shquote(s):
    return "'" + s.replace("'", "'\\''") + "'"


def crypt_hash_problem(h):
    if not re.fullmatch(r"\$(6|5|y|2[aby])\$[^:\s]+", h or ""):
        return "root_password_hash must be a crypt hash ($6$..., $y$..., $5$... or $2b$...), never a plain password"
    return None


# --------------------------------------------------------------------------
# hardware probing
# --------------------------------------------------------------------------
def live_disk_name():
    dev = os.path.realpath("/run/initramfs/livedev") if os.path.exists("/run/initramfs/livedev") else ""
    if not dev:
        rc, out = sh("findmnt -no SOURCE /run/initramfs/live 2>/dev/null")
        dev = out.strip() if rc == 0 else ""
    if not dev:
        return ""
    rc, out = sh(["lsblk", "-no", "PKNAME", dev])
    pk = out.strip().splitlines()[0] if rc == 0 and out.strip() else ""
    return pk or os.path.basename(dev)


def list_disks(include_live=False):
    rc, out = sh(["lsblk", "-J", "-b", "-o", "NAME,PATH,MODEL,SIZE,TYPE,RM,TRAN,ROTA,FSTYPE,LABEL,PARTLABEL"])
    if rc != 0:
        return []
    live = live_disk_name()
    disks = []
    for d in json.loads(out).get("blockdevices", []):
        if d.get("type") != "disk" or d["name"].startswith(("loop", "ram", "zram", "sr", "fd", "nbd")):
            continue
        if int(d.get("size") or 0) < 2 * 1000**3:
            continue
        if d["name"] == live and not include_live:
            continue
        tran = (d.get("tran") or "").lower()
        if d["name"].startswith("nvme"):
            kind = "NVMe"
        elif d["name"].startswith("mmcblk"):
            kind = "eMMC"
        elif tran == "usb":
            kind = "USB"
        elif str(d.get("rota")) in ("1", "True", "true"):
            kind = "HDD"
        else:
            kind = "SSD"
        parts = []
        for c in d.get("children") or []:
            parts.append({"path": c.get("path") or "/dev/" + c["name"], "size": int(c.get("size") or 0),
                          "fstype": c.get("fstype") or "", "label": c.get("label") or "",
                          "partlabel": c.get("partlabel") or ""})
        model = (d.get("model") or "").strip() or "(unknown model)"
        disks.append({"path": d.get("path") or "/dev/" + d["name"], "name": d["name"], "size": int(d.get("size") or 0),
                      "model": model, "kind": kind, "removable": str(d.get("rm")) in ("1", "True", "true"),
                      "parts": parts})
    return disks


NIC_SKIP = ("lo", "veth", "docker", "br-", "virbr", "bond", "dummy", "sit", "tun", "tap", "ifb", "gre", "wg")


def list_nics():
    nics = []
    for n in sorted(os.listdir("/sys/class/net")):
        if n.startswith(NIC_SKIP):
            continue
        base = f"/sys/class/net/{n}"
        if not os.path.exists(base + "/device"):
            continue
        wifi = os.path.isdir(base + "/wireless") or os.path.isdir(base + "/phy80211")
        drv = os.path.basename(os.path.realpath(base + "/device/driver")) if os.path.exists(base + "/device/driver") else ""
        rc, props = sh(f"udevadm info -q property -p {base}")
        model = ""
        for key in ("ID_MODEL_FROM_DATABASE", "ID_MODEL_ENC"):
            m = re.search(rf"^{key}=(.*)$", props, re.M)
            if m:
                model = m.group(1).replace("\\x20", " ")
                break
        nics.append({"name": n, "mac": read(base + "/address"), "wifi": wifi, "driver": drv, "model": model})
    return nics


def bring_up(nics):
    for n in nics:
        if not n["wifi"]:
            sh(["ip", "link", "set", n["name"], "up"])


def carrier(name):
    return read(f"/sys/class/net/{name}/carrier") == "1"


def nic_ip(name):
    rc, out = sh(["ip", "-4", "-o", "addr", "show", "dev", name])
    m = re.search(r"inet (\S+)", out)
    return m.group(1) if m else ""


def nic_line(n):
    kind = "Wi-Fi" if n["wifi"] else ("link UP" if carrier(n["name"]) else "no link")
    ip = nic_ip(n["name"])
    return f"{n['name']:<10} {n['mac']}  {kind:<8} {n['driver']:<10} {n['model'][:24]}" + (f"  {ip}" if ip else "")


def detect_link_change(names, timeout=45, tick=None):
    """OPNsense-style assignment: wait until one NIC's carrier flips; return its name."""
    base = {n: carrier(n) for n in names}
    end = time.time() + timeout
    while time.time() < end:
        for n in names:
            if carrier(n) != base[n]:
                return n
        if tick:
            tick(int(end - time.time()))
        time.sleep(0.5)
    return None


def wifi_scan(iface):
    sh(["iwctl", "station", iface, "scan"])
    time.sleep(3)
    rc, out = sh(["iwctl", "station", iface, "get-networks"])
    out = re.sub(r"\x1b\[[0-9;]*m", "", out)
    nets = []
    for line in out.splitlines():
        m = re.match(r"^\s*(?:>\s*)?(.+?)\s{2,}(\S+)\s+([*]+)\s*$", line)
        if m and m.group(1) != "Network name":
            nets.append((m.group(1).strip(), m.group(2), m.group(3)))
    return nets


def wifi_connect(iface, ssid, psk):
    cmd = ["iwctl"] + (["--passphrase", psk] if psk else []) + ["station", iface, "connect", ssid]
    rc, out = sh(cmd, timeout=40)
    end = time.time() + 30
    while time.time() < end:
        if nic_ip(iface):
            return True, nic_ip(iface)
        time.sleep(1)
    return False, out.strip()[-200:]


def keymaps():
    found = set()
    for root, _d, files in os.walk("/usr/share/keymaps"):
        for f in files:
            if f.endswith((".map.gz", ".map")):
                found.add(f.split(".map")[0])
    pref = ["us", "uk", "de", "fr", "es", "it", "se", "no", "dk", "fi", "pt-latin1", "br-abnt2", "dvorak", "colemak", "us-acentos"]
    out = [k for k in pref if k in found]
    out += sorted(found - set(out))
    return out or ["us"]


def timezones():
    zs = []
    tab = "/usr/share/zoneinfo/zone1970.tab"
    if os.path.exists(tab):
        for l in open(tab):
            if not l.startswith("#") and l.strip():
                zs.append(l.split("\t")[2])
    else:
        for root, _d, files in os.walk("/usr/share/zoneinfo"):
            for f in files:
                p = os.path.relpath(os.path.join(root, f), "/usr/share/zoneinfo")
                if "/" in p and not p.startswith(("posix", "right", "Etc")):
                    zs.append(p)
    zs = sorted(set(zs))
    return ["UTC"] + zs


LOCALES = [("en_US.UTF-8", "English (United States)"), ("en_GB.UTF-8", "English (United Kingdom)"),
           ("de_DE.UTF-8", "Deutsch"), ("fr_FR.UTF-8", "Francais"), ("es_ES.UTF-8", "Espanol"),
           ("it_IT.UTF-8", "Italiano"), ("pt_BR.UTF-8", "Portugues (Brasil)"), ("nl_NL.UTF-8", "Nederlands"),
           ("sv_SE.UTF-8", "Svenska"), ("pl_PL.UTF-8", "Polski"), ("ru_RU.UTF-8", "Russian"),
           ("ja_JP.UTF-8", "Japanese"), ("zh_CN.UTF-8", "Chinese (Simplified)")]


def apply_live_keymap(km):
    sh(["loadkeys", km], timeout=15)


def mem_mb():
    m = re.search(r"MemTotal:\s+(\d+)", read("/proc/meminfo"))
    return int(m.group(1)) // 1024 if m else 0


def large_screen():
    try:
        w = int(read("/sys/class/graphics/fb0/virtual_size").split(",")[0])
        return w >= 1600
    except (ValueError, IndexError):
        return False


# --------------------------------------------------------------------------
# answer-file location
# --------------------------------------------------------------------------
def find_answer_file(proj, cmdline):
    """kernel arg <proj>.answers=URL | path, else a volume labelled <PROJ>-ANSWERS holding answer.toml"""
    m = re.search(rf"{proj}\.answers=(\S+)", cmdline)
    if m:
        src = m.group(1)
        dst = "/run/live-installer-answers.toml"
        if src.startswith(("http://", "https://")):
            import urllib.request
            with urllib.request.urlopen(src, timeout=30) as r:
                open(dst, "wb").write(r.read())
            return dst
        if os.path.exists(src):
            return src
    label = f"{proj.upper()}-ANSWERS"
    for _ in range(15):          # USB media can take a few seconds to appear
        rc, dev = sh(["blkid", "-L", label])
        if rc == 0 and dev.strip():
            mnt = "/run/answers-media"
            os.makedirs(mnt, exist_ok=True)
            if sh(["mount", "-o", "ro", dev.strip(), mnt])[0] == 0 or os.path.ismount(mnt):
                for name in ("answer.toml", "answers.toml"):
                    p = os.path.join(mnt, name)
                    if os.path.exists(p):
                        shutil.copy(p, "/run/live-installer-answers.toml")
                        return "/run/live-installer-answers.toml"
        time.sleep(2)
    return None


# --------------------------------------------------------------------------
# config model shared by the TUI and the answer file
# --------------------------------------------------------------------------
def default_config(profile):
    return {
        "locale": "en_US.UTF-8", "keymap": "us", "timezone": "UTC",
        "disks": [], "fs": "ext4", "zfs_raid": "single",
        "hostname": profile["default_hostname"],
        "mgmt": {"iface": "", "mode": "dhcp", "address": "", "gateway": "", "dns": ""},
        "wan": {"iface": "", "mode": "dhcp", "address": "", "gateway": "", "dns": ""},
        "lan": {"iface": "", "mode": "static", "address": "192.168.1.1/24"},
        "wifi": None,
        "root_password": "", "root_hash": "", "web_password": "",
        "ssh_keys": [], "ssh_password_auth": False, "email": "",
    }


def config_from_answers(profile, path):
    import tomllib
    with open(path, "rb") as f:
        a = tomllib.load(f)
    c = default_config(profile)
    g = a.get("global", {})
    for k_in, k_out in (("locale", "locale"), ("keyboard", "keymap"), ("timezone", "timezone"), ("hostname", "hostname"),
                        ("email", "email")):
        if g.get(k_in):
            c[k_out] = g[k_in]
    c["root_hash"] = g.get("root_password_hash", "")
    keys = g.get("ssh_keys", g.get("ssh_key", []))
    c["ssh_keys"] = [keys] if isinstance(keys, str) else list(keys)
    c["ssh_password_auth"] = bool(g.get("ssh_password_auth", False))
    d = a.get("disk", {})
    c["fs"] = d.get("filesystem", "ext4")
    c["zfs_raid"] = d.get("zfs_raid", "single")
    dl = d.get("disks", d.get("disk", "first"))
    c["disks"] = [dl] if isinstance(dl, str) else list(dl)
    n = a.get("network", {})
    for sect in ("mgmt", "wan", "lan"):
        src = n.get("management" if sect == "mgmt" else sect, {})
        for k in ("iface", "mode", "address", "gateway"):
            if k in src:
                c[sect][k] = src[k]
        if "dns" in src:
            v = src["dns"]
            c[sect]["dns"] = " ".join(v) if isinstance(v, list) else v
    return c


def validate_config(profile, c, unattended=False, require_secret=True):
    """Return a list of problems (empty = ok)."""
    errs = []
    if c["fs"] not in profile["filesystems"]:
        errs.append(f"filesystem {c['fs']} not offered ({', '.join(profile['filesystems'])})")
    if hostname_problem(c["hostname"]):
        errs.append("hostname: " + hostname_problem(c["hostname"]))
    if not c["disks"]:
        errs.append("no target disk")
    if c["fs"] == "zfs" and c["zfs_raid"] == "mirror" and len(set(c["disks"])) < 2:
        errs.append("ZFS mirror needs two different disks")
    if c["fs"] != "zfs" and len(c["disks"]) > 1:
        errs.append("only ZFS uses more than one disk")
    if require_secret:
        if c["root_hash"]:
            if crypt_hash_problem(c["root_hash"]):
                errs.append(crypt_hash_problem(c["root_hash"]))
        elif c["root_password"]:
            pp = password_problem(c["root_password"], c["hostname"])
            if pp:
                errs.append(pp)
        elif not c["ssh_keys"]:
            errs.append("no root password hash and no SSH key supplied: the installed system would be unreachable")
    for k in c["ssh_keys"]:
        if ssh_key_problem(k):
            errs.append("ssh key: " + ssh_key_problem(k))
    if c["email"] and email_problem(c["email"]):
        errs.append("email: " + email_problem(c["email"]))
    nets = ["wan", "lan"] if profile["network_mode"] == "wan-lan" else ["mgmt"]
    for s in nets:
        n = c[s]
        if n.get("mode") == "static":
            if cidr_problem(n.get("address", "")):
                errs.append(f"{s} address: " + cidr_problem(n["address"]))
            if s != "lan":
                if n.get("gateway") and ip_problem(n["gateway"], n["address"]):
                    errs.append(f"{s} gateway: " + ip_problem(n["gateway"], n["address"]))
                if n.get("dns") and dns_problem(n["dns"]):
                    errs.append(f"{s} dns: " + dns_problem(n["dns"]))
    return errs


def part_path(disk, n):
    return f"{disk}p{n}" if disk[-1].isdigit() else f"{disk}{n}"


# --------------------------------------------------------------------------
# the installer
# --------------------------------------------------------------------------
class InstallError(Exception):
    pass


class Installer:
    def __init__(self, profile, cfg, log, progress):
        self.p, self.c = profile, cfg
        self.log_cb, self.progress_cb = log, progress
        self.target = "/mnt/target"
        self.efi = efi_boot()
        self.logf = None

    # -- plumbing -----------------------------------------------------
    def log(self, line):
        line = line.rstrip()
        if not line:
            return
        self.log_cb(line)
        try:
            with open(LOG, "a") as f:
                f.write(line + "\n")
        except OSError:
            pass

    def prog(self, pct, note):
        self.progress_cb(pct, note)

    def run(self, cmd, check=True, input_=None, line_cb=None, chroot=False, env=None):
        if chroot:
            cmd = ["chroot", self.target] + (cmd if isinstance(cmd, list) else ["sh", "-c", cmd])
        self.log("$ " + (cmd if isinstance(cmd, str) else " ".join(shquote(x) if " " in x else x for x in cmd)))
        pr = subprocess.Popen(cmd, shell=isinstance(cmd, str), stdin=subprocess.PIPE if input_ is not None else subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1,
                              env={**os.environ, **(env or {})})
        if input_ is not None:
            pr.stdin.write(input_)
            pr.stdin.close()
        for line in pr.stdout:
            if line_cb:
                line_cb(line)
            else:
                self.log(line)
        rc = pr.wait()
        if rc != 0 and check:
            raise InstallError(f"command failed ({rc}): {cmd if isinstance(cmd, str) else ' '.join(cmd)}")
        return rc

    def write(self, rel, text, mode=0o644, append=False):
        p = os.path.join(self.target, rel.lstrip("/"))
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "a" if append else "w") as f:
            f.write(text)
        os.chmod(p, mode)

    # -- stages ---------------------------------------------------------
    def go(self):
        c, p = self.c, self.p
        self.log(f"== live-installer: {p['title']} on {', '.join(c['disks'])} ({'UEFI' if self.efi else 'BIOS'}), fs={c['fs']}")
        self.prog(2, "Partitioning")
        self.partition()
        self.prog(8, "Creating file systems")
        self.mkfs()
        self.prog(12, "Mounting")
        self.mount()
        self.prog(14, "Copying the system (this takes a few minutes)")
        self.copy_system()
        self.prog(80, "Configuring the new system")
        self.configure()
        self.prog(90, "Installing the boot loader")
        self.bootloader()
        self.prog(97, "Finishing")
        self.finish()
        self.prog(100, "Done")

    def zfs(self):
        return self.c["fs"] == "zfs"

    def layout(self, disk):
        """Partition numbers: 1 = ESP/BIOS-boot, 2 = /boot, 3 = swap, 4 = root."""
        return {n: part_path(disk, n) for n in (1, 2, 3, 4)}

    def partition(self):
        swap = self.p["swap_mb"]
        if swap:
            ram = mem_mb()
            swap = max(1024, min(swap, ram)) if ram else swap
        self.swap_mb = swap
        for disk in self.c["disks"]:
            self.run(["sh", "-c", f"swapoff -a 2>/dev/null; wipefs -a {shquote(disk)} >/dev/null 2>&1; sgdisk -Z {shquote(disk)} >/dev/null 2>&1; true"], check=False)
            first = ["-n", "1:0:+512M", "-t", "1:ef00", "-c", "1:ESP"] if self.efi else ["-n", "1:0:+1M", "-t", "1:ef02", "-c", "1:BIOS-boot"]
            args = ["sgdisk"] + first + ["-n", "2:0:+1G", "-t", "2:8300", "-c", "2:boot"]
            if swap:
                args += ["-n", f"3:0:+{swap}M", "-t", "3:8200", "-c", "3:swap"]
            else:
                args += ["-n", "3:0:+64M", "-t", "3:8200", "-c", "3:swap-unused"]
            args += ["-n", "4:0:0", "-t", "4:bf00" if self.zfs() else "4:8300", "-c", "4:root", disk]
            self.run(args)
            self.run(["partprobe", disk], check=False)
        self.run("udevadm settle --timeout=20 || true", check=False)
        time.sleep(1)

    def mkfs(self):
        c = self.c
        d0 = c["disks"][0]
        L = self.layout(d0)
        for disk in c["disks"]:
            l = self.layout(disk)
            if self.efi:
                self.run(["mkfs.vfat", "-F32", "-n", "ESP", l[1]])
        self.run(["mkfs.ext4", "-F", "-q", "-L", "boot", L[2]])
        if self.swap_mb:
            self.run(["mkswap", "-L", "swap", L[3]])
        if c["fs"] == "ext4":
            self.run(["mkfs.ext4", "-F", "-q", "-L", "root", L[4]])
        elif c["fs"] == "xfs":
            self.run(["mkfs.xfs", "-f", "-L", "root", L[4]])
        elif c["fs"] == "zfs":
            devs = [self.layout(d)[4] for d in c["disks"]]
            self.run("modprobe zfs", check=False)
            self.run("test -e /etc/hostid || zgenhostid", check=False)
            vd = (["mirror"] + devs) if c["zfs_raid"] == "mirror" else devs[:1]
            self.run(["zpool", "create", "-f", "-o", "ashift=12", "-o", "autotrim=on", "-O", "compression=lz4",
                      "-O", "acltype=posixacl", "-O", "xattr=sa", "-O", "relatime=on", "-O", "mountpoint=none",
                      "-R", self.target, "rpool"] + vd)
            self.run(["zfs", "create", "-o", "mountpoint=/", "-o", "canmount=noauto", "rpool/ROOT"])
        else:
            raise InstallError("unknown filesystem " + c["fs"])

    def mount(self):
        c = self.c
        L = self.layout(c["disks"][0])
        os.makedirs(self.target, exist_ok=True)
        if self.zfs():
            self.run(["zfs", "mount", "rpool/ROOT"])
        else:
            self.run(["mount", L[4], self.target])
        os.makedirs(self.target + "/boot", exist_ok=True)
        self.run(["mount", L[2], self.target + "/boot"])
        if self.efi:
            os.makedirs(self.target + "/boot/efi", exist_ok=True)
            self.run(["mount", L[1], self.target + "/boot/efi"])
        if self.swap_mb:
            self.run(["swapon", L[3]], check=False)

    def find_squashfs(self):
        for pth in ("/run/initramfs/live/LiveOS/squashfs.img", "/run/initramfs/squashfs.img", "/cdrom/LiveOS/squashfs.img"):
            if os.path.exists(pth):
                return pth
        label = read("/etc/live-iso-volid")
        if label:
            rc, dev = sh(["blkid", "-L", label])
            if rc == 0:
                os.makedirs("/run/live-src", exist_ok=True)
                if sh(["mount", "-o", "ro", dev.strip(), "/run/live-src"])[0] == 0 and os.path.exists("/run/live-src/LiveOS/squashfs.img"):
                    return "/run/live-src/LiveOS/squashfs.img"
        raise InstallError("cannot find the live medium's squashfs.img (was the stick removed?)")

    def copy_system(self):
        img = self.find_squashfs()
        self.log("system image: " + img)
        last = [0]

        def cb(line):
            s = line.strip()
            if s.isdigit():
                pct = int(s)
                if pct != last[0]:
                    last[0] = pct
                    self.prog(14 + int(pct * 0.64), f"Copying the system: {pct}%")
            elif s:
                self.log(s)
        self.run(["unsquashfs", "-percentage", "-f", "-d", self.target, img], line_cb=cb)

    # -- configuration -------------------------------------------------------
    def configure(self):
        c, p, T = self.c, self.p, self.target
        name = p["name"]
        for d in ("dev", "proc", "sys", "run"):
            os.makedirs(f"{T}/{d}", exist_ok=True)
        for src in ("dev", "proc", "sys"):
            self.run(["mount", "--rbind", "/" + src, f"{T}/{src}"], check=False)
            self.run(["mount", "--make-rslave", f"{T}/{src}"], check=False)
        host = c["hostname"].lower()
        short = host.split(".")[0]
        self.write("/etc/hostname", short + "\n")
        self.write("/etc/conf.d/hostname", f'hostname="{short}"\n')
        self.write("/etc/hosts", f"127.0.0.1 localhost\n::1 localhost\n127.0.1.1 {host} {short}\n")
        self.write("/etc/timezone", c["timezone"] + "\n")
        if os.path.exists(f"{T}/usr/share/zoneinfo/{c['timezone']}"):
            lt = f"{T}/etc/localtime"
            if os.path.lexists(lt):
                os.unlink(lt)
            os.symlink(f"/usr/share/zoneinfo/{c['timezone']}", lt)
        self.write("/etc/conf.d/keymaps", f'keymap="{c["keymap"]}"\n')
        self.write("/etc/env.d/02locale", f'LANG="{c["locale"]}"\n')
        self.run(["env-update"], chroot=True, check=False)
        if self.zfs():
            # the pool was created on the live system: give the installed system the same host id
            if os.path.exists("/etc/hostid"):
                shutil.copy("/etc/hostid", f"{T}/etc/hostid")
            for svc in ("zfs-import", "zfs-mount"):
                self.run(["rc-update", "add", svc, "boot"], chroot=True, check=False)
        if c["locale"] not in ("en_US.UTF-8", "C.UTF-8"):
            self.write("/etc/locale.gen", f"{c['locale']} UTF-8\nen_US.UTF-8 UTF-8\n", append=True)
            self.run(["locale-gen"], chroot=True, check=False)

        # ---- accounts -----------------------------------------------------
        if c["root_hash"]:
            self.run(["chpasswd", "-e"], chroot=True, input_=f"root:{c['root_hash']}\n")
        elif c["root_password"]:
            self.run(["chpasswd"], chroot=True, input_=f"root:{c['root_password']}\n")
        else:
            self.run(["passwd", "-l", "root"], chroot=True)
        keys = [k.strip() for k in c["ssh_keys"] if k.strip()]
        if keys:
            self.write("/root/.ssh/authorized_keys", "\n".join(keys) + "\n", 0o600)
            os.chmod(f"{T}/root/.ssh", 0o700)

        # ---- sshd: key-only unless the operator opted in ---------------------
        cfgp = f"{T}/etc/ssh/sshd_config"
        txt = open(cfgp).read() if os.path.exists(cfgp) else ""
        txt = re.sub(r"(?m)^\s*#?\s*(PermitRootLogin|PasswordAuthentication|KbdInteractiveAuthentication|PermitEmptyPasswords)\b.*\n", "", txt)
        if c["ssh_password_auth"]:
            hard = "PermitRootLogin yes\nPasswordAuthentication yes\nKbdInteractiveAuthentication no\nPermitEmptyPasswords no\n"
        else:
            hard = "PermitRootLogin prohibit-password\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitEmptyPasswords no\n"
        # first match wins in sshd_config: put ours at the very top
        open(cfgp, "w").write("# set by live-installer\n" + hard + txt)
        os.makedirs(f"{T}/etc/ssh/sshd_config.d", exist_ok=True)
        for f in os.listdir(f"{T}/etc/ssh/sshd_config.d"):
            fp = f"{T}/etc/ssh/sshd_config.d/{f}"
            body = open(fp).read()
            if re.search(r"(?im)^\s*(PermitRootLogin|PasswordAuthentication)\b", body):
                open(fp, "w").write(re.sub(r"(?im)^\s*(PermitRootLogin|PasswordAuthentication)\b.*$", r"# \g<0>", body))
        self.run(["rc-update", "add", "sshd", "default"], chroot=True, check=False)

        # ---- network ----------------------------------------------------------------
        self.configure_network()

        # ---- web admin password (first boot) ------------------------------------------
        pf = p.get("admin_password_file")
        if c["web_password"] and pf:
            self.write(pf, c["web_password"] + "\n", 0o600)
            owner = p.get("admin_password_owner", "root")
            self.run(["chown", f"{owner}:{owner}" if owner != "root" else "root:root", pf], chroot=True, check=False)
            if p.get("admin_password_cleanup", True):
                self.write("/etc/local.d/50-installer-firstboot.start", FIRSTBOOT.format(port=p["web_port"], files=pf), 0o755)
        if c["email"]:
            self.write(f"/etc/{name}/notify-email", c["email"] + "\n", 0o644)
        # live-only services are not wanted on the installed system
        for s in ("live-installer",):
            self.run(["rc-update", "del", s, "default"], chroot=True, check=False)
        self.write("/etc/motd", self.motd())
        self.write("/etc/live-installer/installed", time.strftime("%FT%TZ", time.gmtime()) + "\n")
        for s in p["services"]:
            self.run(["rc-update", "add", s, "default"], chroot=True, check=False)
        # serial console keeps working when the installer was driven over a serial line
        if "console=ttyS0" in read("/proc/cmdline"):
            self.run(["rc-update", "add", "live-serial", "default"], chroot=True, check=False)

    def motd(self):
        return (f"\n  {self.p['title']} (installed {time.strftime('%F')})\n"
                f"  Web UI : {self.p['web_scheme']}://<this-ip>:{self.p['web_port']}   user: admin\n"
                f"  SSH    : key-only for root{' (password login enabled by the installer)' if self.c['ssh_password_auth'] else ''}\n\n")

    def configure_network(self):
        c, p = self.c, self.p
        dh = ["# written by live-installer"]
        wifi = c.get("wifi")
        mgmt = [("mgmt", "Management")] if p["network_mode"] != "wan-lan" else [("wan", "WAN"), ("lan", "LAN")]
        for key, _lbl in mgmt:
            n = c[key]
            if not n.get("iface"):
                continue
            if n.get("mode") == "static":
                dh += ["", f"interface {n['iface']}", f"static ip_address={n['address']}"]
                if n.get("gateway"):
                    dh.append(f"static routers={n['gateway']}")
                if n.get("dns"):
                    dh.append("static domain_name_servers=" + " ".join(re.split(r"[\s,]+", n["dns"].strip())))
        if wifi:
            dh += ["", "# Wi-Fi is configured by iwd (EnableNetworkConfiguration)", "denyinterfaces wlan* wlp*"]
        self.write("/etc/dhcpcd.conf", open(f"{self.target}/etc/dhcpcd.conf").read() + "\n".join(dh) + "\n") \
            if os.path.exists(f"{self.target}/etc/dhcpcd.conf") else self.write("/etc/dhcpcd.conf", "\n".join(dh) + "\n")
        if wifi:
            ssid = wifi["ssid"]
            fn = ssid if re.fullmatch(r"[A-Za-z0-9 _-]+", ssid) else "=" + ssid.encode().hex()
            sec = "psk" if wifi.get("psk") else "open"
            body = "[Security]\nPassphrase=" + wifi["psk"] + "\n" if wifi.get("psk") else ""
            self.write(f"/var/lib/iwd/{fn}.{sec}", body, 0o600)
            self.write("/etc/iwd/main.conf", "[General]\nEnableNetworkConfiguration=true\n")
            self.run(["rc-update", "add", "iwd", "default"], chroot=True, check=False)
        if p["network_mode"] == "wan-lan":
            self.write_roles()

    def write_roles(self):
        c = self.c
        path = f"{self.target}/etc/patronus/patronus.toml"
        txt = open(path).read() if os.path.exists(path) else ""
        wan = c["wan"].get("iface") or (c["wifi"] and c["wifi"].get("iface")) or ""
        lan = c["lan"].get("iface") or ""
        lines = [l for l in txt.splitlines() if not re.match(r"\s*(wan_interfaces?|lan_interfaces?)\s*=", l) and l.strip() != "# written by live-installer"]
        roles = ["# written by live-installer", f'wan_interface = "{wan}"' if wan else "", f'lan_interface = "{lan}"' if lan else ""]
        roles = [r for r in roles if r]
        out, placed = [], False
        for l in lines:
            out.append(l)
            if l.strip() == "[network]" and not placed:
                out += roles
                placed = True
        if not placed:
            out += ["", "[network]"] + roles
        open(path, "w").write("\n".join(out) + "\n")
        self.run(["chgrp", "patronus", "/etc/patronus/patronus.toml"], chroot=True, check=False)
        self.run(["chmod", "0640", "/etc/patronus/patronus.toml"], chroot=True, check=False)

    # -- boot loader ------------------------------------------------------------------
    def bootloader(self):
        c, p, T = self.c, self.p, self.target
        name = p["name"]
        L = self.layout(c["disks"][0])
        extra = "console=tty0 console=ttyS0,115200n8" if "console=ttyS0" in read("/proc/cmdline") else ""
        self.write("/etc/default/grub", "\n".join([
            f'GRUB_DISTRIBUTOR="{p["title"]}"', 'GRUB_TIMEOUT=3', f'GRUB_CMDLINE_LINUX="{extra}"',
            'GRUB_DISABLE_OS_PROBER=true',
        ] + (['GRUB_TERMINAL="console serial"', 'GRUB_SERIAL_COMMAND="serial --unit=0 --speed=115200"'] if extra else [])) + "\n")
        for dev in (L[1], L[2], L[4]):
            self.run(f"blkid -s UUID -o value {dev}", check=False)
        self.run("udevadm trigger --subsystem-match=block --action=change; udevadm settle --timeout=10; true", check=False)
        if self.efi:
            for i, disk in enumerate(c["disks"]):
                l = self.layout(disk)
                mnt = "/boot/efi" if i == 0 else f"/boot/efi{i + 1}"
                if i:
                    os.makedirs(f"{T}{mnt}", exist_ok=True)
                    self.run(["mount", l[1], T + mnt])
                self.run(["grub-install", "--target=x86_64-efi", f"--efi-directory={mnt}", f"--bootloader-id={p['title']}", "--removable"], chroot=True)
        else:
            for disk in c["disks"]:
                self.run(["grub-install", "--target=i386-pc", disk], chroot=True)
        fsfix = fsuuid(L[4]) if not self.zfs() else ""
        if self.zfs():
            self.write_zfs_grubcfg(L)
        else:
            self.run(["grub-mkconfig", "-o", "/boot/grub/grub.cfg"], chroot=True)
            if fsfix:
                self.run(["sed", "-i", f"s#root={L[4]}\\b#root=UUID={fsfix}#g", f"{T}/boot/grub/grub.cfg"], check=False)
            # a device name in root= would break on the next boot when disks reorder
            if self.run(["grep", "-q", f"root={L[4]}", f"{T}/boot/grub/grub.cfg"], check=False) == 0:
                raise InstallError("grub.cfg still names a device in root=")

    def write_zfs_grubcfg(self, L):
        T = self.target
        kver = sorted(f for f in os.listdir(f"{T}/lib/modules"))[-1]
        extra = "console=tty0 console=ttyS0,115200n8" if "console=ttyS0" in read("/proc/cmdline") else ""
        boot_uuid = fsuuid(L[2])
        serial = ("serial --unit=0 --speed=115200\nterminal_input console serial\nterminal_output console serial\n") if extra else ""
        cfg = f"""# generated by live-installer (root on ZFS: grub-mkconfig cannot probe it)
set timeout=3
{serial}insmod part_gpt
insmod ext2
search --no-floppy --fs-uuid --set=root {boot_uuid}
menuentry "{self.p['title']} (ZFS root)" {{
    linux /vmlinuz-{kver} root=ZFS=rpool/ROOT zfsforce=1 ro {extra}
    initrd /initramfs-{kver}.img
}}
"""
        self.write("/boot/grub/grub.cfg", cfg)
        self.log("wrote a static grub.cfg for ZFS root, kernel " + kver)

    def finish(self):
        T = self.target
        self.run("sync", check=False)
        for d in ("dev", "proc", "sys"):
            self.run(["umount", "-R", "-l", f"{T}/{d}"], check=False)
        self.run(f"umount -R {T}/boot 2>/dev/null; true", check=False)
        if self.zfs():
            self.run(["zfs", "umount", "-a"], check=False)
            self.run(["zpool", "export", "rpool"], check=False)
        else:
            self.run(["umount", "-R", T], check=False)
        self.log("install finished")


def fsuuid(dev):
    rc, out = sh(["blkid", "-s", "UUID", "-o", "value", dev])
    return out.strip() if rc == 0 else ""


FIRSTBOOT = """#!/bin/sh
# one-shot: remove the first-boot web-admin password file once the service is up
( n=0
  while [ $n -lt 300 ]; do
    if ss -ltn 2>/dev/null | grep -q ':{port} '; then
      sleep 5; rm -f {files}; rm -f /etc/local.d/50-installer-firstboot.start; exit 0
    fi
    sleep 2; n=$((n+2))
  done ) >/dev/null 2>&1 &
"""
