"""Init-system-aware shell snippets for the QEMU tests (owner rule 2026-11-13: Horcrux and Patronus support OpenRC *and* systemd).

qemu-test.py runs these over ssh inside the installed guest; the guest's init is the `--init` argument (the image's INIT, see
image-name.sh). Pure functions, unit-tested in tests/test_init_cmds.py: no VM needed.
"""
import re

INITS = ("openrc", "systemd")

# what the first console lines of a boot look like on either init (kernel banner, or the init's own banner)
BOOT_RE = r"(Linux version|Booting|OpenRC|systemd)"


def _check(init):
    if init not in INITS:
        raise ValueError(f"unknown init {init!r} (expected one of {INITS})")


def service_status_cmd(init, svc):
    """Prints the service's state text; `started`/`active` mean running (see service_running_re)."""
    _check(init)
    return f"rc-service {svc} status 2>&1" if init == "openrc" else f"systemctl is-active {svc} 2>&1"


def service_running_re(init):
    _check(init)
    return r"started" if init == "openrc" else r"^active$"


def services_health_sh(init, services):
    """Shell lines that print `BAD service ...` for every non-running service and `BAD crashed/failed services` for a failed unit."""
    _check(init)
    names = " ".join(services)
    if init == "openrc":
        return [
            f'for s in {names}; do st=$(rc-service $s status 2>&1); case "$st" in *started*) ;; *) echo "BAD service $s: $st";; esac; done',
            'c=$(rc-status --crashed 2>/dev/null); [ -z "$c" ] || echo "BAD crashed services: $c"',
        ]
    return [
        f'for s in {names}; do st=$(systemctl is-active $s 2>&1); [ "$st" = active ] || echo "BAD service $s: $st"; done',
        'c=$(systemctl --failed --no-legend --plain 2>/dev/null | awk \'{print $1}\' | tr "\\n" " "); [ -z "$c" ] || echo "BAD crashed services: $c"',
    ]


def service_status_with_logs_cmd(init, svc, logs_tail=""):
    """`<status>; ls ...` diagnostic used when the web UI does not answer."""
    return service_status_cmd(init, svc) + ("; " + logs_tail if logs_tail else "")


def boot_diag_cmd(init):
    """Stopped/crashed services + zram state, for the zram-not-active diagnostic."""
    _check(init)
    first = "rc-status -a 2>&1 | grep -iE 'zram|stopped|crashed'" if init == "openrc" else "systemctl list-units --all --no-legend 2>&1 | grep -iE 'zram|inactive|failed'"
    return first + "; dmesg 2>/dev/null | grep -i zram | tail -5; zramctl 2>&1; cat /proc/swaps"


def boots_with(init, console_text):
    """True when the captured console text shows the boot of this init (used by the unit test fixtures)."""
    _check(init)
    return re.search(r"OpenRC" if init == "openrc" else r"systemd", console_text) is not None
