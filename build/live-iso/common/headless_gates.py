"""Headless-first gates shared by every server/firewall image (Horcrux server, Patronus), on every architecture.

Owner rule 2026-11-03: the product must work on a headless server (console + SSH + web UI); a desktop is an optional add-on.
qemu-test.py imports this module and runs these checks on the INSTALLED system when `--headless-gates` is given:

  * months-off hardware clock: every QEMU boot (live and installed) starts with an RTC `MONTHS_OFF` days in the past;
    the installer must have written a good RTC, chrony must step the system clock once the network is up;
  * late network: the installed system boots with its NICs' links DOWN (QMP set_link) until the login prompt appeared,
    then they come up: the boot must not wait for the network, and clock, DHCP lease and web UI must follow;
  * no GUI stack: no X/Wayland/toolkit/desktop package in the Portage database, no display manager or compositor
    running or enabled (a vdb scan is the closure of @world on an installed box: the Portage tree is not shipped there);
  * accounts: no empty password, no autologin on a console, the web admin password is not the root/login password.
Pure functions: `sh(cmd) -> (rc, out)` and `step(name, ok, info)` are passed in, so tests/test_headless_gates.py needs no VM.
"""
import base64
import datetime
import hashlib
import re
import subprocess
import time

MONTHS_OFF = 120          # days

# packages that must NOT exist on a headless install (fnmatch on category/name)
GUI_DENY = [
    "x11-base/*", "x11-wm/*", "x11-drivers/*", "gui-wm/*", "gui-libs/gtk*", "gui-libs/libadwaita*", "x11-libs/gtk+*",
    "x11-libs/libX11", "x11-libs/libXext", "x11-libs/libXrender", "x11-libs/libxcb", "x11-libs/cairo", "x11-libs/pango",
    "gnome-base/*", "gnome-extra/*", "kde-*/*", "xfce-*/*", "lxde-*/*", "media-libs/mesa", "media-libs/libglvnd",
    "dev-python/pygobject", "dev-python/pycairo", "app-accessibility/*", "x11-misc/lightdm*", "x11-misc/sddm*",
    "sys-auth/seatd", "media-fonts/dejavu", "x11-themes/*", "gui-apps/*", "www-client/*", "app-text/evince*",
]
GUI_PROCS = ("Xorg", "Xwayland", "sway", "cage", "gnome-shell", "gdm", "lightdm", "sddm", "weston", "seatd", "xfce4-session")
GUI_SERVICES = ("xdm", "gdm", "lightdm", "sddm", "display-manager", "seatd", "elogind-gui")

GUI_CHECK = r'''
import fnmatch, os, subprocess
deny = %(deny)r
procs = %(procs)r
svcs = %(svcs)r
pk = []
for cat in sorted(os.listdir("/var/db/pkg")):
    d = os.path.join("/var/db/pkg", cat)
    if os.path.isdir(d):
        for pf in sorted(os.listdir(d)):
            pk.append(cat + "/" + pf)
def name(x):
    c, pf = x.split("/", 1)
    import re
    m = re.match(r"^(.*?)-[0-9][^-]*(-r[0-9]+)?$", pf)
    return c + "/" + (m.group(1) if m else pf)
bad = sorted({name(x) for x in pk if any(fnmatch.fnmatch(name(x), p) for p in deny)})
print("PKG-COUNT", len(pk))
print("GUI-PACKAGES", len(bad), " ".join(bad[:25]))
ps = subprocess.run(["ps", "-eo", "comm"], capture_output=True, text=True).stdout.split()
run = sorted({p for p in ps if p in procs})
print("GUI-PROCESSES", len(run), " ".join(run))
rc = subprocess.run("rc-update show -v 2>/dev/null | awk '{print $1}'", shell=True, capture_output=True, text=True).stdout.split()
en = sorted({s for s in rc if s in svcs})
print("GUI-SERVICES", len(en), " ".join(en))
bins = [b for b in ("/usr/bin/Xorg", "/usr/bin/Xwayland", "/usr/bin/sway", "/usr/bin/cage", "/usr/bin/gnome-shell", "/usr/bin/startx") if os.path.exists(b)]
print("GUI-BINARIES", len(bins), " ".join(bins))
print("GUI-DONE")
''' % dict(deny=GUI_DENY, procs=GUI_PROCS, svcs=GUI_SERVICES)

