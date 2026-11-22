#!/usr/bin/env python3
"""Type keys into the focused window of a Wayland compositor through the
virtual-keyboard protocol (zwp_virtual_keyboard_v1; sway, cage, labwc and
Mutter-less wlroots compositors serve it), with REAL evdev key codes.

`wtype` is not used on purpose: it builds a private keymap whose keycodes
follow the order the characters first appear in, so a client that reads
physical keys (horcrux-viewer sends the physical key to the guest) would
see the wrong keys. Here the keymap is the stock `evdev + us` one and the
key codes are the Linux input codes a real keyboard sends.

Pure standard library: the Wayland wire protocol is a few dozen lines.

    wl_keys.py [--delay-ms N] TEXT...        (\\n = Enter)
Env: WAYLAND_DISPLAY, XDG_RUNTIME_DIR.
"""
import array
import os
import socket
import struct
import sys
import tempfile
import time

KEYMAP = b"""xkb_keymap {
  xkb_keycodes { include "evdev+aliases(qwerty)" };
  xkb_types { include "complete" };
  xkb_compat { include "complete" };
  xkb_symbols { include "pc+us+inet(evdev)" };
};
\0"""

# Linux input event codes (input-event-codes.h)
KEY_LEFTSHIFT = 42
LETTERS = dict(zip("qwertyuiop", range(16, 26)))
LETTERS.update(zip("asdfghjkl", range(30, 39)))
LETTERS.update(zip("zxcvbnm", range(44, 51)))
DIGITS = dict(zip("1234567890", range(2, 12)))
# char -> (needs_shift, code)
PUNCT = {
    " ": (False, 57), "\n": (False, 28), "\t": (False, 15),
    "-": (False, 12), "=": (False, 13), "[": (False, 26), "]": (False, 27),
    ";": (False, 39), "'": (False, 40), "`": (False, 41), "\\": (False, 43),
    ",": (False, 51), ".": (False, 52), "/": (False, 53),
    "_": (True, 12), "+": (True, 13), "{": (True, 26), "}": (True, 27),
    ":": (True, 39), '"': (True, 40), "~": (True, 41), "|": (True, 43),
    "<": (True, 51), ">": (True, 52), "?": (True, 53),
    "!": (True, 2), "@": (True, 3), "#": (True, 4), "$": (True, 5), "%": (True, 6),
    "^": (True, 7), "&": (True, 8), "*": (True, 9), "(": (True, 10), ")": (True, 11),
}


def key_for(ch):
    if ch in PUNCT:
        return PUNCT[ch]
    if ch in DIGITS:
        return (False, DIGITS[ch])
    if ch.lower() in LETTERS:
        return (ch.isupper(), LETTERS[ch.lower()])
    raise ValueError(f"no key for {ch!r}")


class Wl:
    def __init__(self):
        path = os.environ["WAYLAND_DISPLAY"]
        if not path.startswith("/"):
            path = os.path.join(os.environ["XDG_RUNTIME_DIR"], path)
        self.s = socket.socket(socket.AF_UNIX)
        self.s.connect(path)
        self.buf = b""
        self.next_id = 2
        self.globals = {}

    def new_id(self):
        i = self.next_id
        self.next_id += 1
        return i

    def send(self, obj, opcode, args=b"", fds=()):
        msg = struct.pack("<II", obj, ((8 + len(args)) << 16) | opcode) + args
        if fds:
            self.s.sendmsg([msg], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", fds))])
        else:
            self.s.sendall(msg)

    def events(self):
        """Read until at least one complete event; yield (obj, opcode, payload)."""
        data = self.s.recv(65536)
        if not data:
            raise SystemExit("wl_keys: compositor closed the connection")
        self.buf += data
        while len(self.buf) >= 8:
            obj, w = struct.unpack_from("<II", self.buf)
            size, opcode = w >> 16, w & 0xFFFF
            if len(self.buf) < size:
                break
            payload, self.buf = self.buf[8:size], self.buf[size:]
            if obj == 1 and opcode == 0:
                oid, code, msg = struct.unpack_from("<III", payload)
                raise SystemExit(f"wl_keys: protocol error on object {oid} code {code}: "
                                 f"{payload[12:12 + msg].rstrip(chr(0).encode()).decode(errors='replace')}")
            yield obj, opcode, payload

    @staticmethod
    def string(s):
        b = s.encode() + b"\0"
        return struct.pack("<I", len(b)) + b + b"\0" * (-len(b) % 4)

    def roundtrip(self):
        cb = self.new_id()
        self.send(1, 0, struct.pack("<I", cb))
        while True:
            for obj, opcode, payload in self.events():
                if obj == 2 and opcode == 0:  # registry.global
                    name, n = struct.unpack_from("<II", payload)
                    iface = payload[8:8 + n - 1].decode()
                    ver = struct.unpack_from("<I", payload, 8 + n + (-n % 4))[0]
                    self.globals[iface] = (name, ver)
                if obj == cb and opcode == 0:
                    return

    def bind(self, iface, version=1):
        name, have = self.globals[iface]
        oid = self.new_id()
        args = struct.pack("<I", name) + self.string(iface) + struct.pack("<II", min(version, have), oid)
        self.send(2, 0, args)
        return oid

    def keyboard(self):
        self.send(1, 1, struct.pack("<I", self.new_id()))  # get_registry -> id 2
        self.roundtrip()
        seat = self.bind("wl_seat", 1)
        mgr = self.bind("zwp_virtual_keyboard_manager_v1", 1)
        kb = self.new_id()
        self.send(mgr, 0, struct.pack("<II", seat, kb))
        with tempfile.TemporaryFile() as f:
            f.write(KEYMAP)
            f.flush()
            self.send(kb, 0, struct.pack("<II", 1, len(KEYMAP)), fds=[f.fileno()])
            self.roundtrip()
        return kb

    def key(self, kb, code, down):
        self.send(kb, 1, struct.pack("<III", int(time.monotonic() * 1000) & 0xFFFFFFFF, code, 1 if down else 0))


def type_text(text, delay_ms=40):
    wl = Wl()
    kb = wl.keyboard()
    for ch in text:
        shift, code = key_for(ch)
        if shift:
            wl.key(kb, KEY_LEFTSHIFT, True)
        wl.key(kb, code, True)
        wl.roundtrip()
        time.sleep(delay_ms / 2000)
        wl.key(kb, code, False)
        if shift:
            wl.key(kb, KEY_LEFTSHIFT, False)
        wl.roundtrip()
        time.sleep(delay_ms / 2000)


if __name__ == "__main__":
    args = sys.argv[1:]
    delay = 40
    if args[:1] == ["--delay-ms"]:
        delay, args = int(args[1]), args[2:]
    type_text(" ".join(args).replace("\\n", "\n"), delay)
