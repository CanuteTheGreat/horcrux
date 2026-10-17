#!/usr/bin/env python3
"""End-to-end LXC test (Proxmox `pct` parity) against a real horcrux-api.

Runs INSIDE a privileged container that has LXC installed (see
lxc_e2e.sh): every step goes through the HTTP API (the same calls the web
UI makes) and is then checked against reality: the generated LXC config,
cgroup values read inside the running container, the guest's network
(static IP over a tagged VLAN, DHCP from dnsmasq), mounts, features,
snapshots, the PTY web terminal and the console log WebSockets.

Environment:
  API (default https://127.0.0.1:8006/api), HORCRUX_CA_FILE, LXC_PATH (/var/lib/lxc),
  KEYCTL_PROBE: path (inside the bind-mount source) of a static binary that
  prints the errno of keyctl(KEYCTL_GET_KEYRING_ID) (0 = allowed).
"""
import base64
import json
import os
import re
import socket
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request

API = os.environ.get("API", "https://127.0.0.1:8006/api")
# The API serves HTTPS from the node-local CA: trust that CA when given
# (HORCRUX_CA_FILE), otherwise accept the self-signed certificate.
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL_CTX = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
LXC_PATH = os.environ.get("LXC_PATH", "/var/lib/lxc")
BIND_SRC = "/srv/e2e-bind"
STEP = [0]
TOKEN = [None]


def log(msg):
    print(f"[lxc-e2e] {msg}", flush=True)


def step(msg):
    STEP[0] += 1
    log(f"--- {STEP[0]}. {msg}")


def fail(msg):
    log(f"FAIL: {msg}")
    subprocess.run(["sh", "-c", """
        echo '== api log'; grep -v 'Failed to broadcast' /tmp/api.log | tail -40
        for c in $LXC_PATH/*/start.log; do echo "== $c"; tail -20 "$c"; done
        echo '== dnsmasq'; tail -20 /tmp/dnsmasq.log; echo '== bridge'; bridge vlan show; ip -br addr
        for c in $(lxc-ls -P $LXC_PATH --running); do
          echo "== inside $c"
          lxc-attach -P $LXC_PATH -n $c -- sh -c 'ip addr; ip route; cat /etc/network/interfaces; ps' 2>&1 | head -60
          tail -30 $LXC_PATH/$c/console.log
        done"""])
    sys.exit(1)


def check(cond, msg):
    if not cond:
        fail(msg)
    log(f"ok: {msg}")


def req(method, path, body=None, expect=200, timeout=900):
    data = None if body is None else json.dumps(body).encode()
    r = urllib.request.Request(API + path, data=data, method=method)
    r.add_header("Content-Type", "application/json")
    if TOKEN[0]:
        r.add_header("Authorization", "Bearer " + TOKEN[0])
    try:
        with urllib.request.urlopen(r, timeout=timeout, context=SSL_CTX) as resp:
            status, text = resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if expect is not None and status != expect:
        fail(f"{method} {path} -> {status} (expected {expect}): {text[:600]}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def sh(cmd, check_rc=True):
    p = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if check_rc and p.returncode != 0:
        fail(f"`{cmd}` failed ({p.returncode}): {p.stderr.strip()} {p.stdout.strip()}")
    return p.stdout.strip()


def exec_ct(ct, cmd, expect=200):
    _, body = req("POST", f"/containers/{ct}/exec", {"command": ["sh", "-c", cmd]}, expect=expect)
    return body["output"].strip() if expect == 200 else body


def wait_for(desc, fn, timeout=60):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        try:
            last = fn()
            if last:
                return last
        except SystemExit:
            raise
        except Exception as e:  # noqa: BLE001
            last = e
        time.sleep(2)
    fail(f"timed out waiting for {desc} (last: {last})")


def lxc_state(name):
    return sh(f"lxc-info -P {LXC_PATH} -n {name} -s", check_rc=False)


# ---- minimal WebSocket client (stdlib only) ----------------------------
def ws_connect(path):
    scheme, host, port = re.match(r"(https?)://([^:/]+):(\d+)", API).groups()
    s = socket.create_connection((host, int(port)), timeout=30)
    if scheme == "https":
        s = SSL_CTX.wrap_socket(s, server_hostname=host)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall(
        (
            f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\n"
            f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
            f"Authorization: Bearer {TOKEN[0]}\r\n\r\n"
        ).encode()
    )
    resp = b""
    while b"\r\n\r\n" not in resp:
        chunk = s.recv(1)
        if not chunk:
            break
        resp += chunk
    if b" 101 " not in resp.split(b"\r\n")[0]:
        fail(f"WebSocket upgrade for {path} failed: {resp[:200]!r}")
    return s


def ws_send(s, text):
    payload = text.encode()
    mask = os.urandom(4)
    hdr = bytearray([0x81])
    n = len(payload)
    if n < 126:
        hdr.append(0x80 | n)
    else:
        hdr.append(0x80 | 126)
        hdr += n.to_bytes(2, "big")
    s.sendall(bytes(hdr) + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))


