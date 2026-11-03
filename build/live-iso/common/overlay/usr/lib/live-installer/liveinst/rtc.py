"""Hardware clock (RTC) for the installed system: written from a clock that has been checked against a network source.

Real laptops (gringotts, 2026-11-03) booted the installed system with an RTC months off: the live stick's clock was never synced, so nothing
wrote a good time to the hardware clock, chronyd had no sources yet, and Tailscale/TLS/emerge all failed until an agent set the date by hand.
`sync()` runs at the end of every install (before the target is unmounted): verified time -> `date -u -s` when the live clock is off ->
`hwclock -w -u`. Offline installs leave the RTC alone (never write an unverified clock): the NetworkManager dispatcher hook of
net-misc/horcrux-chrony-nm and chrony's `makestep` fix the clock at the first network.
"""
import email.utils
import subprocess
import time

# plain HTTP on purpose: a certificate check fails when the clock is months off
HTTP_SOURCES = ("http://www.google.com", "http://www.gentoo.org", "http://cloudflare.com")
MAX_SKEW = 120          # seconds the live clock may differ from the reference before it is stepped
MIN_YEAR = 2026         # a reference older than this is garbage (captive portal, cache)


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


def http_date(sh=_sh, sources=HTTP_SOURCES):
    """Epoch seconds from a plain-HTTP Date header, or None."""
    for url in sources:
        rc, out = sh(["curl", "-sI", "-m", "6", url])
        if rc != 0:
            continue
        for line in out.splitlines():
            if line.lower().startswith("date:"):
                try:
                    t = email.utils.parsedate_to_datetime(line.split(":", 1)[1].strip()).timestamp()
                except (TypeError, ValueError):
                    continue
                if time.gmtime(t).tm_year >= MIN_YEAR:
                    return t
    return None


def sync(log, sh=_sh, now=time.time):
    """Write the RTC (UTC) from a verified clock. Returns True when the hardware clock was written. `log(line)` receives one `rtc: ...` line."""
    src, ref = None, None
    if chrony_synced(sh):
        src = "chrony"
    else:
        ref = http_date(sh)
        if ref is not None:
            src = "http-date"
            skew = ref - now()
            if abs(skew) > MAX_SKEW:
                rc, out = sh(["date", "-u", "-s", f"@{int(ref)}"])
                if rc != 0:
                    log(f"rtc: NOT written: could not step the live clock by {int(skew)} s ({out.strip()[:80]})")
                    return False
                log(f"rtc: live clock was {int(skew)} s off: stepped from an HTTP Date header")
    if src is None:
        log("rtc: NOT written: no verified time source (offline install); chrony + the NetworkManager hook correct the clock at the first network")
        return False
    rc, out = sh(["hwclock", "-w", "-u"])
    if rc != 0:
        log(f"rtc: hwclock -w -u failed: {out.strip()[:120]}")
        return False
    log(f"rtc: hardware clock written (UTC) from a synced clock ({src}): {time.strftime('%Y-%m-%d %H:%M:%S', time.gmtime(now()))} UTC")
    return True
