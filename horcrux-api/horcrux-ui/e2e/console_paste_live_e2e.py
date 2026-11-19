#!/usr/bin/env python3
"""Console "Paste" against a REAL guest through the real noVNC library.

Not part of CI (needs KVM, a guest image and internet for the noVNC CDN): run it
in a container that has qemu and playwright (python venv + firefox) next to the
guest image; the page is the one `novnc_page_dump` writes.

The noVNC page the API serves (dumped with NOVNC_DUMP=<file> NOVNC_DUMP_WS=ws://127.0.0.1:<port>)
connects straight to QEMU's own VNC websocket (`-vnc 127.0.0.1:N,websocket=on`), so
exactly the keysyms the web console sends reach QEMU's keymap code.

usage: console_paste_live_e2e.py windows <novnc.html> <ws-port> <unused> <shots-dir>
         Windows guest without a clipboard agent: Win+R, notepad, Paste (screenshots are the proof)
       console_paste_live_e2e.py linux <novnc.html> <ws-port> <serial-file> <shots-dir>
         cirros guest: logs in on tty1 BY PASTING, pastes a payload into `cat > file`,
         sends it back on the serial port and compares it byte for byte.
       console_paste_live_e2e.py grub <novnc.html> <ws-port> <serial-file> <shots-dir>
         pre-OS: opens the GRUB command line (no clipboard agent exists there), pastes
         commands that print on the serial port and checks them.
"""
import os
import sys
import time
from playwright.sync_api import sync_playwright

mode, html_path, wsport, serial, shots = sys.argv[1:6]
HTML = open(html_path, encoding="utf-8").read()
ORIGIN = "https://horcrux.test"
fails = []


def check(c, msg):
    print(("ok   " if c else "FAIL ") + msg, flush=True)
    if not c:
        fails.append(msg)


def diff(want, got):
    i = next((k for k, (a, b) in enumerate(zip(want, got)) if a != b), min(len(want), len(got)))
    return f"first difference at {i} of {len(want)}/{len(got)}: want {want[i-10:i+30]!r} got {got[i-10:i+30]!r}"


def ser():
    return open(serial, "rb").read().decode("latin-1")


def wait_serial(needle, secs, start=0):
    end = time.time() + secs
    while time.time() < end:
        if needle in ser()[start:]:
            return True
        time.sleep(1)
    return False


PAYLOAD = ("Hello, World! ABC xyz 0123456789\n"
           "~!@#$%^&*()_+{}|:\"<>? -=[];',./`\\\n"
           "tab\there\n"
           + "The quick brown fox jumps over the lazy dog. " * 8 + "\n"
           "last line without newline")

