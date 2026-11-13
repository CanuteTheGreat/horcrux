#!/usr/bin/env python3
"""UI crawl: every page reachable from the nav renders cleanly at desktop AND phone width, light AND dark.

One deterministic, no-LLM check for the classes of defect the owner kept finding by hand:

  js-error          uncaught exception / console.error / failed script (blank UI after a bad wasm build)
  blank             page body empty after load
  http-5xx          an API request answered 5xx while the page loaded
  not-found         the page itself says 404 / not found
  bad-text          'undefined', 'NaN', '[object Object]', 'missing field', 'panicked' printed to the user
  private-text      a private URL / private host / owner machine name visible in the UI
  internal-name     build-internal vocabulary shown to users (see forbidden_names in the config)
  overflow          the page scrolls sideways (phone: fail, desktop: warn)
  canvas-overlay    a visible positioned element covers a canvas/iframe/video (VNC console buttons over the guest)
  unlinked-summary  a stat/summary card that is not a link and has no link inside (dead dashboard counts)
  empty-no-action   an empty state ("No X found") with no button or link to do something about it  (warn)
  same-title        document.title does not identify the page on >= 5 routes                     (warn)

Usage:
  BASE_URL=https://host:8006 ADMIN_PASSWORD=... ux_crawl.py [--config ux-crawl.json] [--static DIR] [--report out.json]
        [--shots DIR] [--widths desktop,phone] [--schemes light,dark] [--routes /a,/b] [--max-routes N]

Exit 1 when there is a `fail` finding that is not in the baseline file (the config's "baseline" path: id -> reason/owner).
`warn` findings are printed and written to the report, never fail the run.  Baseline ids are `kind:route`.
--static DIR serves a built SPA from DIR (history fallback to index.html) instead of BASE_URL; the config's "hook" (a python file
exposing install(page)) then stubs the API (Patronus: frontend/scripts/ux_mock.py).
Needs `pip install playwright && playwright install firefox` (Firefox is the QA browser; BROWSER=chromium overrides).
"""
import argparse
import http.server
import importlib.util
import json
import os
import re
import sys
import threading
import time
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright

HERE = os.path.dirname(os.path.abspath(__file__))
VIEWPORTS = {"desktop": (1280, 800), "phone": (390, 844)}

# Runs in the page: facts the rules need.  Kept as one evaluate so a page costs one round trip.
FACTS_JS = r"""() => {
  const vis = el => { const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || +cs.opacity === 0) return false;
                      const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const text = document.body ? document.body.innerText : '';
  // canvas / iframe / video overlays
  const overlays = [];
  for (const c of document.querySelectorAll('canvas, iframe, video')) {
    if (!vis(c)) continue;
    const cr = c.getBoundingClientRect();
    for (const el of document.querySelectorAll('body *')) {
      if (el === c || el.contains(c) || c.contains(el) || !vis(el)) continue;
      const cs = getComputedStyle(el);
      if (!['fixed', 'absolute', 'sticky'].includes(cs.position)) continue;
      const r = el.getBoundingClientRect();
      if (r.left < cr.right - 1 && r.right > cr.left + 1 && r.top < cr.bottom - 1 && r.bottom > cr.top + 1)
        overlays.push(c.tagName + ' covered by ' + el.tagName + '#' + el.id + '.' + String(el.className).slice(0, 40));
    }
  }
  // summary cards that are not links
  const unlinked = [];
  for (const el of document.querySelectorAll('.stat-card, [class*="summary-card"], [class*="kpi"]')) {
    if (!vis(el)) continue;
    if (el.closest('a[href]') || el.querySelector('a[href]') || el.getAttribute('role') === 'link' || el.hasAttribute('onclick')) continue;
    unlinked.push((el.id || el.className || el.tagName) + ': ' + (el.innerText || '').replace(/\s+/g, ' ').trim().slice(0, 50));
  }
  // empty states with no way forward
  const empty = [];
  const emptyRe = /^(no|there are no|nothing|none)\b[^.]{0,60}\b(found|yet|configured|available|defined|to show|to display|here)\b/i;
  for (const el of document.querySelectorAll('td, p, div, span, li')) {
    if (el.children.length > 3 || !vis(el)) continue;
    const t = (el.innerText || '').trim();
    if (t.length > 120 || !emptyRe.test(t)) continue;
    const scope = el.closest('table, section, .card, .panel, main, .content') || document.body;
    const action = [...scope.querySelectorAll('button, a[href], input[type=submit]')].filter(vis).filter(b => !b.closest('nav, aside, header, .sidebar'));
    if (!action.length) empty.push(t.slice(0, 70));
  }
  const root = document.documentElement;
  return {
    textLen: text.trim().length, text: text.slice(0, 60000), title: document.title,
    overflowX: root.scrollWidth - root.clientWidth, overlays: overlays.slice(0, 4), unlinked: unlinked.slice(0, 6), empty: empty.slice(0, 3),
    h1: ((document.querySelector('main h1, h1, main h2, h2') || {}).innerText || '').trim().slice(0, 60),
  };
}"""

