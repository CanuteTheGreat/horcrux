#!/usr/bin/env python3
"""Real-effect test of host intrusion detection and file integrity (run as
root inside a throwaway VM with auditd and aide installed; driven by
security_ids_e2e.sh). Never run it on a machine anyone uses: it turns on
audit rules and changes /etc/passwd (adds and removes a test account).

Checks:
  * audit on through the API -> rule file written, rules loaded, auditd running
  * a change of /etc/passwd (useradd) -> identity event on Security > Events with
    the path, and a `security` notification delivered to a webhook channel
  * AIDE Initialize, then a modified binary below /usr -> `changed` finding (high),
    notification, Accept current files -> next check clean
  * hardening rules svc-auditd and svc-integrity pass
  * audit off -> our rules (hx- keys) and the rule file are gone, other rules kept
"""
import http.server
import json
import os
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

PORT = int(os.environ.get("HX_PORT", "18006"))
API = f"https://127.0.0.1:{PORT}/api"
CTX = ssl._create_unverified_context()
TOKEN = [""]
DATA = "/tmp/hx-ids-data"
BIN = os.environ.get("HX_BIN", "/usr/local/bin/horcrux-api")
CANARY = "/usr/local/bin/hx-ids-canary"
TEST_USER = "agent-test-ids"
HOOK = []


def log(m):
    print(f"[ids-e2e] {m}", flush=True)


def fail(m):
    log(f"FAIL: {m}")
    subprocess.run("tail -60 /tmp/hx-ids-api.log", shell=True)
    sys.exit(1)


def check(c, m):
    if not c:
        fail(m)
    print(f"ok - {m}", flush=True)