def ws_read_until(s, pattern, timeout=60):
    """Read data frames until `pattern` (regex) appears; returns all text."""
    buf, text = b"", ""
    end = time.time() + timeout
    s.settimeout(2)
    while time.time() < end:
        try:
            chunk = s.recv(65536)
        except socket.timeout:
            continue
        if not chunk:
            break
        buf += chunk
        while len(buf) >= 2:
            op, ln = buf[0] & 0x0F, buf[1] & 0x7F
            off = 2
            if ln == 126:
                if len(buf) < 4:
                    break
                ln, off = int.from_bytes(buf[2:4], "big"), 4
            elif ln == 127:
                if len(buf) < 10:
                    break
                ln, off = int.from_bytes(buf[2:10], "big"), 10
            if len(buf) < off + ln:
                break
            data, buf = buf[off : off + ln], buf[off + ln :]
            if op in (1, 2):
                text += data.decode(errors="replace")
            elif op == 8:
                return text
        if re.search(pattern, text):
            return text
    fail(f"WebSocket never produced /{pattern}/; got: {text[-800:]!r}")


# ---- the test ----------------------------------------------------------
def main():
    step("login")
    _, body = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN[0] = body.get("ticket") or body.get("token")
    check(bool(TOKEN[0]), "got an API token")

    step("host info (GET /lxc/host)")
    _, host = req("GET", "/lxc/host")
    check(host["available"] and host["version"], f"LXC available: {host['version']}")
    br = {b["name"]: b for b in host["bridges"]}
    check("hxbr0" in br and br["hxbr0"]["vlan_aware"], "VLAN-aware bridge hxbr0 listed")

    step("template catalog (pveam available) - no hardcoded Alpine 3.19")
    _, cat = req("GET", "/lxc/templates/catalog?arch=amd64")
    alpine = [e for e in cat if e["distro"] == "alpine" and e["variant"] == "default"
              and re.fullmatch(r"\d+\.\d+", e["release"])]
    check(len(alpine) > 0, f"{len(cat)} catalog entries, {len(alpine)} alpine releases")
    rel = max(alpine, key=lambda e: tuple(int(x) for x in e["release"].split(".")))["release"]
    log(f"newest alpine release: {rel}")
    _, deb = req("GET", "/lxc/templates/catalog?arch=amd64&distro=debian")
    check(all(e["distro"] == "debian" for e in deb) and deb, "distro filter works")

    step(f"download alpine/{rel} (pveam download, sha256 verified)")
    _, t = req("POST", "/lxc/templates", {"distro": "alpine", "release": rel})
    check(re.fullmatch(r"[0-9a-f]{64}", t["sha256"]) is not None, f"template {t['id']} sha256 {t['sha256'][:12]}")
    tdir = sh("ls -d /tmp/lxc-e2e-data/lxc-templates/alpine/*/amd64/default")
    got = sh(f"sha256sum {tdir}/rootfs.tar.xz").split()[0]
    check(got == t["sha256"], "cached tarball matches the recorded checksum")
    _, lst = req("GET", "/lxc/templates")
    check(any(x["id"] == t["id"] for x in lst), "template listed (pveam list)")

    step("validation errors are 4xx, nothing created")
    base = {"id": "bad", "name": "bad", "runtime": "lxc", "memory": 128, "cpus": 1,
            "rootfs": f"alpine/{rel}", "status": "stopped"}
    req("POST", "/containers", {**base, "lxc": {"nets": [{"bridge": "hxbr0", "vlan": 5000}]}}, expect=422)
    req("POST", "/containers", {**base, "rootfs": "alpine"}, expect=422)
    req("POST", "/containers", {**base, "lxc": {"mounts": [{"path": "/x", "source": "/a b"}]}}, expect=422)
    req("POST", "/containers", {**base, "runtime": "docker", "rootfs": "busybox", "lxc": {}}, expect=422)
    check(not os.path.exists(f"{LXC_PATH}/bad"), "no half-created container left behind")

    step("create ct1: unprivileged, 1G disk, VLAN static NIC + DHCP NIC, bind + volume mounts, features")
    os.makedirs(BIND_SRC, exist_ok=True)
    with open(f"{BIND_SRC}/hello", "w") as f:
        f.write("from host\n")
    ct = {
        **base, "id": "ct1", "name": "ct1", "memory": 256, "cpus": 1,
        "lxc": {
            "unprivileged": True, "hostname": "ct-one", "swap_mb": 0, "cpu_limit": 0.5,
            "cpu_units": 200, "disk_gb": 1,
            "nets": [
                {"bridge": "hxbr0", "vlan": 20, "ip": "10.20.0.50/24", "gateway": "10.20.0.1"},
                {"bridge": "hxbr0", "ip": "dhcp"},
            ],
            "mounts": [
                {"path": "/mnt/host", "source": BIND_SRC, "read_only": True},
                {"path": "/data"},
            ],
            "features": {"nesting": True, "fuse": True},
            "onboot": True, "startup_order": 3, "nameserver": "10.20.0.1", "searchdomain": "e2e.lan",
            "ssh_public_keys": ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE2e e2e@horcrux"],
        },
    }
    _, c = req("POST", "/containers", ct)
    check(c["runtime"] == "lxc" and c["status"] == "stopped", "created, stopped")
    check(c["lxc"]["nets"][0]["hwaddr"] and c["lxc"]["mounts"][1]["volume"] == "mp0",
          "MACs and volume names assigned and returned")
    cfg = open(f"{LXC_PATH}/ct1/config").read()
    for line in [
        "lxc.cgroup2.cpu.max = 50000 100000",  # the old code wrote `cpu.max = <cpus>`
        "lxc.cgroup2.memory.max = 268435456",
        "lxc.cgroup2.memory.swap.max = 0",
        "lxc.cgroup2.cpu.weight = 200",
        "lxc.idmap = u 0 100000 65536",
        f"lxc.rootfs.path = loop:{LXC_PATH}/ct1/rootfs.raw",
        "lxc.net.0.veth.vlan.id = 20",
        "lxc.start.auto = 1",
        "lxc.start.order = 3",
        "lxc.include = /usr/share/lxc/config/alpine.common.conf",
        "lxc.include = /usr/share/lxc/config/alpine.userns.conf",
    ]:
        check(line in cfg.splitlines(), f"config has `{line}`")
    check(os.path.getsize(f"{LXC_PATH}/ct1/rootfs.raw") == 1 << 30, "1 GiB rootfs image")
    check("keyctl errno 38" in open(f"{LXC_PATH}/ct1/seccomp.conf").read(), "keyctl blocked by default")
    check(sh("grep -c '^root:100000:65536' /etc/subuid") == "1", "root subuid range ensured once")

    step("start ct1")
    _, c = req("POST", "/containers/ct1/start", {})
    check(c["status"] == "running" and "RUNNING" in lxc_state("ct1"), "running per lxc-info")

    step("inside ct1: idmap, hostname, cgroup limits")
    check(exec_ct("ct1", "id -u") == "0", "root inside")
    check(exec_ct("ct1", "cat /proc/self/uid_map").split() == ["0", "100000", "65536"], "uid 0 -> host 100000")
    check(exec_ct("ct1", "hostname") == "ct-one", "hostname set")
    check(exec_ct("ct1", "cat /sys/fs/cgroup/cpu.max") == "50000 100000", "cpu.max applied (0.5 cores)")
    check(exec_ct("ct1", "cat /sys/fs/cgroup/memory.max") == "268435456", "memory.max applied")
    check(exec_ct("ct1", "cat /sys/fs/cgroup/memory.swap.max") == "0", "no swap")
    check(exec_ct("ct1", "cat /sys/fs/cgroup/cpu.weight") == "200", "cpu.weight applied")

    step("network: static IP on VLAN 20 reaches the VLAN gateway; DHCP on the untagged NIC")
    wait_for("eth0 10.20.0.50", lambda: "10.20.0.50/24" in exec_ct("ct1", "ip -4 addr show eth0"))
    exec_ct("ct1", "ping -c1 -W3 10.20.0.1")
    log("ok: ping 10.20.0.1 (host side of VLAN 20) from ct1")
    check(re.search(r"\b20 PVID", sh("bridge vlan show")) is not None, "a veth port is PVID 20 on the bridge")
    dhcp = wait_for("DHCP lease on eth1", lambda: re.search(r"10\.77\.0\.1\d\d", exec_ct("ct1", "ip -4 addr show eth1")), 90)
    log(f"ok: eth1 leased {dhcp.group(0)} from dnsmasq")
    check("10.20.0.1" in exec_ct("ct1", "ip route show default"), "default route via the VLAN gateway")
    resolv = exec_ct("ct1", "cat /etc/resolv.conf")
    check("nameserver 10.20.0.1" in resolv and "search e2e.lan" in resolv, "resolv.conf written")
    check("e2e@horcrux" in exec_ct("ct1", "cat /root/.ssh/authorized_keys"), "ssh key installed")

    step("mounts: read-only bind, managed volume owned by container root")
    check(exec_ct("ct1", "cat /mnt/host/hello") == "from host", "bind mount visible")
    check("Read-only" in exec_ct("ct1", "touch /mnt/host/nope 2>&1 || true"), "bind mount is read-only")
    exec_ct("ct1", "echo vol > /data/f")
    st = os.stat(f"{LXC_PATH}/ct1/volumes/mp0/f")
    check(st.st_uid == 100000, "volume file owned by mapped root (uid 100000) on the host")

    step("features: fuse device, nesting mounts")
    check(exec_ct("ct1", "test -c /dev/fuse && echo yes") == "yes", "/dev/fuse present")
    check(exec_ct("ct1", "test -d /dev/.lxc/proc && echo yes") == "yes", "nesting proc/sys mounts present")
    probe = os.environ.get("KEYCTL_PROBE")
    if probe:
        check(exec_ct("ct1", f"/mnt/host/{os.path.basename(probe)}") == "38", "keyctl() -> ENOSYS while disabled")

    step("disk: 1 GiB ext4 root")
    size = int(exec_ct("ct1", "df -Pm / | tail -1 | awk '{print $2}'"))
    check(800 <= size <= 1024, f"root fs {size} MB")

    step("web terminal: lxc-attach on a PTY over the exec WebSocket")
    _, tk = req("POST", "/containers/ct1/exec-ticket", {"command": ["/bin/sh"], "runtime": "lxc"})
    s = ws_connect(tk["ws_path"])
    ws_read_until(s, r"[#$] ", timeout=30)  # wait for the shell prompt, like a user
    ws_send(s, "tty; echo pty-ok-$((40+2))\n")
    out = ws_read_until(s, r"pty-ok-42")
    check("/dev/pts/" in out or "/dev/console" not in out, "shell has a real tty")
    s.close()

    step("boot log: console log over the logs WebSocket")
    _, tk = req("POST", "/containers/ct1/logs-ticket?runtime=lxc", None)
    s = ws_connect(tk["ws_path"])
    out = ws_read_until(s, r"(?i)openrc|starting|networking", timeout=30)
    check(len(out) > 0, "console output streamed")
    s.close()

    step("pct set while running: limits applied live, the rest pending until reboot")
    _, view = req("GET", "/containers/ct1/lxc/config")
    opts = view["lxc"]
    opts["cpu_limit"] = 1.0
    opts["features"]["keyctl"] = True
    opts["hostname"] = "ct-renamed"
    _, view = req("PUT", "/containers/ct1/lxc/config", {"memory": 384, "lxc": opts})
    check(exec_ct("ct1", "cat /sys/fs/cgroup/memory.max") == str(384 << 20), "memory.max changed live")
    check(exec_ct("ct1", "cat /sys/fs/cgroup/cpu.max") == "100000 100000", "cpu.max changed live")
    check(set(view["pending"]) >= {"features", "hostname"}, f"pending: {view['pending']}")
    check("keyctl" not in open(f"{LXC_PATH}/ct1/seccomp.conf").read(), "seccomp policy rewritten")

    step("reboot applies pending changes")
    _, c = req("POST", "/containers/ct1/reboot", {"timeout": 30})
    check(c["status"] == "running" and "RUNNING" in lxc_state("ct1"), "running again")
    check(exec_ct("ct1", "hostname") == "ct-renamed", "new hostname after reboot")
    _, view = req("GET", "/containers/ct1/lxc/config")
    check(view["pending"] == [], "nothing pending")
    if probe:
        check(exec_ct("ct1", f"/mnt/host/{os.path.basename(probe)}") != "38", "keyctl() allowed after enabling the feature")

    step("resize root disk online 1G -> 2G")
    req("POST", "/containers/ct1/resize", {"disk_gb": 2})
    size = int(exec_ct("ct1", "df -Pm / | tail -1 | awk '{print $2}'"))
    check(size >= 1800, f"root fs grew to {size} MB while running")
    req("POST", "/containers/ct1/resize", {"disk_gb": 1}, expect=422)

    step("snapshot (frozen copy), change, rollback")
    exec_ct("ct1", "echo before > /root/state")
    _, snap = req("POST", "/containers/ct1/snapshots", {"name": "before", "comment": "e2e"})
    check(snap["name"] == "before", "snapshot taken while running")
    check("RUNNING" in lxc_state("ct1"), "thawed after the snapshot")
    exec_ct("ct1", "echo after > /root/state; touch /root/new-file")
    _, view = req("GET", "/containers/ct1/lxc/config")
    req("PUT", "/containers/ct1/lxc/config", {"memory": 512, "lxc": view["lxc"]})
    _, snaps = req("GET", "/containers/ct1/snapshots")
    check([x["name"] for x in snaps] == ["before"], "snapshot listed")
    req("POST", "/containers/ct1/snapshots", {"name": "1-bad"}, expect=422)
    _, c = req("POST", "/containers/ct1/snapshots/before/rollback", {})
    check(c["status"] == "running", "running after rollback")
    check(exec_ct("ct1", "cat /root/state") == "before", "rootfs content rolled back")
    check(exec_ct("ct1", "test -e /root/new-file && echo yes || echo no") == "no", "later file gone")
    _, view = req("GET", "/containers/ct1/lxc/config")
    check(view["memory"] == 384, "settings rolled back with the snapshot (memory 512 -> 384)")
    check(exec_ct("ct1", "cat /sys/fs/cgroup/memory.max") == str(384 << 20), "and applied on restart")
    req("DELETE", "/containers/ct1/snapshots/before", None, expect=204)
    _, snaps = req("GET", "/containers/ct1/snapshots")
    check(snaps == [], "snapshot deleted")

    step("clean shutdown (pct shutdown)")
    _, c = req("POST", "/containers/ct1/shutdown", {"timeout": 60})
    check(c["status"] == "stopped" and "STOPPED" in lxc_state("ct1"), "stopped cleanly")
    req("POST", "/containers/ct1/shutdown", {}, expect=422)

    step("full clone ct1 -> ct2 and start it")
    _, c2 = req("POST", "/containers/ct1/clone", {"target_id": "ct2", "target_name": "ct2", "snapshot": False})
    check(c2["id"] == "ct2" and c2["lxc"]["nets"][0]["hwaddr"] != c["lxc"]["nets"][0]["hwaddr"], "clone has new MACs")
    check(os.path.exists(f"{LXC_PATH}/ct2/volumes/mp0/f"), "volumes copied")
    req("POST", "/containers/ct2/start", {})
    check(exec_ct("ct2", "cat /data/f") == "vol", "clone runs with the copied volume")
    req("POST", "/containers/ct2/stop", {})
    check("STOPPED" in lxc_state("ct2"), "hard stop (pct stop)")
    req("DELETE", "/containers/ct2", None, expect=204)
    check(not os.path.exists(f"{LXC_PATH}/ct2"), "ct2 destroyed with its volumes")

    step("privileged container on a directory rootfs, no network")
    req("POST", "/containers", {**base, "id": "ct3", "name": "ct3", "lxc": {"unprivileged": False}})
    check(os.path.isdir(f"{LXC_PATH}/ct3/rootfs"), "directory rootfs")
    req("POST", "/containers/ct3/start", {})
    check(exec_ct("ct3", "cat /proc/self/uid_map").split() == ["0", "0", "4294967295"], "privileged: no id shift")
    req("POST", "/containers/ct3/resize", {"disk_gb": 2}, expect=422)
    req("POST", "/containers/ct3/stop", {})
    req("DELETE", "/containers/ct3", None, expect=204)

    if os.environ.get("LXC_E2E_DEBIAN", "1") == "1":
        step("another distro: debian/bookworm (systemd guest) with a static IP")
        req("POST", "/containers", {**base, "id": "deb", "name": "deb", "rootfs": "debian/bookworm",
                                     "lxc": {"nets": [{"bridge": "hxbr0", "ip": "10.77.0.60/24"}]}})
        meta = json.load(open(f"{LXC_PATH}/deb/horcrux.json"))
        log(f"debian guest network config system: {meta['guest_net']}, distro {meta['distro']}")
        check("lxc.include = /usr/share/lxc/config/debian.common.conf" in open(f"{LXC_PATH}/deb/config").read(),
              "debian config include")
        req("POST", "/containers/deb/start", {})
        wait_for("debian static IP", lambda: "10.77.0.60/24" in exec_ct("deb", "ip -4 addr show eth0"), 90)
        log("ok: debian guest has 10.77.0.60")
        exec_ct("deb", "ping -c1 -W3 10.77.0.1")
        log("ok: debian guest reaches the bridge")
        req("POST", "/containers/deb/shutdown", {"timeout": 60, "force": True})
        req("DELETE", "/containers/deb", None, expect=204)

    step("backup a running LXC container and restore it as a new one (vzdump/pct restore)")
    req("POST", "/containers/ct1/start", {})
    marker = f"lxc-journey-{os.getpid()}"
    exec_ct("ct1", f"echo {marker} > /root/marker; sync")
    _, bk = req("POST", "/backups", {"target_id": "ct1", "mode": "suspend", "compression": "zstd"}, expect=201)
    check("RUNNING" in lxc_state("ct1"), "suspend-mode backup leaves ct1 running")
    _, v = req("POST", f"/backups/{bk['id']}/verify", {})
    check(v["ok"] is True, "backup verifies")
    exec_ct("ct1", "echo changed > /root/marker; sync")
    _, r = req("POST", f"/backups/{bk['id']}/restore", {"target_id": "ct1r", "name": "ct1r"})
    check(os.path.isdir(f"{LXC_PATH}/ct1r"), "restored container created")
    req("POST", "/containers/ct1r/start", {})
    check(exec_ct("ct1r", "cat /root/marker") == marker, "restored container has the data as of the backup")
    check(exec_ct("ct1", "cat /root/marker") == "changed", "original untouched by the restore")
    req("POST", "/containers/ct1r/stop", {})
    req("DELETE", "/containers/ct1r", None, expect=204)
    req("DELETE", f"/backups/{bk['id']}", None, expect=204)

    def restart_api(signal):
        sh(f"kill -{signal} $(cat /tmp/api.pid); sleep 2")
        sh("bash /tmp/start-api.sh")
        TOKEN[0] = None
        _, body = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
        TOKEN[0] = body.get("ticket") or body.get("token")

    step("a running container is untouched by API restarts (SIGTERM and SIGKILL)")
    init_pid = sh("lxc-info -n ct1 -P " + LXC_PATH + " -p -H").strip()
    check(init_pid.isdigit(), f"ct1 init pid {init_pid!r}")
    for signal in ("TERM", "KILL"):
        restart_api(signal)
        check("RUNNING" in lxc_state("ct1"), f"SIG{signal}: ct1 still running")
        check(sh("lxc-info -n ct1 -P " + LXC_PATH + " -p -H").strip() == init_pid,
              f"SIG{signal}: same init process")
        _, lst = req("GET", "/containers")
        row = [x for x in lst if x["id"] == "ct1"]
        check(row and row[0]["status"] == "running", f"SIG{signal}: API reports ct1 running")
        check(exec_ct("ct1", "cat /root/marker") == "changed", f"SIG{signal}: exec still works")
    req("POST", "/containers/ct1/shutdown", {"timeout": 60, "force": True})

    step("settings survive an API restart (horcrux.json + reconcile)")
    restart_api("TERM")
    _, lst = req("GET", "/containers")
    ct1 = [x for x in lst if x["id"] == "ct1"]
    check(ct1 and ct1[0]["runtime"] == "lxc" and ct1[0]["lxc"]["hostname"] == "ct-renamed"
          and ct1[0]["memory"] == 384, "ct1 listed with its settings after restart")

    step("delete ct1 and the template")
    req("DELETE", "/containers/ct1", None, expect=204)
    check(not os.path.exists(f"{LXC_PATH}/ct1"), "ct1 gone")
    _, lst = req("GET", "/containers")
    check(not any(x["id"] == "ct1" for x in lst), "ct1 no longer listed")
    req("DELETE", "/lxc/templates/" + t["id"], None, expect=204)
    _, lst = req("GET", "/lxc/templates")
    check(not any(x["id"] == t["id"] for x in lst), "template deleted")

    log(f"ALL {STEP[0]} STEPS PASSED")


if __name__ == "__main__":
    main()
