"""Virtual L2 networks between guests, in user space (no root, no bridges, no taps).

A Hub is a learning Ethernet switch that speaks QEMU's `-netdev socket,connect=HOST:PORT`
framing (4-byte big-endian length + frame).  Because every frame passes through Python
the test can inject faults on the wire: partitions, one-way blocks, loss, delay, and it
writes a pcap per network for failure artifacts.

Root-only alternative: `Network(kind="bridge", bridge="br0")` hands QEMU
`-netdev bridge` (needs qemu-bridge-helper); faults then have to be injected in the guests.
"""
from __future__ import annotations

import os
import random
import socket
import struct
import threading
import time


class _Port:
    def __init__(self, sock, addr):
        self.sock, self.addr = sock, addr
        self.node: str | None = None          # resolved from the source MAC of the first frame
        self.lock = threading.Lock()
        self.alive = True

    def send(self, frame: bytes):
        try:
            with self.lock:
                self.sock.sendall(struct.pack(">I", len(frame)) + frame)
        except OSError:
            self.alive = False


class Hub:
    def __init__(self, name: str, outdir: str, pcap: bool = True):
        self.name = name
        self.macs: dict[bytes, str] = {}          # registered MAC -> node name
        self.table: dict[bytes, _Port] = {}       # learned MAC -> port
        self.ports: list[_Port] = []
        self.blocked: set[tuple[str, str]] = set()  # (src node, dst node): frames dropped
        self.loss: dict = {}
        self.delay: dict = {}
        self.stats = {"forwarded": 0, "dropped": 0}
        self.events: list[str] = []
        self.lock = threading.Lock()
        self._stop = False
        self.srv = socket.socket()
        self.srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.srv.bind(("127.0.0.1", 0))
        self.srv.listen(16)
        self.port = self.srv.getsockname()[1]
        self.pcap = None
        if pcap:
            os.makedirs(outdir, exist_ok=True)
            self.pcap = open(os.path.join(outdir, f"net-{name}.pcap"), "wb")
            self.pcap.write(struct.pack("<IHHiIII", 0xA1B2C3D4, 2, 4, 0, 0, 65535, 1))
        threading.Thread(target=self._accept, daemon=True).start()

    # ---- topology / faults -------------------------------------------------------
    def register(self, mac: str, node: str):
        self.macs[bytes.fromhex(mac.replace(":", ""))] = node

    def _log(self, msg):
        self.events.append(f"{time.strftime('%H:%M:%S')} {self.name}: {msg}")

    def block(self, a: str, b: str, both: bool = True):
        with self.lock:
            self.blocked.add((a, b))
            if both:
                self.blocked.add((b, a))
        self._log(f"block {a}{' <-> ' if both else ' -> '}{b}")

    def unblock(self, a: str, b: str, both: bool = True):
        with self.lock:
            self.blocked.discard((a, b))
            if both:
                self.blocked.discard((b, a))
        self._log(f"unblock {a} {b}")

    def partition(self, *groups):
        """Split the network: nodes in different groups cannot exchange a single frame."""
        for i, g1 in enumerate(groups):
            for g2 in groups[i + 1:]:
                for a in g1:
                    for b in g2:
                        self.block(a, b)

    def isolate(self, node: str):
        for other in set(self.macs.values()) - {node}:
            self.block(node, other)

    def heal(self):
        with self.lock:
            self.blocked.clear()
            self.loss.clear()
            self.delay.clear()
        self._log("heal: all faults cleared")

    def set_loss(self, a: str, b: str, pct: float, both: bool = True):
        self.loss[(a, b)] = pct / 100.0
        if both:
            self.loss[(b, a)] = pct / 100.0
        self._log(f"loss {a}<->{b} {pct}%")

    def set_delay(self, a: str, b: str, ms: float, both: bool = True):
        self.delay[(a, b)] = ms / 1000.0
        if both:
            self.delay[(b, a)] = ms / 1000.0
        self._log(f"delay {a}<->{b} {ms}ms")

    # ---- wire ---------------------------------------------------------------------
    def _accept(self):
        while not self._stop:
            try:
                s, addr = self.srv.accept()
            except OSError:
                return
            p = _Port(s, addr)
            with self.lock:
                self.ports.append(p)
            threading.Thread(target=self._rx, args=(p,), daemon=True).start()

    def _recv_exact(self, s, n):
        buf = b""
        while len(buf) < n:
            c = s.recv(n - len(buf))
            if not c:
                return None
            buf += c
        return buf

    def _rx(self, p: _Port):
        while not self._stop:
            try:
                h = self._recv_exact(p.sock, 4)
                if not h:
                    break
                frame = self._recv_exact(p.sock, struct.unpack(">I", h)[0])
                if frame is None:
                    break
            except OSError:
                break
            if len(frame) >= 14:
                self._switch(p, frame)
        p.alive = False

    def _switch(self, src: _Port, frame: bytes):
        dst_mac, src_mac = frame[0:6], frame[6:12]
        with self.lock:
            self.table[src_mac] = src
            if src.node is None:
                src.node = self.macs.get(src_mac)
            if self.pcap:
                t = time.time()
                self.pcap.write(struct.pack("<IIII", int(t), int((t % 1) * 1e6), len(frame), len(frame)) + frame)
                self.pcap.flush()
            if dst_mac[0] & 1 or dst_mac not in self.table:
                targets = [x for x in self.ports if x is not src and x.alive]
            else:
                targets = [self.table[dst_mac]]
            blocked, loss, delay = set(self.blocked), dict(self.loss), dict(self.delay)
        for t in targets:
            key = (src.node, t.node)
            if key in blocked:
                self.stats["dropped"] += 1
                continue
            if loss.get(key) and random.random() < loss[key]:
                self.stats["dropped"] += 1
                continue
            self.stats["forwarded"] += 1
            d = delay.get(key)
            if d:
                threading.Timer(d, t.send, args=(frame,)).start()
            else:
                t.send(frame)

    def close(self):
        self._stop = True
        try:
            self.srv.close()
        except OSError:
            pass
        for p in self.ports:
            try:
                p.sock.close()
            except OSError:
                pass
        if self.pcap:
            self.pcap.close()