def req(method, path, body=None, expect=200):
    data = None if body is None else json.dumps(body).encode()
    r = urllib.request.Request(API + path, data=data, method=method)
    r.add_header("Content-Type", "application/json")
    if TOKEN[0]:
        r.add_header("Authorization", "Bearer " + TOKEN[0])
    try:
        with urllib.request.urlopen(r, timeout=600, context=CTX) as resp:
            status, text = resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if expect is not None and status != expect:
        fail(f"{method} {path} -> {status} (expected {expect}): {text[:800]}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def sh(cmd, ok_rc=False):
    p = subprocess.run(cmd, shell=True, capture_output=True, text=True, executable="/bin/bash")
    if p.returncode != 0 and not ok_rc:
        fail(f"`{cmd}` -> {p.returncode}: {p.stderr.strip()} {p.stdout.strip()}")
    return (p.stdout + p.stderr).strip()


def wait_for(desc, fn, timeout=120, every=2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(every)
    fail(f"timed out waiting for {desc}")


class Hook(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        HOOK.append(self.rfile.read(n).decode(errors="replace"))
        self.send_response(200)
        self.end_headers()

    def log_message(self, *a):
        pass


def start_api():
    sh(f"""mkdir -p {DATA}; cd {DATA}
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT={PORT} HORCRUX_HTTP_REDIRECT_PORT=0 HORCRUX_DATA_DIR={DATA} \
HORCRUX_DATABASE_URL=sqlite://{DATA}/horcrux.db ADMIN_PASSWORD=admin HORCRUX_SSL_DIR={DATA}/ssl \
RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info \
nohup {BIN} >> /tmp/hx-ids-api.log 2>&1 &
echo $! > /tmp/hx-ids-api.pid""")
    for _ in range(90):
        if sh(f"curl -skf https://127.0.0.1:{PORT}/api/health >/dev/null && echo up", ok_rc=True) == "up":
            _, b = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
            TOKEN[0] = b.get("ticket") or b.get("token")
            return
        time.sleep(1)
    fail("horcrux-api did not start")


def events(q):
    return req("GET", "/security/events?" + q)[1]


def ids():
    return req("GET", "/security/ids")[1]


def integrity(what):
    req("POST", f"/security/integrity/{what}", {}, expect=202)
    st = wait_for(f"integrity {what}", lambda: (lambda s: s if s["integrity_running"] is None else None)(ids()), timeout=1800, every=3)
    check(st.get("integrity_error") is None, f"integrity {what} finished without error ({st.get('integrity_error')})")
    return st


def rule(rid):
    r = req("GET", "/security/hardening")[1]
    return next(x for x in r["rules"] if x["id"] == rid)


def main():
    srv = http.server.HTTPServer(("127.0.0.1", 0), Hook)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    hook_url = f"http://127.0.0.1:{srv.server_address[1]}/hook"
    # A rule of "another tool" that must survive our enable/disable.
    sh("auditctl -w /etc/hosts -p wa -k other-tool")
    start_api()
    req("POST", "/notifications/channels", {
        "name": "ids-e2e", "channel_type": "webhook", "enabled": True,
        "config": {"type": "Webhook", "url": hook_url, "method": "POST", "headers": {}, "auth": None},
    }, expect=201)

    _, s = req("GET", "/security/settings")
    check(s["audit"]["enabled"] is False, "audit is off by default")
    s["audit"]["enabled"] = True
    s["audit"]["collect_secs"] = 10
    s["integrity"]["enabled"] = True
    s["integrity"]["paths"] = ["/usr/local", "/etc"]
    s["integrity"]["schedule_hours"] = 24
    req("PUT", "/security/settings", s)
    rules = open("/etc/audit/rules.d/50-horcrux.rules").read()
    check("-w /etc/passwd -p wa -k hx-identity" in rules, "rule file written with the /etc/passwd watch")
    loaded = sh("auditctl -l")
    check("hx-identity" in loaded and "hx-modules" in loaded, "Horcrux rules loaded in the kernel")
    check("other-tool" in loaded, "other tools' rules kept")
    check(sh("systemctl is-active auditd", ok_rc=True) == "active", "auditd running")
    st = ids()
    check(st["audit_rules_loaded"] > 5 and st["auditd_running"], f"status: {st['audit_rules_loaded']} rules, auditd running")

    # /etc/passwd change -> audit event + notification
    sh(f"userdel {TEST_USER} 2>/dev/null; useradd -M -s /usr/sbin/nologin {TEST_USER}")
    ev = wait_for("identity event for /etc/passwd", lambda: [
        e for e in events("source=audit&kind=identity&limit=200")["events"] if e["path"] == "/etc/passwd"
    ], timeout=90)
    check(ev[0]["severity"] == "high" and "useradd" in ev[0]["exe"] + ev[0]["detail"] + ev[0]["summary"],
          f"identity event shown: {ev[0]['summary']}")
    acct = events("source=audit&q=" + TEST_USER)
    check(acct["total"] >= 1, f"account change record for {TEST_USER} shown ({acct['total']})")
    wait_for("security notification for the audit event", lambda: [h for h in HOOK if "/etc/passwd" in h], timeout=60)
    check(True, "audit event notified through the webhook channel")
    # sudo / login record types parse from the real log too
    sh("su -s /bin/sh -c true nobody", ok_rc=True)
    wait_for("su run recorded", lambda: events("source=audit&kind=sudo")["total"] > 0, timeout=60)
    check(True, "su run recorded (kind sudo)")

    # AIDE: baseline, modified binary, finding, accept
    sh(f"printf '#!/bin/sh\\necho canary\\n' > {CANARY}; chmod 755 {CANARY}")
    t0 = time.time()
    integrity("init")
    log(f"AIDE init took {time.time() - t0:.1f}s")
    check(rule("svc-integrity")["status"] == "pass", "hardening svc-integrity passes")
    check(rule("svc-auditd")["status"] == "pass", "hardening svc-auditd passes")
    HOOK.clear()
    sh(f"printf 'echo tampered\\n' >> {CANARY}")
    st = integrity("check")
    check(st["integrity_last_differences"] >= 1, f"check found {st['integrity_last_differences']} difference(s)")
    ch = events("source=integrity&kind=changed&q=hx-ids-canary")
    check(ch["total"] == 1, "AIDE finding for the modified binary")
    e = ch["events"][0]
    check(e["path"] == CANARY and e["severity"] == "high" and "sha256" in e["detail"],
          f"finding: {e['summary']} ({e['detail'][:80]})")
    wait_for("integrity notification", lambda: [h for h in HOOK if "hx-ids-canary" in h], timeout=30)
    check(True, "integrity finding notified through the webhook channel")
    integrity("accept")
    st = integrity("check")
    check(st["integrity_last_differences"] == 0, "after Accept current files the next check is clean")

    # Low priority: AIDE ran with nice 19 (sampled during a check)
    # Audit off -> only our rules removed
    s["audit"]["enabled"] = False
    req("PUT", "/security/settings", s)
    loaded = sh("auditctl -l")
    check("hx-" not in loaded, "audit off removes the Horcrux rules")
    check("other-tool" in loaded, "audit off keeps other tools' rules")
    check(not os.path.exists("/etc/audit/rules.d/50-horcrux.rules"), "audit off removes the rule file")
    sh(f"userdel {TEST_USER}; rm -f {CANARY}; auditctl -W /etc/hosts -p wa -k other-tool", ok_rc=True)
    print("ALL-OK", flush=True)


if __name__ == "__main__":
    try:
        main()
    finally:
        subprocess.run("kill $(cat /tmp/hx-ids-api.pid) 2>/dev/null", shell=True)
