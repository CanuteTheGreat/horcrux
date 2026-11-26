#!/usr/bin/env python3
"""System Log page: filters, priority colours, reboot separator and LIVE mode against a real syslog daemon.

A real rsyslogd (started by system_log_e2e.sh with its own socket and a log file that carries <PRI>) receives lines from the real
`logger`. The node reads that file (the `syslog_files` and `wtmp_path` settings are changed through the API, which also proves the
typed keys), and the page is driven in a real browser:

  * a minimum-priority choice hides lower-priority lines and the error line is coloured as an error (class prio-3)
  * the identifier filter takes a `*` wildcard; the kernel-only box hides lines that are not from the kernel
  * a reboot separator stands between two boots (two BOOT_TIME records in a wtmp file; older lines belong to the first boot)
  * Live: `logger -t hxtest ...` after switching Live on shows up on the page without a reload; switching Live off stops it

Env: BASE_URL, HORCRUX_CA_FILE, HX_SYSLOG_SOCK (logger -u), HX_SYSLOG_FILE, HX_WTMP_FILE.
"""
import json
import os
import ssl
import struct
import subprocess
import time
import urllib.error
import urllib.request

from playwright.sync_api import expect, sync_playwright

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
_CA = os.environ.get("HORCRUX_CA_FILE")
SSL = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
SOCK = os.environ["HX_SYSLOG_SOCK"]
LOGFILE = os.environ["HX_SYSLOG_FILE"]
WTMP = os.environ["HX_WTMP_FILE"]


def req(method, path, tok=None, body=None):
    h = {"Content-Type": "application/json"}
    if tok:
        h["Authorization"] = f"Bearer {tok}"
    r = urllib.request.Request(f"{BASE}{path}", method=method, headers=h,
                               data=json.dumps(body).encode() if body is not None else None)
    try:
        with urllib.request.urlopen(r, context=SSL) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raise AssertionError(f"{method} {path} -> {e.code}: {e.read()[:300]}")


def logger(tag, msg, prio="user.info"):
    subprocess.check_call(["logger", "-u", SOCK, "-t", tag, "-p", prio, msg])


def wtmp_record(ts):
    rec = bytearray(384)
    struct.pack_into("<h", rec, 0, 2)             # BOOT_TIME
    struct.pack_into("<i", rec, 340, int(ts))
    return bytes(rec)


def ok(msg):
    print(f"ok - {msg}", flush=True)


