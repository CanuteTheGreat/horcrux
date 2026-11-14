#!/usr/bin/env python3
"""No overlay may cover the VNC console (Firefox, Playwright).

Loads the real noVNC page the API serves (HTML dumped by the Rust test
`novnc_page_dump`: NOVNC_DUMP=<file>) with a stubbed noVNC library that draws a
canvas and fires `connect`, so it needs no VM and no network. Checks:

 * embedded (iframe in the UI, ?embed=1): the iframe's canvas fills the whole
   iframe, no visible fixed/absolute element intersects it, and the guest's top
   40 px hit the canvas (not a toolbar or a status badge);
 * pop-out window: the same, the auto-hide bar is out of view until the pointer
   touches the top edge, appears there, and hides again afterwards;
 * the buttons still drive the guest (Ctrl+Alt+Del, paste) in both modes;
 * leaving fullscreen by any path (document.exitFullscreen() = F11/Esc, the
   button, or just a window resize back from the fullscreen size) re-fits the
   guest: the canvas stays inside its container (aspect kept) and the page has
   no horizontal overflow. The fit stub only rescales when `scaleViewport` is
   (re)assigned, i.e. it does not watch the container itself, so the page's own
   refit logic is what is under test.

usage: console_overlay_e2e.py <novnc.html>   (exit 1 on any failure)
"""
import sys
from playwright.sync_api import sync_playwright

HTML = open(sys.argv[1], encoding="utf-8").read()
ORIGIN = "https://horcrux.test"
STUB = """
export default class RFB {
  constructor(el) {
    this.l = {};
    const c = document.createElement('canvas');
    c.width = 1024; c.height = 768; c.id = 'guest';
    c.style.cssText = 'width:100%;height:100%;display:block;background:#345';
    el.appendChild(c);
    setTimeout(() => (this.l.connect || []).forEach(f => f({})), 30);
  }
  addEventListener(n, f) { (this.l[n] = this.l[n] || []).push(f); }
  sendCtrlAltDel() { window.__cad = (window.__cad || 0) + 1; }
  clipboardPasteFrom(t) { window.__paste = t; }
}
"""
STUB_FIT = """
export default class RFB {
  constructor(el) {
    this.l = {}; this.el = el; this._s = false;
    const c = document.createElement('canvas');
    c.width = 1024; c.height = 768; c.id = 'guest';
    c.style.cssText = 'display:block;background:#345;width:1024px;height:768px';
    el.appendChild(c); this.c = c;
    setTimeout(() => (this.l.connect || []).forEach(f => f({})), 30);
  }
  set scaleViewport(v) {
    this._s = v;
    if (!v) { this.c.style.width = '1024px'; this.c.style.height = '768px'; return; }
    const k = Math.min(this.el.clientWidth / 1024, this.el.clientHeight / 768);
    this.c.style.width = Math.floor(1024 * k) + 'px';
    this.c.style.height = Math.floor(768 * k) + 'px';
  }
  get scaleViewport() { return this._s; }
  addEventListener(n, f) { (this.l[n] = this.l[n] || []).push(f); }
  sendCtrlAltDel() {}
  clipboardPasteFrom(t) {}
}
"""
FIT = """() => {
  const c = document.getElementById('guest').getBoundingClientRect();
  const s = document.getElementById('screen').getBoundingClientRect();
  const d = document.documentElement;
  return {c: [c.left, c.top, c.right, c.bottom], s: [s.left, s.top, s.right, s.bottom],
          fits: c.left >= s.left - 1 && c.top >= s.top - 1 && c.right <= s.right + 1 && c.bottom <= s.bottom + 1,
          overflow: d.scrollWidth > d.clientWidth || document.body.scrollWidth > d.clientWidth,
          ratio: c.width / c.height};
}"""
PARENT = """<!doctype html><body style="margin:0">
<div id="toolbar" style="height:48px;background:#222;color:#fff">toolbar</div>
<iframe id="console-frame" src="/novnc?embed=1" style="display:block;width:100%;height:600px;border:0"></iframe>
<script>window.addEventListener('message',e=>{if(e.data&&e.data.horcruxConsole==='status')window.__st=e.data.state;});</script>
"""