with sync_playwright() as p:
    b = p.firefox.launch()
    ctx = b.new_context(viewport={"width": 1100, "height": 800}, ignore_https_errors=True)
    ctx.route(ORIGIN + "/api/console/vm1/novnc", lambda r: r.fulfill(status=200, content_type="text/html", body=HTML))
    ctx.route("**/paste-audit", lambda r: r.fulfill(status=204, body=""))
    pg = ctx.new_page()
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    pg.goto(ORIGIN + "/api/console/vm1/novnc")
    pg.wait_for_function("document.getElementById('status').textContent === 'Connected'", timeout=60000)

    def paste(text):
        pg.evaluate("t => window.horcruxPaste(t)", text)  # resolves when typing is done
        time.sleep(0.5)

    def key(sym, down=None):
        pg.evaluate("([s, d]) => rfb.sendKey(s, null, d)", [sym, down]) if down is not None else \
            pg.evaluate("s => rfb.sendKey(s)", sym)

    def shot(name):
        pg.screenshot(path=f"{shots}/{name}.png")

    if mode == "linux":
        check(wait_serial("login:", 240), "guest booted (serial login prompt)")
        time.sleep(5)
        shot("01-tty1-login-prompt")
        paste("cirros\n")
        time.sleep(2)
        paste("gocubsgo\n")
        time.sleep(3)
        shot("02-logged-in-by-paste")
        paste("sudo chmod 666 /dev/ttyS0\n")
        time.sleep(2)
        mark = len(ser())
        paste("echo LOGIN-OK-$((40+2)) > /dev/ttyS0\n")
        check(wait_serial("LOGIN-OK-42", 20, mark), "login typed by Paste reached a working shell")
        for d in [int(x) for x in os.environ.get("PASTE_DELAYS", "").split(",") if x] or [None]:
            if d is not None:
                pg.evaluate("d => { window.horcruxPasteCfg.delay = d; }", d)
            tag = f"delay={d if d is not None else 'default'}ms"
            f = f"/tmp/p{d}.txt"
            paste(f"cat > {f}\n")
            paste(PAYLOAD)
            for _ in range(2):  # Ctrl+D flushes the unterminated last line, the second ends cat
                key(0xFFE3, True); key(0x64); key(0xFFE3, False)
                time.sleep(0.3)
            time.sleep(1)
            shot(f"03-payload-typed-{d}")
            mark = len(ser())
            paste(f"echo BEGIN > /dev/ttyS0; cat {f} > /dev/ttyS0; echo > /dev/ttyS0; echo END > /dev/ttyS0\n")
            check(wait_serial("END", 30, mark), f"[{tag}] payload came back on the serial port")
            got = ser()[mark:].replace("\r\n", "\n").replace("\r", "")
            got = got.split("BEGIN\n", 1)[-1].rsplit("\nEND", 1)[0].rstrip("\n")
            check(got == PAYLOAD, f"[{tag}] typed text equals the pasted text byte for byte"
                  + ("" if got == PAYLOAD else "\n  " + diff(PAYLOAD, got)))
        shot("04-readback")
    elif mode == "windows":
        # Windows guest WITHOUT a clipboard agent: Win+R, notepad, then Paste into it.
        time.sleep(int(os.environ.get("WIN_BOOT_WAIT", "150")))
        shot("01-desktop")
        key(0xFFEB, True); key(0x72); key(0xFFEB, False)  # Win+R
        time.sleep(3)
        shot("02-run-dialog")
        paste("notepad\n")
        time.sleep(6)
        shot("03-notepad-open")
        paste("Hello, World! ABC xyz 0123456789\n~!@#$%^&*()_+{}|:\"<>? -=[];',./`\\\nPasted into Windows without any clipboard agent.")
        time.sleep(1)
        shot("04-notepad-pasted")
    else:  # grub
        # QEMU was started paused (-S) so the boot menu's timeout cannot run out
        # before the console is up: resume it over the monitor socket.
        import socket
        mon = socket.socket(socket.AF_UNIX)
        mon.connect(os.environ["QEMU_MON"])
        mon.sendall(b"cont\n")
        time.sleep(5)
        shot("01-boot-menu")
        key(0x63)  # 'c' -> GRUB command line
        time.sleep(2)
        shot("02-grub-prompt")
        mark = len(ser())
        paste("serial --unit=0 --speed=115200\nterminal_input console\nterminal_output serial console\n")
        paste("echo 'Hello, World! @#$%^&*()_+{}|:<>? ~`-=[];,./\\\\'\n")  # single quotes: GRUB does not expand inside
        paste("echo Second-line-ABC-xyz-0123\n")
        shot("03-pasted-at-grub-prompt")
        ok = wait_serial("Second-line-ABC-xyz-0123", 20, mark)
        out = ser()[mark:]
        check(ok, "text typed by Paste at the boot loader prompt ran as commands")
        check("Hello, World! @#$%^&*()_+{}|:<>? ~`-=[];,./\\\\" in out, "symbols and capitals arrived intact at the pre-OS prompt")
    check(errs == [], f"no uncaught JS errors ({errs})")
    b.close()

if fails:
    print(f"\n{len(fails)} FAILED")
    sys.exit(1)
print("\nlive paste checks passed")
