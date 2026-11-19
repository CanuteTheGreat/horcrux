#!/usr/bin/env python3
"""Console "Paste" types the text as key presses (Firefox, Playwright).

Loads the real noVNC page the API serves (HTML dumped by the Rust test
`novnc_page_dump`: NOVNC_DUMP=<file>) with a stubbed noVNC library that records
`sendKey` / `clipboardPasteFrom` calls, so it needs no VM and no network.

 * keysym mapping (unit level, in the page): letters, digits, shifted symbols,
   Return/Tab, CRLF, Latin-1 and Unicode keysyms, control characters skipped,
   shift only for A-Z on a non-US layout;
 * default mode types: every character is a press + release, Shift is held
   around capitals/symbols and released at the end;
 * `clipboard` mode sends only the clipboard text, `both` does both;
 * pacing is honoured and a cancel stops typing and releases Shift;
 * the audit call carries a character count and mode, never the text;
 * too-long text is refused; long text asks for confirmation.

usage: console_paste_e2e.py <novnc.html>   (exit 1 on any failure)
"""
import json
import sys
from playwright.sync_api import sync_playwright

HTML = open(sys.argv[1], encoding="utf-8").read()
ORIGIN = "https://horcrux.test"
STUB = """
export default class RFB {
  constructor(el) {
    this.l = {};
    const c = document.createElement('canvas');
    c.width = 640; c.height = 400; c.id = 'guest';
    el.appendChild(c);
    window.__keys = []; window.__clip = [];
    setTimeout(() => (this.l.connect || []).forEach(f => f({})), 30);
  }
  addEventListener(n, f) { (this.l[n] = this.l[n] || []).push(f); }
  sendCtrlAltDel() {}
  sendKey(k, c, d) { window.__keys.push([k, c, d]); }
  clipboardPasteFrom(t) { window.__clip.push(t); }
}
"""

fails = []


def check(cond, msg):
    print(("ok   " if cond else "FAIL ") + msg)
    if not cond:
        fails.append(msg)


SECRET = "S3cr3t!Pa$$w0rd"
audits = []