# every visible positioned element other than the canvas must not intersect it
OVERLAYS = """() => {
  const c = document.getElementById('guest').getBoundingClientRect();
  const out = [];
  for (const el of document.querySelectorAll('body *')) {
    if (el.id === 'guest' || el.id === 'screen' || el.contains(document.getElementById('guest'))) continue;
    const cs = getComputedStyle(el);
    if (!['fixed', 'absolute', 'sticky'].includes(cs.position)) continue;
    if (cs.display === 'none' || cs.visibility === 'hidden' || +cs.opacity === 0) continue;
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) continue;
    if (r.left < c.right && r.right > c.left && r.top < c.bottom && r.bottom > c.top)
      out.push(el.tagName + '#' + el.id + '.' + el.className);
  }
  return out;
}"""
TOPHITS = """() => {
  const c = document.getElementById('guest').getBoundingClientRect();
  const pts = [[c.left + 5, c.top + 5], [c.left + 5, c.top + 39], [c.right - 5, c.top + 5],
               [c.right - 5, c.top + 39], [c.left + c.width / 2, c.top + 20]];
  return pts.map(([x, y]) => (document.elementFromPoint(x, y) || {}).id);
}"""

fails = []
def check(cond, msg):
    print(("ok   " if cond else "FAIL ") + msg)
    if not cond:
        fails.append(msg)

