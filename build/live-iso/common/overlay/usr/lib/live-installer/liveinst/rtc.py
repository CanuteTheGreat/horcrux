"""Hardware clock (RTC) for the installed system: written from a clock that has been checked against a network source.

Real laptops (gringotts, 2026-11-03) booted the installed system with an RTC months off: the live stick's clock was never synced, so nothing
wrote a good time to the hardware clock, chronyd had no sources yet, and TLS/emerge all failed until an agent set the date by hand.
`sync()` runs at the end of every install (before the target is unmounted): verified time -> `date -u -s` when the live clock is off ->
`hwclock -w -u`. Verified time is, in order: a synced chronyd, a plain-HTTP Date header, one NTP measurement (`chronyd -Q`, UDP 123). The live
system's lease can still be in flight when the install ends, so the sources are retried for up to NET_WAIT_S seconds (env LIVEINST_RTC_NET_WAIT_S;
0 = one pass). A machine that stays offline for that whole window leaves the RTC alone (never write an unverified clock): the NetworkManager
dispatcher hook of net-misc/horcrux-chrony-nm / dhcpcd's exit hook and chrony's `makestep` fix the clock at the first network. The `NOT written`
line carries why every source failed plus the live system's address/route/resolver state, so a red gate can be read from the log alone.
"""
import email.utils
import os
import re
import subprocess
import time

# plain HTTP on purpose: a certificate check fails when the clock is months off
HTTP_SOURCES = ("http://www.google.com", "http://www.gentoo.org", "http://cloudflare.com")
MAX_SKEW = 120          # seconds the live clock may differ from the reference before it is stepped
MIN_YEAR = 2026         # a reference older than this is garbage (captive portal, cache)
NTP_SERVERS = tuple(os.environ.get("LIVEINST_RTC_NTP", "pool.ntp.org").split())   # one measurement against these when no HTTP source answers
NET_WAIT_S = int(os.environ.get("LIVEINST_RTC_NET_WAIT_S", "60"))   # how long a still-coming-up network is waited for before the install is called offline
RETRY_PAUSE_S = 5


def _sh(cmd, timeout=15):
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return p.returncode, (p.stdout or "") + (p.stderr or "")
    except (OSError, subprocess.TimeoutExpired) as e:
        return 127, str(e)


def chrony_synced(sh=_sh):
    """True when a running chronyd reports a normal leap status and a non-zero stratum."""
    rc, out = sh(["chronyc", "-n", "tracking"])
    if rc != 0:
        return False
    st = lead = None
    for line in out.splitlines():
        k, _, v = line.partition(":")
        if k.strip() == "Stratum":
            st = v.split()[0] if v.split() else None
        if k.strip() == "Leap status":
            lead = v.strip()
    return lead == "Normal" and st not in (None, "0")


def http_date(sh=_sh, sources=HTTP_SOURCES, errors=None):
    """Epoch seconds from a plain-HTTP Date header, or None. `errors` (a list) collects one short reason per failed source."""
    for url in sources:
        rc, out = sh(["curl", "-sI", "-m", "6", url])
        if rc != 0:
            if errors is not None:
                errors.append(f"http {url.split('//')[-1]}: curl rc={rc} {out.strip()[:60]}")
            continue
        for line in out.splitlines():
            if line.lower().startswith("date:"):
                try:
                    t = email.utils.parsedate_to_datetime(line.split(":", 1)[1].strip()).timestamp()
                except (TypeError, ValueError):
                    continue
                if time.gmtime(t).tm_year >= MIN_YEAR:
                    return t
        if errors is not None:
            errors.append(f"http {url.split('//')[-1]}: no usable Date header")
    return None