LINKS_JS = r"""() => [...document.querySelectorAll('a[href]')].map(a => [a.getAttribute('href'), (a.innerText || '').trim().slice(0, 40), !!a.closest('nav, aside, .sidebar')])"""


def load_config(path):
    cfg = json.load(open(path))
    cfg.setdefault("login_path", "/login")
    cfg.setdefault("max_routes", 120)
    cfg.setdefault("forbidden_names", [])
    cfg.setdefault("summary_fail_routes", ["/"])  # the landing dashboard; stat cards elsewhere are filters/summaries -> warn
    cfg.setdefault("private_fail", [])
    cfg.setdefault("private_warn", [])
    return cfg


def slug(route):
    return re.sub(r"[^a-zA-Z0-9]+", "-", route).strip("-") or "home"


def norm_route(href, base):
    """Same-origin app route (path + query) or None."""
    if not href or href.startswith(("#", "javascript:", "mailto:", "tel:")):
        return None
    u = urlsplit(href)
    if u.scheme and u.netloc and u.netloc != urlsplit(base).netloc:
        return None
    path = u.path or "/"
    if path.startswith(("/api/", "/novnc", "/static/", "/ws")) or re.search(r"\.(png|jpg|svg|ico|css|js|wasm|pem|json|zip|iso|vv|txt|log)$", path):
        return None
    return path + (("?" + u.query) if u.query else "")


def login(page, base, cfg):
    page.goto(base + cfg["login_path"], wait_until="load")
    page.locator(cfg["pass_sel"]).first.wait_for(timeout=30000)
    page.locator(cfg["user_sel"]).first.fill(cfg.get("username", "admin"))
    page.locator(cfg["pass_sel"]).first.fill(os.environ.get(cfg.get("password_env", "ADMIN_PASSWORD"), cfg.get("password", "admin")))
    page.locator(cfg["submit_sel"]).first.click()
    page.wait_for_function("() => !location.pathname.startsWith('/login')", timeout=30000)