ACCOUNT_CHECK = r'''
import subprocess, re
sh = open("/etc/shadow").read().splitlines()
empty = [l.split(":")[0] for l in sh if l.split(":")[1] == ""]
print("EMPTY-PASSWORDS", len(empty), " ".join(empty))
root = [l.split(":")[1] for l in sh if l.startswith("root:")]
print("ROOT-HASH", root[0] if root else "")
ps = subprocess.run(["ps", "-eo", "args"], capture_output=True, text=True).stdout.splitlines()
ag = [l.strip() for l in ps if "agetty" in l and "grep" not in l]
auto = [l for l in ag if re.search(r"(^|\s)(-a\s+\S+|--autologin\b|-n(\s|$)|--skip-login\b)", l)]
print("AGETTY", len(ag), "AUTOLOGIN-OR-NOLOGIN", len(auto), " | ".join(auto)[:200])
inittab = [l for l in open("/etc/inittab").read().splitlines() if re.search(r"agetty.*(autologin|\s-a\s)", l) and not l.startswith("#")]
print("INITTAB-AUTOLOGIN", len(inittab))
users = [l.split(":")[0] for l in open("/etc/passwd").read().splitlines() if l.split(":")[6] not in ("/sbin/nologin", "/bin/false", "/usr/sbin/nologin", "") and l.split(":")[0] != "root" and not l.split(":")[0].startswith("sync") and l.split(":")[0] not in ("shutdown", "halt")]
print("LOGIN-USERS-OTHER-THAN-ROOT", len(users), " ".join(users))
print("ACCOUNT-DONE")
'''


def b64(interp, text):
    return f"echo {base64.b64encode(text.encode()).decode()} | base64 -d | {interp}"


def months_off_base(now=None, days=MONTHS_OFF):
    """QEMU `-rtc base=` value: `days` before now (UTC)"""
    now = now or datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)
    return (now - datetime.timedelta(days=days)).replace(microsecond=0).isoformat()


def _kv(out, key):
    m = re.search(rf"(?m)^{key} (\d+)(?: (.*))?$", out.replace("\r", ""))
    return (int(m.group(1)), (m.group(2) or "").strip()) if m else (None, "")


def gui_check(sh, step):
    rc, o = sh(b64("python3", GUI_CHECK))
    n_pk, pk = _kv(o, "GUI-PACKAGES")
    n_pr, pr = _kv(o, "GUI-PROCESSES")
    n_sv, sv = _kv(o, "GUI-SERVICES")
    n_bn, bn = _kv(o, "GUI-BINARIES")
    cnt, _ = _kv(o, "PKG-COUNT")
    ok = "GUI-DONE" in o and n_pk == 0 and n_pr == 0 and n_sv == 0 and n_bn == 0
    info = f"{cnt} packages in the database; GUI packages {n_pk} {pk} | processes {n_pr} {pr} | enabled services {n_sv} {sv} | binaries {n_bn} {bn}"
    step("no-gui-stack-installed-headless-first-class", ok, re.sub(r"\s+", " ", info)[:400])
    return ok


# Portage admin tools every Gentoo install carries (app-portage/gentoolkit, owner 2026-11-12); a missing one fails the gate
ADMIN_TOOLS = ("equery", "eclean", "revdep-rebuild", "euse")


def admin_tools_gate(sh, step):
    """gentoolkit is installed and its tools run on the INSTALLED system (every project, every variant: ssh_checks calls this)"""
    cmd = "for t in " + " ".join(ADMIN_TOOLS) + "; do command -v $t >/dev/null 2>&1 && echo TOOL-OK $t || echo TOOL-MISSING $t; done; " \
          "equery --version 2>&1 | head -1; qlist -Iv app-portage/gentoolkit 2>&1 | head -1; echo ADMIN-DONE"
    rc, o = sh(cmd)
    o = (o or "").replace("\r", "")
    missing = re.findall(r"(?m)^TOOL-MISSING (\S+)", o)
    okn = len(re.findall(r"(?m)^TOOL-OK \S+", o))
    ok = "ADMIN-DONE" in o and not missing and okn == len(ADMIN_TOOLS) and "app-portage/gentoolkit-" in o
    step("gentoolkit-installed-equery-eclean-revdep-rebuild-euse", ok, re.sub(r"\s+", " ", f"missing {missing}; {o.strip()[-160:]}")[:300])
    return ok


