#!/usr/bin/env python3
"""Tiny WebSocket client for the live system-log stream (stdlib only), run INSIDE a node.

  hx_logws.py follow "<filter query>" SECONDS    print every frame (one JSON object per line) until the stream
                                                 closes or SECONDS pass; exit 0
  Env: HX_API (default https://127.0.0.1:8006), HX_USER / HX_PASS (default admin / admin)
"""
import base64
import json
import os
import socket
import ssl
import struct
import sys
import time
import urllib.request
from urllib.parse import urlparse

API = os.environ.get("HX_API", "https://127.0.0.1:8006")
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE


def call(method, path, body=None, token=None):
    r = urllib.request.Request(API + path, data=None if body is None else json.dumps(body).encode(), method=method)
    r.add_header("Content-Type", "application/json")
    if token:
        r.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(r, context=CTX, timeout=60) as resp:
        t = resp.read().decode()
        return json.loads(t) if t.strip() else None


def read_exact(s, n):
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise EOFError
        buf += chunk
    return buf


def one_frame(s):
    """(opcode, payload) of the next server frame (unmasked)."""
    h = read_exact(s, 2)
    op, ln = h[0] & 0x0F, h[1] & 0x7F
    if ln == 126:
        ln = struct.unpack(">H", read_exact(s, 2))[0]
    elif ln == 127:
        ln = struct.unpack(">Q", read_exact(s, 8))[0]
    return op, read_exact(s, ln)


def follow(query, seconds):
    tok = call("POST", "/api/auth/login", {"username": os.environ.get("HX_USER", "admin"),
                                           "password": os.environ.get("HX_PASS", "admin")})
    token = tok.get("ticket") or tok.get("token")
    tk = call("POST", "/api/system/logs/stream/ticket", {}, token)
    path = tk["ws_path"] + ("?" + query if query else "")
    u = urlparse(API)
    raw = socket.create_connection((u.hostname, u.port or 443), timeout=30)
    s = CTX.wrap_socket(raw, server_hostname=u.hostname)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f"GET {path} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
               f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n").encode())
    head = b""
    while b"\r\n\r\n" not in head:
        head += s.recv(1)
    status = head.split(b"\r\n")[0].decode()
    if " 101 " not in status:
        print(json.dumps({"type": "http-error", "status": status, "body": head.decode(errors="replace")}), flush=True)
        return
    end = time.time() + seconds
    s.settimeout(1.0)
    while time.time() < end:
        try:
            op, payload = one_frame(s)
        except (socket.timeout, TimeoutError):
            continue
        except EOFError:
            break
        if op == 1:
            print(payload.decode(), flush=True)
        elif op == 8:
            break


if __name__ == "__main__":
    if len(sys.argv) < 4 or sys.argv[1] != "follow":
        sys.exit(__doc__)
    deadline = time.time() + float(sys.argv[3])
    # the socket timeout ends a quiet wait; keep going until the deadline
    while time.time() < deadline:
        try:
            follow(sys.argv[2], max(1.0, deadline - time.time()))
            break
        except Exception as e:  # noqa: BLE001
            print(json.dumps({"type": "client-error", "message": str(e)}), flush=True)
            break
