#!/usr/bin/env python3
"""Talk to a guest's QEMU guest agent socket from a test (stdlib only).

  hx_qga.py SOCK ping
  hx_qga.py SOCK exec [--timeout S] [--input TEXT] PATH [ARG...]   run a command in the guest; prints its stdout,
                                                                    then `EXIT=<code>` on the last line

Every call starts with the guest-sync-delimited handshake (a stale reply from an earlier session is skipped), the same
way the node does it (crates/horcrux-vm/src/vm/devices.rs). The guest agent can stall for a while around heavy guest
activity, so `exec` keeps asking for the status until its timeout.
"""
import base64
import json
import random
import socket
import sys
import time


def call(sock_path, command, arguments=None, timeout=15):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    s.connect(sock_path)
    f = s.makefile("rwb", buffering=0)
    sid = random.randrange(1, 1 << 30)
    f.write(b"\xff" + json.dumps({"execute": "guest-sync-delimited", "arguments": {"id": sid}}).encode() + b"\n")
    while True:
        line = f.readline()
        if not line:
            raise RuntimeError("guest agent channel closed")
        pos = line.rfind(b"\xff")
        if pos < 0:
            continue
        try:
            if json.loads(line[pos + 1:]).get("return") == sid:
                break
        except ValueError:
            continue
    f.write(json.dumps({"execute": command, "arguments": arguments or {}}).encode() + b"\n")
    while True:
        line = f.readline()
        if not line:
            raise RuntimeError("guest agent channel closed")
        try:
            v = json.loads(line)
        except ValueError:
            continue
        if "error" in v:
            raise RuntimeError(v["error"].get("desc", "guest agent error"))
        if "return" in v:
            s.close()
            return v["return"]


def run(sock_path, path, args, timeout=60, stdin=None):
    req = {"path": path, "arg": args, "capture-output": True}
    if stdin is not None:
        req["input-data"] = base64.b64encode(stdin.encode()).decode()
    pid = call(sock_path, "guest-exec", req)["pid"]
    end = time.time() + timeout
    while time.time() < end:
        time.sleep(0.5)
        try:
            st = call(sock_path, "guest-exec-status", {"pid": pid}, timeout=10)
        except (OSError, RuntimeError):
            continue
        if st.get("exited"):
            out = base64.b64decode(st.get("out-data", "") or "").decode(errors="replace")
            err = base64.b64decode(st.get("err-data", "") or "").decode(errors="replace")
            return st.get("exitcode", -1), out, err
    raise TimeoutError(f"{path} did not finish within {timeout} s")


def main(argv):
    sock, cmd, rest = argv[1], argv[2], argv[3:]
    if cmd == "ping":
        call(sock, "guest-ping")
        print("pong")
        return 0
    if cmd == "exec":
        timeout, stdin = 60, None
        while rest and rest[0] in ("--timeout", "--input"):
            if rest[0] == "--timeout":
                timeout = int(rest[1])
            else:
                stdin = rest[1]
            rest = rest[2:]
        code, out, err = run(sock, rest[0], rest[1:], timeout, stdin)
        sys.stdout.write(out)
        if err:
            sys.stderr.write(err)
        print(f"EXIT={code}")
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