def verifies(secret, shash):
    """True when the crypt hash `shash` ($1$/$5$/$6$ with plain salt) is the hash of `secret` (openssl reads the secret from stdin: never in argv)."""
    m = re.match(r"^\$(1|5|6)\$(?:rounds=\d+\$)?([^$]{1,16})\$", shash or "")
    if not m or "rounds=" in shash:
        return None
    try:
        r = subprocess.run(["openssl", "passwd", "-" + m.group(1), "-salt", m.group(2), "-stdin"], input=secret + "\n", capture_output=True, text=True, timeout=20)
    except (OSError, subprocess.SubprocessError):
        return None
    return r.stdout.strip() == shash if r.returncode == 0 else None


def account_check(sh, step, web_secret="", other_secrets=(), console_open=False):
    """no empty passwords, no autologin; no account's hash equals the web admin password or another known secret"""
    rc, o = sh(b64("python3", ACCOUNT_CHECK))
    n_e, e = _kv(o, "EMPTY-PASSWORDS")
    m = re.search(r"AGETTY (\d+) AUTOLOGIN-OR-NOLOGIN (\d+) ?(.*)", o)
    n_auto = int(m.group(2)) if m else None
    n_it, _ = _kv(o, "INITTAB-AUTOLOGIN")
    ok = "ACCOUNT-DONE" in o and n_e == 0 and n_it == 0 and (n_auto == 0 or console_open)
    step("no-empty-password-no-console-autologin", ok,
         re.sub(r"\s+", " ", f"empty passwords {n_e} {e}; agetty autologin/no-login {n_auto}{' (console menu opened on request)' if console_open and n_auto else ''} {m.group(3) if m else ''}; inittab autologin {n_it}")[:300])
    rh = re.search(r"(?m)^ROOT-HASH (\S*)", o.replace("\r", ""))
    rhash = rh.group(1) if rh else ""
    reused = []
    for label, sec in [("web-admin", web_secret)] + list(other_secrets):
        if sec and rhash:
            v = verifies(sec, rhash)
            if v:
                reused.append(label)
            elif v is None and not rhash.startswith(("!", "*")):
                reused.append(label + "?unverifiable")
    step("root-password-is-not-the-web-admin-or-disk-secret", not reused, "root's shadow hash does not verify against: " + ", ".join(l for l, s in [("web-admin", web_secret)] + list(other_secrets) if s) if not reused else "REUSED: " + ", ".join(reused))
    return ok and not reused


def link(vm, names, up):
    """QMP set_link on the user-mode netdevs (also while the VM is paused with -S)"""
    out = []
    for n in names:
        out.append(vm.qmp("set_link", name=n, up=up).strip()[:80])
    return out


def clock_gate(sh, step, host_now=time.time, timeout=480, label="months-off-rtc-clock-stepped-by-chrony-after-the-late-network"):
    """the guest booted with an RTC months off: after the network came up its clock must be within 120 s of the host's"""
    rc, o = sh("chronyc -n tracking 2>&1 | head -3; grep -h '^makestep' /etc/chrony/chrony.conf 2>/dev/null; grep -hc '^rtcsync' /etc/chrony/chrony.conf 2>/dev/null; rc-update show default boot | grep -E 'chronyd|hwclock|swclock'")
    cfg = re.sub(r"\s+", " ", o).strip()[:200]
    rc, o = sh("date -u +%s")
    try:
        first = int(o.strip().splitlines()[-1])
    except (ValueError, IndexError):
        first = 0
    t0, ok, delta = time.time(), False, None
    end = t0 + timeout
    while time.time() < end:
        rc, o = sh("date -u +%s")
        try:
            delta = int(o.strip().splitlines()[-1]) - int(host_now())
        except (ValueError, IndexError):
            time.sleep(5)
            continue
        if abs(delta) < 120:
            ok = True
            break
        time.sleep(8)
    rc, ch = sh("chronyc -n sources 2>&1 | head -6; chronyc -n tracking 2>&1 | head -3")
    off_days = (int(host_now()) - first) // 86400 if first else "?"
    step(label, ok, f"clock was {off_days} days off at the first look, after {int(time.time() - t0)} s the offset is {delta} s | {cfg} | " + re.sub(r"\s+", " ", ch)[:160])
    return ok