with sync_playwright() as p:
    b = p.firefox.launch()
    for name, vp in (("desktop", {"width": 1280, "height": 800}), ("phone", {"width": 390, "height": 760})):
        ctx = b.new_context(viewport=vp, ignore_https_errors=True)
        stub = {"v": STUB}
        def route(r):
            u = r.request.url
            if u.endswith("rfb.js"):
                r.fulfill(status=200, content_type="text/javascript", body=stub["v"])
            elif u.startswith(ORIGIN + "/novnc"):
                r.fulfill(status=200, content_type="text/html", body=HTML)
            elif u == ORIGIN + "/parent":
                r.fulfill(status=200, content_type="text/html", body=PARENT)
            else:
                r.fulfill(status=404, body="")
        ctx.route("**/*", route)

        # ---- embedded -------------------------------------------------------
        pg = ctx.new_page()
        pg.goto(ORIGIN + "/parent")
        fr = pg.frame_locator("#console-frame")
        fr.locator("#guest").wait_for(timeout=15000)
        f = next(x for x in pg.frames if x != pg.main_frame)
        f.wait_for_function("document.getElementById('status').textContent === 'Connected'")
        pg.wait_for_function("window.__st === 'connected'")
        check(True, f"[{name}] embedded: status reaches the parent toolbar")
        c = f.evaluate("(() => { const r = document.getElementById('guest').getBoundingClientRect();"
                       " return [r.left, r.top, r.width, r.height, innerWidth, innerHeight]; })()")
        check(c[0] == 0 and c[1] == 0 and abs(c[2] - c[4]) <= 1 and abs(c[3] - c[5]) <= 1,
              f"[{name}] embedded: canvas fills the whole iframe ({c})")
        ov = f.evaluate(OVERLAYS)
        check(ov == [], f"[{name}] embedded: no element overlaps the canvas ({ov})")
        hits = f.evaluate(TOPHITS)
        check(set(hits) == {"guest"}, f"[{name}] embedded: guest top 40px hit the canvas ({hits})")
        pg.evaluate("document.getElementById('console-frame').contentWindow.postMessage("
                    "{horcruxConsole:'ctrl-alt-del'}, location.origin)")
        pg.evaluate("document.getElementById('console-frame').contentWindow.postMessage("
                    "{horcruxConsole:'paste', text:'hello'}, location.origin)")
        f.wait_for_function("window.__cad === 1 && window.__paste === 'hello'")
        check(True, f"[{name}] embedded: parent toolbar drives Ctrl+Alt+Del and paste")
        pg.close()

        # ---- pop-out window -------------------------------------------------
        pg = ctx.new_page()
        pg.goto(ORIGIN + "/novnc")
        pg.wait_for_function("document.getElementById('status').textContent === 'Connected'")
        c = pg.evaluate("(() => { const r = document.getElementById('guest').getBoundingClientRect();"
                        " return [r.left, r.top, r.width, r.height, innerWidth, innerHeight]; })()")
        check(c[0] == 0 and c[1] == 0 and abs(c[2] - c[4]) <= 1 and abs(c[3] - c[5]) <= 1,
              f"[{name}] pop-out: canvas fills the window ({c})")
        ov = pg.evaluate(OVERLAYS)
        check(ov == [], f"[{name}] pop-out: bar hidden, nothing overlaps the canvas ({ov})")
        hits = pg.evaluate(TOPHITS)
        check(set(hits) == {"guest"}, f"[{name}] pop-out: guest top 40px hit the canvas ({hits})")
        check(not pg.locator("#bar").is_visible(), f"[{name}] pop-out: bar not visible by default")
        pg.mouse.move(300, 300)
        pg.mouse.move(300, 0)
        pg.wait_for_function("document.getElementById('bar').classList.contains('show')")
        pg.wait_for_timeout(250)
        check(pg.locator("#bar").is_visible() and pg.locator("#btn-cad").is_visible(),
              f"[{name}] pop-out: bar slides in at the top edge")
        pg.locator("#btn-cad").click()
        check(pg.evaluate("window.__cad") == 1, f"[{name}] pop-out: bar Ctrl+Alt+Del reaches the guest")
        pg.mouse.move(300, 300)
        pg.wait_for_function("!document.getElementById('bar').classList.contains('show')", timeout=5000)
        pg.wait_for_timeout(300)
        check(not pg.locator("#bar").is_visible(), f"[{name}] pop-out: bar hides again")
        check(set(pg.evaluate(TOPHITS)) == {"guest"}, f"[{name}] pop-out: top 40px clickable again after hide")
        pg.close()

        # ---- leaving fullscreen re-fits the guest ---------------------------
        stub["v"] = STUB_FIT
        def fit_ok(pg, label):
            pg.wait_for_timeout(1000)  # refit settles over <= 700 ms
            r = pg.evaluate(FIT)
            check(r["fits"] and not r["overflow"] and abs(r["ratio"] - 4 / 3) < 0.03,
                  f"[{name}] {label}: canvas fits its container, aspect kept, no h-overflow ({r})")

        for mode in ("pop-out", "embedded"):
            pg = ctx.new_page()
            pg.set_viewport_size({"width": 1920, "height": 1080})  # 'fullscreen' size
            if mode == "pop-out":
                pg.goto(ORIGIN + "/novnc")
                tgt = pg
            else:
                pg.goto(ORIGIN + "/parent")
                pg.frame_locator("#console-frame").locator("#guest").wait_for(timeout=15000)
                tgt = next(x for x in pg.frames if x != pg.main_frame)
            tgt.wait_for_function("document.getElementById('status').textContent === 'Connected'")
            fit_ok(tgt, f"{mode} at 1920x1080")
            # F11 / browser UI: the window shrinks back, no click on our button
            pg.set_viewport_size(vp)
            fit_ok(tgt, f"{mode} after window resize back to {vp['width']}x{vp['height']}")
            pg.set_viewport_size({"width": 1920, "height": 1080})
            fit_ok(tgt, f"{mode} grown again")
            # button -> fullscreen -> exitFullscreen() (what Esc/F11 end up calling)
            if mode == "pop-out":
                pg.mouse.move(300, 300)
                pg.mouse.move(300, 0)
                pg.wait_for_function("document.getElementById('bar').classList.contains('show')")
                pg.wait_for_timeout(250)
                pg.locator("#btn-fullscreen").click()
            else:
                tgt.evaluate("document.documentElement.requestFullscreen().catch(() => {})")
            pg.wait_for_timeout(500)
            pg.set_viewport_size(vp)
            tgt.evaluate("document.fullscreenElement ? document.exitFullscreen() : null")
            fit_ok(tgt, f"{mode} after exitFullscreen()")
            pg.close()
        stub["v"] = STUB
        ctx.close()
    b.close()

print("FAILED: %d" % len(fails) if fails else "PASS")
sys.exit(1 if fails else 0)