def main():
    tok = req("POST", "/api/auth/login", body={"username": "admin", "password": "admin"})["ticket"]
    now = time.time()
    # two boots: the older lines (written below, 40 min ago) belong to the first
    with open(WTMP, "wb") as f:
        f.write(wtmp_record(now - 3000) + wtmp_record(now - 1200))
    stamp = time.strftime("%b %e %H:%M:%S", time.localtime(now - 2400))
    with open(LOGFILE, "a") as f:
        f.write(f"<14>{stamp} hxhost hxold[10]: line from the first boot\n")
        f.write(f"<11>{stamp} hxhost hxold[10]: error from the first boot\n")
    s = req("GET", "/api/system/logs/settings", tok)
    s.update(syslog_files=[LOGFILE], wtmp_path=WTMP, follow_poll_ms=500)
    req("PUT", "/api/system/logs/settings", tok, s)
    ok("settings are typed keys (syslog file and wtmp path changed through the API)")
    info = req("GET", "/api/system/logs/info", tok)
    assert info["source"] == "syslog-file" and any("priority" in l for l in info["limitations"]), info
    ok("the API says it reads a syslog file and what that cannot give")

    logger("hxtest", "hxtest an informational line", "user.info")
    logger("hxtest", "hxtest an error line", "user.err")
    logger("hxother", "hxother a warning line", "user.warning")
    time.sleep(1.5)

    with sync_playwright() as p:
        b = p.firefox.launch()
        ctx = b.new_context(ignore_https_errors=True, viewport={"width": 1400, "height": 900})
        page = ctx.new_page()
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.goto(f"{BASE}/login")
        page.fill("#login-username", "admin")
        page.fill("#login-password", "admin")
        page.locator("#login-submit").click()
        page.wait_for_url(f"{BASE}/")
        page.goto(f"{BASE}/system/logs")
        rows = page.locator("tr.log-row")
        expect(rows.first).to_be_visible(timeout=30000)
        assert page.title().startswith("System Log"), page.title()
        for label in ("Time span", "Minimum priority", "Unit / service", "Syslog identifier", "Boot", "Search"):
            expect(page.locator("label.filter-group", has_text=label).first).to_be_visible()
        expect(page.locator("#log-live")).to_be_visible()
        expect(page.locator(".log-limits")).to_contain_text("syslog file")
        ok("filter bar is labelled; the source limits are shown on a syslog host")

        # priority colours + minimum priority
        err_row = page.locator("tr.log-row", has_text="hxtest an error line")
        expect(err_row).to_have_count(1)
        assert "prio-3" in err_row.get_attribute("class"), err_row.get_attribute("class")
        assert "prio-4" in page.locator("tr.log-row", has_text="hxother a warning line").get_attribute("class")
        info_row = page.locator("tr.log-row", has_text="hxtest an informational line")
        assert "prio-6" in info_row.get_attribute("class")
        page.select_option("#log-priority", "3")
        expect(page.locator("tr.log-row", has_text="hxtest an informational line")).to_have_count(0, timeout=15000)
        expect(page.locator("tr.log-row", has_text="hxtest an error line")).to_have_count(1)
        ok("minimum priority hides the info line; the error line carries the error colour class")
        page.select_option("#log-priority", "")

        # identifier wildcard
        page.fill("#log-identifier", "hxt*")
        page.locator("#log-identifier").dispatch_event("change")
        expect(page.locator("tr.log-row", has_text="hxother")).to_have_count(0, timeout=15000)
        expect(page.locator("tr.log-row", has_text="hxtest")).to_have_count(2)
        page.fill("#log-identifier", "")
        page.locator("#log-identifier").dispatch_event("change")
        expect(page.locator("tr.log-row", has_text="hxother")).to_have_count(1, timeout=15000)
        ok("identifier filter takes a * wildcard")

        # kernel only
        page.check("#log-kernel")
        expect(page.locator("tr.log-row")).to_have_count(0, timeout=15000)
        expect(page.locator(".empty-state")).to_contain_text("No log entries match")
        page.uncheck("#log-kernel")
        expect(page.locator("tr.log-row", has_text="hxother")).to_have_count(1, timeout=15000)
        ok("kernel-only hides non-kernel lines and the empty state says what to widen")

        # reboot separator between the first boot's lines and the second's
        sep = page.locator("tr.boot-sep")
        expect(sep).to_have_count(1, timeout=15000)
        assert "Rebooted" in sep.inner_text()
        old = page.locator("tr.log-row", has_text="from the first boot")
        expect(old).to_have_count(2)
        ok("a separator stands where the node rebooted")
        bsel = page.locator("#log-boot option")
        assert bsel.count() >= 3, bsel.count()   # all boots + two boots
        page.select_option("#log-boot", "-1")
        expect(page.locator("tr.log-row", has_text="hxother")).to_have_count(0, timeout=15000)
        expect(page.locator("tr.log-row", has_text="from the first boot")).to_have_count(2)
        page.select_option("#log-boot", "")
        ok("boot picker narrows to the previous boot")

        # live mode
        page.check("#log-live")
        expect(page.locator("#log-live-note")).to_contain_text("Live", timeout=15000)
        time.sleep(1.5)
        mark = f"hxtest live line {int(time.time())}"
        logger("hxtest", mark, "user.notice")
        expect(page.locator("tr.log-row", has_text=mark)).to_have_count(1, timeout=20000)
        first = page.locator("tr.log-row").first
        assert mark in first.inner_text(), "live entries go on top (newest first)"
        ok("live mode shows a line from `logger -t hxtest` without a reload")
        page.uncheck("#log-live")
        time.sleep(1)
        later = f"hxtest after live {int(time.time())}"
        logger("hxtest", later, "user.notice")
        time.sleep(3)
        expect(page.locator("tr.log-row", has_text=later)).to_have_count(0)
        ok("switching Live off stops the stream")

        # limits editor: typed keys are editable here
        page.locator(".log-settings summary").click()
        page.fill("#set-follow-idle", "300")
        page.locator("#set-follow-idle").dispatch_event("change")
        page.click("#set-save")
        expect(page.locator("#set-msg")).to_contain_text("Saved", timeout=15000)
        assert req("GET", "/api/system/logs/settings", tok)["follow_idle_secs"] == 300
        ok("the limits editor saves a typed key")
        assert not errors, errors
        b.close()


if __name__ == "__main__":
    main()