# Docker (>= 28) sets the iptables FORWARD policy to DROP; every other base chain also sees forwarded packets, so libvirt/Horcrux NAT VMs lost the internet
# (HOGSMEADE 2026-11-12). horcrux-core ships /etc/docker/daemon.json {"ip-forward-no-drop": true}. The netns stands in for a NAT-network VM: a veth with
# its own accept+masquerade nft table (what libvirt installs for virbr0) must reach the outside (10.0.2.2, the QEMU user-net gateway) WHILE dockerd runs.
DOCKER_FWD_CHECK = r'''
cfg=0; grep -Eq '"ip-forward-no-drop"[[:space:]]*:[[:space:]]*true' /etc/docker/daemon.json 2>/dev/null && cfg=1
echo "CFG $cfg"
if ! command -v dockerd >/dev/null 2>&1; then echo "DOCKER-ABSENT"; echo "FWD-DONE"; exit 0; fi
rc-service docker status >/dev/null 2>&1 || rc-service docker start >/dev/null 2>&1
n=0; until docker info >/dev/null 2>&1 || [ $n -ge 60 ]; do n=$((n+1)); sleep 1; done
docker info >/dev/null 2>&1 && echo "DOCKER-UP" || echo "DOCKER-DOWN"
echo "POLICY $( (nft list chain ip filter FORWARD 2>/dev/null || iptables -S FORWARD 2>/dev/null) | grep -Eo 'policy [a-z]+|-P FORWARD [A-Z]+' | head -1)"
cleanup() { ip netns del hxfwdt 2>/dev/null; ip link del hxfwdt-h 2>/dev/null; nft delete table ip hxfwdtest 2>/dev/null; }
cleanup
echo 1 > /proc/sys/net/ipv4/ip_forward
ip netns add hxfwdt && ip link add hxfwdt-h type veth peer name hxfwdt-n && ip link set hxfwdt-n netns hxfwdt \
 && ip addr add 192.168.251.1/24 dev hxfwdt-h && ip link set hxfwdt-h up \
 && ip -n hxfwdt addr add 192.168.251.2/24 dev hxfwdt-n && ip -n hxfwdt link set hxfwdt-n up && ip -n hxfwdt link set lo up \
 && ip -n hxfwdt route add default via 192.168.251.1
nft -f - <<NFT
table ip hxfwdtest {
  chain hxfwd_forward { type filter hook forward priority 0; policy accept; iifname "hxfwdt-h" accept; oifname "hxfwdt-h" ct state established,related accept; }
  chain hxfwd_nat { type nat hook postrouting priority 100; ip saddr 192.168.251.0/24 oifname != "hxfwdt-h" masquerade; }
}
NFT
ip netns exec hxfwdt ping -c 3 -W 3 10.0.2.2 >/dev/null 2>&1 && echo "NETNS-PING-OK" || echo "NETNS-PING-FAIL"
# the other order: dockerd (re)started AFTER the VM bridge exists (a restart, a Docker upgrade) must not drop it either
rc-service docker restart >/dev/null 2>&1
n=0; until docker info >/dev/null 2>&1 || [ $n -ge 60 ]; do n=$((n+1)); sleep 1; done
echo "POLICY-AFTER-RESTART $( (nft list chain ip filter FORWARD 2>/dev/null || iptables -S FORWARD 2>/dev/null) | grep -Eo 'policy [a-z]+|-P FORWARD [A-Z]+' | head -1)"
ip netns exec hxfwdt ping -c 3 -W 3 10.0.2.2 >/dev/null 2>&1 && echo "NETNS-PING-AFTER-RESTART-OK" || echo "NETNS-PING-AFTER-RESTART-FAIL"
cleanup
echo "FWD-DONE"
'''


def docker_forward_gate(sh, step):
    """horcrux-core's daemon.json keeps Docker from dropping forwarded traffic; with Docker installed a NAT-style netns still reaches the outside"""
    rc, o = sh(b64("sh", DOCKER_FWD_CHECK), 180)
    o = (o or "").replace("\r", "")
    cfg = re.search(r"(?m)^CFG 1$", o) is not None
    absent = "DOCKER-ABSENT" in o
    ok = "FWD-DONE" in o and cfg and (absent or ("DOCKER-UP" in o and "NETNS-PING-OK" in o and "NETNS-PING-AFTER-RESTART-OK" in o and not re.search(r"(?mi)^POLICY(-AFTER-RESTART)? .*drop", o)))
    step("docker-does-not-break-vm-nat-forwarding", ok, re.sub(r"\s+", " ", o.strip())[-300:] if not absent or not ok else "docker not installed in this image; daemon.json ip-forward-no-drop shipped")
    return ok