def settle(page):
    try:
        page.wait_for_function("() => document.body && document.body.innerText.trim().length > 20", timeout=15000)
    except Exception:  # noqa: BLE001  (blank pages are reported by the rule, not by the wait)
        pass
    page.wait_for_timeout(900)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=os.path.join(HERE, "ux-crawl.json"))
    ap.add_argument("--report")
    ap.add_argument("--shots")
    ap.add_argument("--widths", default="desktop,phone")
    ap.add_argument("--schemes", default="light,dark")
    ap.add_argument("--routes", help="comma separated routes instead of discovering them from the nav")
    ap.add_argument("--max-routes", type=int)
    ap.add_argument("--static")
    ap.add_argument("--base", default=os.environ.get("BASE_URL", "https://localhost:8006"))
    a = ap.parse_args()
    cfg = load_config(a.config)
    base = a.base.rstrip("/")
    hook = None
    if a.static:
        root = os.path.abspath(a.static)

        class H(http.server.SimpleHTTPRequestHandler):
            def __init__(s, *x, **k):
                super().__init__(*x, directory=root, **k)

            def log_message(s, *x):
                pass

            def do_GET(s):
                if "." not in os.path.basename(s.path.split("?")[0]):
                    s.path = "/index.html"
                return super().do_GET()

        srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{srv.server_address[1]}"
    if cfg.get("hook"):
        sp = importlib.util.spec_from_file_location("ux_hook", os.path.join(os.path.dirname(os.path.abspath(a.config)), cfg["hook"]))
        hook = importlib.util.module_from_spec(sp)
        sp.loader.exec_module(hook)
    baseline = {}
    bpath = cfg.get("baseline")
    if bpath and os.path.exists(os.path.join(os.path.dirname(os.path.abspath(a.config)), bpath)):
        baseline = json.load(open(os.path.join(os.path.dirname(os.path.abspath(a.config)), bpath)))
    forbidden = [re.compile(p, re.I) for p in cfg["forbidden_names"]]
    pfail = [re.compile(p, re.I) for p in cfg["private_fail"]]
    pwarn = [re.compile(p, re.I) for p in cfg["private_warn"]]
    bad_text = re.compile(r"\bundefined\b|\bNaN\b|\[object Object\]|missing field|panicked at|Traceback \(most|RUST_BACKTRACE|\{\{|\}\}", re.I)
    notfound = re.compile(r"\b404\b|page not found|route not found|^not found$", re.I | re.M)
    findings, pages = [], []
    seen = set()

    def add(kind, sev, route, vp, scheme, detail):
        key = (kind, route, vp, scheme, detail)
        if key in seen:
            return
        seen.add(key)
        findings.append({"id": f"{kind}:{route.split('?')[0]}", "kind": kind, "sev": sev, "route": route, "viewport": vp, "scheme": scheme, "detail": detail})

    widths = [w for w in a.widths.split(",") if w]
    schemes = [s for s in a.schemes.split(",") if s]
    t0 = time.time()
    with sync_playwright() as p:
        browser = getattr(p, os.environ.get("BROWSER", "firefox")).launch()
        routes = None
        for si, scheme in enumerate(schemes):
            for wi, vp in enumerate(widths):
                ctx = browser.new_context(ignore_https_errors=True, viewport={"width": VIEWPORTS[vp][0], "height": VIEWPORTS[vp][1]}, color_scheme=scheme)
                page = ctx.new_page()
                if hook:
                    hook.install(page)
                errs, bad_api = [], []
                page.on("pageerror", lambda e: errs.append(f"pageerror: {str(e)[:200]}"))
                page.on("console", lambda m: m.type == "error" and "favicon" not in m.text and "interrupted while the page was loading" not in m.text and errs.append(f"console.error: {m.text[:200]}"))
                page.on("response", lambda r: r.status >= 500 and bad_api.append(f"{r.status} {urlsplit(r.url).path}"))
                login(page, base, cfg)
                if routes is None:  # discover once, from the first logged-in page (nav) and then from each page's own links
                    if a.routes:
                        routes = [r for r in a.routes.split(",") if r]
                    else:
                        settle(page)
                        routes = []
                        for href, _t, in_nav in page.evaluate(LINKS_JS):
                            r = norm_route(href, base)
                            if r and r not in routes:
                                routes.append(r)
                queue, done = list(routes), set()
                limit = a.max_routes or cfg["max_routes"]
                while queue and len(done) < limit:
                    route = queue.pop(0)
                    if route in done:
                        continue
                    done.add(route)
                    errs.clear()
                    bad_api.clear()
                    page.goto(base + route, wait_until="load")
                    settle(page)
                    f = page.evaluate(FACTS_JS)
                    pages.append({"route": route, "viewport": vp, "scheme": scheme, "title": f["title"], "h1": f["h1"], "textLen": f["textLen"]})
                    for e in errs[:3]:
                        add("js-error", "fail", route, vp, scheme, e)
                    for e in bad_api[:3]:
                        add("http-5xx", "fail", route, vp, scheme, e)
                    if f["textLen"] < 20:
                        add("blank", "fail", route, vp, scheme, "page body empty")
                    if notfound.search(f["text"][:2000]):
                        add("not-found", "fail", route, vp, scheme, "page says 404/not found")
                    m = bad_text.search(f["text"])
                    if m:
                        add("bad-text", "fail", route, vp, scheme, f"'{m.group(0)}' shown to the user")
                    for rx in pfail:
                        m = rx.search(f["text"])
                        if m:
                            add("private-text", "fail", route, vp, scheme, f"private '{m.group(0)}' visible")
                    for rx in pwarn:
                        m = rx.search(f["text"])
                        if m:
                            add("private-text", "warn", route, vp, scheme, f"owner machine name '{m.group(0)}' visible")
                    for rx in forbidden:
                        m = rx.search(f["text"])
                        if m:
                            add("internal-name", "fail", route, vp, scheme, f"internal name '{m.group(0)}' visible")
                    if f["overflowX"] > 1:
                        add("overflow", "fail" if vp == "phone" else "warn", route, vp, scheme, f"scrolls sideways by {f['overflowX']}px")
                    for o in f["overlays"]:
                        add("canvas-overlay", "fail", route, vp, scheme, o)
                    for u in f["unlinked"]:
                        add("unlinked-summary", "fail" if route.split("?")[0] in cfg["summary_fail_routes"] else "warn", route, vp, scheme, u)
                    for e in f["empty"]:
                        add("empty-no-action", "warn", route, vp, scheme, e)
                    # follow the page's own links (one level, same origin): cards/rows that point somewhere are pages too
                    for href, _t, in_nav in page.evaluate(LINKS_JS):
                        r = norm_route(href, base)
                        if r and r not in done and r not in queue and len(queue) + len(done) < limit:
                            queue.append(r)
                    if a.shots:
                        d = os.path.join(a.shots, f"{scheme}-{vp}")
                        os.makedirs(d, exist_ok=True)
                        try:
                            page.screenshot(path=os.path.join(d, slug(route) + ".png"), full_page=True, timeout=20000)
                        except Exception as e:  # noqa: BLE001
                            add("screenshot", "warn", route, vp, scheme, str(e)[:120])
                ctx.close()
        browser.close()
    titles = {}
    for pg in pages:
        titles.setdefault(pg["title"], set()).add(pg["route"].split("?")[0])
    for t, rs in titles.items():
        if len(rs) >= 5:
            add("same-title", "warn", "*", "all", "all", f"document.title '{t}' on {len(rs)} routes")

    new_fail = [x for x in findings if x["sev"] == "fail" and x["id"] not in baseline]
    known = sorted({x["id"] for x in findings if x["sev"] == "fail" and x["id"] in baseline})
    stale = sorted(set(baseline) - {x["id"] for x in findings})
    report = {"base": base, "routes": len(routes or []), "pages_checked": len(pages), "seconds": int(time.time() - t0),
              "new_fail": len({x["id"] for x in new_fail}), "baseline_hit": known, "baseline_stale": stale, "findings": findings}
    if a.report:
        json.dump(report, open(a.report, "w"), indent=1)
    by = {}
    for x in findings:
        by.setdefault((x["sev"], x["id"]), []).append(x)
    for (sev, fid), xs in sorted(by.items(), key=lambda kv: (kv[0][0] != "fail", kv[0][1])):
        tag = "KNOWN" if fid in baseline else sev.upper()
        x = xs[0]
        print(f"{tag:5} {fid}  [{x['viewport']}/{x['scheme']}{' +%d more' % (len(xs) - 1) if len(xs) > 1 else ''}] {x['detail']}")
    if stale:
        print(f"note: baseline entries no longer reproduce (remove them): {', '.join(stale)}")
    print(f"ux_crawl: {len(pages)} page loads over {len(routes or [])} routes in {report['seconds']}s; "
          f"{report['new_fail']} new fail, {len(known)} known, {sum(1 for x in findings if x['sev'] == 'warn')} warn")
    sys.exit(1 if new_fail else 0)


if __name__ == "__main__":
    main()