with sync_playwright() as p:
    b = p.firefox.launch()
    ctx = b.new_context(viewport={"width": 1000, "height": 700}, ignore_https_errors=True)

    def route(r):
        u = r.request.url
        if u.endswith("rfb.js"):
            r.fulfill(status=200, content_type="text/javascript", body=STUB)
        elif "/paste-audit" in u:
            audits.append((u, r.request.post_data))
            r.fulfill(status=204, body="")
        elif u.startswith(ORIGIN + "/api/console/vm1/novnc"):
            r.fulfill(status=200, content_type="text/html", body=HTML)
        else:
            r.fulfill(status=404, body="")

    ctx.route("**/*", route)
    pg = ctx.new_page()
    errors = []
    pg.on("pageerror", lambda e: errors.append(str(e)))
    pg.goto(ORIGIN + "/api/console/vm1/novnc")
    pg.wait_for_function("document.getElementById('status').textContent === 'Connected'")

    # ---- keysym mapping ----------------------------------------------------
    plan = lambda t, layout="en-us", **o: pg.evaluate(
        "([t, l, o]) => HorcruxPaste.plan(t, l, o)", [t, layout, o])
    ks = lambda t, layout="en-us", **o: [(s["keysym"], s["shift"]) for s in plan(t, layout, **o)["steps"]]

    check(ks("aZ1 ") == [(0x61, False), (0x5A, True), (0x31, False), (0x20, False)],
          "letters, capital (shift), digit, space")
    check(ks("!@#") == [(0x21, True), (0x40, True), (0x23, True)], "US shifted symbols press Shift")
    check(ks("-=[];',./`\\") == [(ord(c), False) for c in "-=[];',./`\\"], "unshifted symbols stay unshifted")
    check(ks('a\nb\tc\r\nd') == [(0x61, False), (0xFF0D, False), (0x62, False), (0xFF09, False),
                                 (0x63, False), (0xFF0D, False), (0x64, False)],
          "newline/CRLF -> Return, tab -> Tab")
    pl = plan("a\x00b\x1bc\x7f")
    check(pl["skipped"] == 3 and [s["keysym"] for s in pl["steps"]] == [0x61, 0x62, 0x63],
          "control characters are skipped and counted")
    check(ks("é€😀") == [(0xE9, False), (0x01000000 + 0x20AC, False), (0x01000000 + 0x1F600, False)],
          "Latin-1 keysym = code point, others 0x01000000+cp (astral kept whole)")
    pl = plan("aé", unicode=False)
    check(pl["skipped"] == 1 and pl["nonAscii"] == 1 and len(pl["steps"]) == 1, "unicode keysyms can be switched off")
    check(plan("é€")["nonAscii"] == 2, "non-ASCII characters are counted for the warning")
    check(ks("A!", "de") == [(0x41, True), (0x21, False)],
          "non-US layout: Shift for A-Z only (the VM keymap picks the keys for symbols)")

    # ---- default mode types -------------------------------------------------
    def paste(text, mode=None):
        pg.evaluate("([t, m]) => { window.__keys.length = 0; window.__clip.length = 0;"
                    " window.postMessage({horcruxConsole: 'paste', text: t, mode: m}, location.origin); }",
                    [text, mode])

    def settled():
        pg.wait_for_function("document.getElementById('btn-paste').textContent === 'Paste'", timeout=20000)

    paste("Hi!\n")
    pg.wait_for_function("window.__keys.length >= 9")
    settled()
    keys = pg.evaluate("window.__keys")
    seq = [(k[0], k[2]) for k in keys]
    want = [(0xFFE1, True), (0x48, True), (0x48, False), (0xFFE1, False), (0x69, True), (0x69, False),
            (0xFFE1, True), (0x21, True), (0x21, False), (0xFFE1, False), (0xFF0D, True), (0xFF0D, False)]
    check(seq == want, f"default mode types 'Hi!\\n' with Shift around H and ! ({seq})")
    check(pg.evaluate("window.__clip") == [], "default mode does not touch the guest clipboard")
    check(pg.evaluate("document.getElementById('status').textContent") == "Connected",
          "status goes back to Connected after typing")

    # ---- clipboard / both ----------------------------------------------------
    paste("clip me", "clipboard")
    pg.wait_for_function("window.__clip.length === 1")
    check(pg.evaluate("window.__keys.length") == 0 and pg.evaluate("window.__clip") == ["clip me"],
          "clipboard mode only sets the guest clipboard")
    paste("ab", "both")
    pg.wait_for_function("window.__clip.length === 1 && window.__keys.length === 4")
    settled()
    check(pg.evaluate("window.__clip") == ["ab"] and pg.evaluate("window.__keys.length") == 4,
          "both mode sets the clipboard and types")

    # ---- audit never carries the text ----------------------------------------
    audits.clear()
    paste(SECRET)
    pg.wait_for_function("window.__keys.length >= %d" % (2 * len(SECRET)))
    settled()
    check(len(audits) == 1, "one audit call per paste")
    body = audits[0][1] if audits else ""
    check(json.loads(body) == {"chars": len(SECRET), "mode": "type"}, f"audit body is count + mode only ({body})")
    check(SECRET not in body and SECRET not in audits[0][0], "pasted text is not in the audit call")
    check("/api/console/vm1/paste-audit" in audits[0][0], "audit goes to this VM's route")

    # ---- cancel releases Shift -------------------------------------------------
    pg.evaluate("window.__keys.length = 0")
    pg.evaluate("window.postMessage({horcruxConsole: 'paste', text: 'A'.repeat(300)}, location.origin)")
    pg.wait_for_function("document.getElementById('btn-paste').textContent === 'Cancel paste'")
    pg.wait_for_function("window.__keys.length > 20")
    pg.evaluate("window.postMessage({horcruxConsole: 'paste-cancel'}, location.origin)")
    settled()
    n = pg.evaluate("window.__keys.length")
    pg.wait_for_timeout(300)
    check(pg.evaluate("window.__keys.length") == n, "typing stops after cancel")
    last = pg.evaluate("window.__keys[window.__keys.length - 1]")
    check(last[0] == 0xFFE1 and last[2] is False, "cancel leaves no Shift held in the guest")
    check(n < 600, f"cancel really cut the paste short ({n} key events of 602)")

    # ---- limits -----------------------------------------------------------------
    dialogs = []
    pg.on("dialog", lambda d: (dialogs.append((d.type, d.message)), d.dismiss()))
    paste("x" * 20001)
    pg.wait_for_timeout(300)
    check(dialogs and dialogs[-1][0] == "alert" and pg.evaluate("window.__keys.length") == 0,
          "text over the limit is refused with a message")
    dialogs.clear()
    paste("y" * 501)
    pg.wait_for_timeout(300)
    check(dialogs and dialogs[-1][0] == "confirm" and pg.evaluate("window.__keys.length") == 0,
          "long text asks for confirmation (declined = nothing typed)")

    check(errors == [], f"no uncaught JS errors ({errors})")
    b.close()

if fails:
    print(f"\n{len(fails)} FAILED")
    sys.exit(1)
print("\nall console paste checks passed")
