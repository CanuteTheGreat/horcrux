#!/usr/bin/env python3
"""Helper that runs INSIDE a node container of vm_migrate_e2e (stdlib only).

  hx_node.py api METHOD PATH [JSON]   call this node's API (or another node's
                                      with HX_API=https://ip:8006), print JSON
  hx_node.py serial SOCK CMD          log in to a CirrOS guest on its serial
                                      socket (if needed), run CMD, print output
  hx_node.py qmp SOCK CMD [JSON]      one QMP command, print the reply
"""
import json
import os
import socket
import ssl
import sys
import time
import urllib.error
import urllib.request

API = os.environ.get("HX_API", "https://127.0.0.1:8006")
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE


def req(method, path, body=None, token=None):
    data = None if body is None else json.dumps(body).encode()
    r = urllib.request.Request(API + "/api" + path, data=data, method=method)
    r.add_header("Content-Type", "application/json")
    if token:
        r.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(r, context=CTX, timeout=120) as resp:
            return resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def token():
    cache = "/tmp/hx-token-" + API.replace("/", "_").replace(":", "_")
    if os.path.exists(cache) and time.time() - os.path.getmtime(cache) < 3000:
        return open(cache).read().strip()
    st, body = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
    if st != 200:
        sys.exit(f"login {st}: {body}")
    j = json.loads(body)
    t = j.get("ticket") or j.get("token")
    open(cache, "w").write(t)
    return t


def api(method, path, body=None):
    st, text = req(method, path, body, token())
    print(json.dumps({"status": st, "body": json.loads(text) if text.strip().startswith(("{", "[")) else text}))


def serial(sock_path, cmd, timeout=600):
    """Run `cmd` in the guest; output between unique markers."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(sock_path)
    s.settimeout(2)
    buf = b""

    def read_for(sec):
        nonlocal buf
        end = time.time() + sec
        while time.time() < end:
            try:
                d = s.recv(65536)
                if not d:
                    break
                buf += d
            except socket.timeout:
                pass
        return buf

    def wait(pats, sec):
        # A pattern counts only at the very end of what the guest printed:
        # boot messages contain "# "/"$ " too, and typing a command at what
        # was really a login prompt makes it the user name.
        nonlocal buf
        end = time.time() + sec
        while time.time() < end:
            read_for(1)
            tail = buf.rstrip(b" \r\n")
            for p in pats:
                if tail.endswith(p.rstrip()):
                    return p
        return None

    deadline = time.time() + timeout
    text = ""
    for _attempt in range(3):
        # Get to a shell prompt: log in if the guest shows login:.
        while time.time() < deadline:
            buf = b""
            s.sendall(b"\n")
            got = wait([b"login:", b"$ ", b"# "], 10)
            if got == b"login:":
                s.sendall(b"cirros\n")
                if wait([b"assword:"], 30):
                    s.sendall(b"gocubsgo\n")
                    wait([b"$ "], 60)
                continue
            if got in (b"$ ", b"# "):
                break
        else:
            sys.exit("no shell prompt on " + sock_path)
        mark = "HX%d" % int(time.time() * 1000)
        buf = b""
        s.sendall(f"echo {mark}-B; {cmd}; echo {mark}-E\n".encode())
        end_pat = f"{mark}-E\r\n".encode()
        t_end = time.time() + 120
        while time.time() < t_end and end_pat not in buf:
            read_for(1)
            # Still a login prompt after all (getty restarted under us):
            # log in and send the command again.
            if b"login:" in buf and end_pat not in buf and buf.rstrip().endswith((b"login:", b"assword:")):
                break
        text = buf.decode(errors="replace")
        b = text.rfind(f"{mark}-B\r\n")
        e = text.rfind(f"{mark}-E\r\n")
        if b >= 0 and e >= 0:
            print(text[b + len(mark) + 4 : e].replace("\r", "").strip())
            return
        if buf.rstrip().endswith(b"assword:"):
            s.sendall(b"\n")  # abandon that bogus login
            time.sleep(3)
    sys.exit("no command output: " + text[-500:])


def qmp(sock_path, command, arguments=None):
    s = socket.socket(socket.AF_UNIX)
    s.connect(sock_path)
    f = s.makefile("rw")
    f.readline()
    f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n")
    f.flush()
    f.readline()
    msg = {"execute": command}
    if arguments:
        msg["arguments"] = arguments
    f.write(json.dumps(msg) + "\n")
    f.flush()
    while True:
        line = json.loads(f.readline())
        if "return" in line or "error" in line:
            s.close()
            print(json.dumps(line))
            return


if __name__ == "__main__":
    if sys.argv[1] == "api":
        api(sys.argv[2], sys.argv[3], json.loads(sys.argv[4]) if len(sys.argv) > 4 else None)
    elif sys.argv[1] == "qmp":
        qmp(sys.argv[2], sys.argv[3], json.loads(sys.argv[4]) if len(sys.argv) > 4 else None)
    elif sys.argv[1] == "serial":
        serial(sys.argv[2], sys.argv[3])
    else:
        sys.exit(__doc__)