def ntp_time(sh=_sh, now=time.time, servers=NTP_SERVERS, errors=None):
    """Epoch seconds from ONE NTP measurement (`chronyd -Q`: measures, prints the offset, does not touch the clock), or None.
    UDP 123 is often open where plain HTTP to the three Date sources is not (and the other way round), so this is a second independent source."""
    if not servers:
        return None
    cmd = ["chronyd", "-d", "-Q", "-t", "25"] + [f"server {x} iburst maxsamples 2" for x in servers]
    rc, out = sh(cmd, 40)
    m = re.search(r"System clock wrong by (-?[0-9.]+) seconds", out)
    if not m:
        if errors is not None:
            errors.append(f"ntp {' '.join(servers)}: chronyd rc={rc} {' '.join(out.split())[-80:]}")
        return None
    t = now() + float(m.group(1))
    return t if time.gmtime(t).tm_year >= MIN_YEAR else None


def net_state(sh=_sh):
    """one line on the live system's network (for the `NOT written` log line): addresses, default route, resolver, dhcp client"""
    parts = []
    for label, cmd in (("addr", ["ip", "-br", "addr"]), ("route", ["ip", "route", "show", "default"]), ("dns", ["grep", "-h", "^nameserver", "/etc/resolv.conf"]),
                       ("dhcpcd", ["pgrep", "-c", "dhcpcd"])):
        rc, out = sh(cmd, 5)
        parts.append(f"{label}={' / '.join(out.split(chr(10)))[:110] if rc == 0 and out.strip() else 'none'}")
    return "; ".join(parts)


def reference_time(sh=_sh, now=time.time, wait=None, sleep=time.sleep, mono=time.monotonic, errors=None):
    """(source, epoch) from the first verified source that answers within `wait` seconds, else (None, None). One pass when wait is 0."""
    wait = NET_WAIT_S if wait is None else wait
    end = mono() + wait
    while True:
        if errors is not None:
            del errors[:]
        ref = http_date(sh, errors=errors)
        if ref is not None:
            return "http-date", ref
        ref = ntp_time(sh, now, errors=errors)
        if ref is not None:
            return "ntp", ref
        if mono() + RETRY_PAUSE_S >= end:
            return None, None
        sleep(RETRY_PAUSE_S)


def sync(log, sh=_sh, now=time.time, wait=None, sleep=time.sleep, mono=time.monotonic):
    """Write the RTC (UTC) from a verified clock. Returns True when the hardware clock was written. `log(line)` receives one `rtc: ...` line."""
    src, ref, errs = None, None, []
    if chrony_synced(sh):
        src = "chrony"
    else:
        src, ref = reference_time(sh, now, wait, sleep, mono, errs)
        if src is not None:
            skew = ref - now()
            if abs(skew) > MAX_SKEW:
                rc, out = sh(["date", "-u", "-s", f"@{int(ref)}"])
                if rc != 0:
                    log(f"rtc: NOT written: could not step the live clock by {int(skew)} s ({out.strip()[:80]})")
                    return False
                log(f"rtc: live clock was {int(skew)} s off: stepped from {'an HTTP Date header' if src == 'http-date' else 'an NTP measurement'}")
    if src is None:
        rc, out = sh(["chronyc", "-n", "tracking"])
        why = " ".join(l.strip() for l in out.splitlines() if l.split(":")[0].strip() in ("Reference ID", "Stratum", "Leap status"))[:160]
        log("rtc: NOT written: no verified time source (offline install); chrony + the NetworkManager hook correct the clock at the first network"
            f" [chronyc rc={rc}: {why or out.strip()[:120]}] [sources: {' | '.join(errs)[:400] or 'none tried'}] [net: {net_state(sh)}]")
        return False
    rc, out = sh(["hwclock", "-w", "-u"])
    if rc != 0:
        log(f"rtc: hwclock -w -u failed: {out.strip()[:120]}")
        return False
    log(f"rtc: hardware clock written (UTC) from a synced clock ({src}): {time.strftime('%Y-%m-%d %H:%M:%S', time.gmtime(now()))} UTC")
    return True
