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


# ---- guest-side snippets that detect the init at run time (the encrypted-ZFS / desktop gates in zfs-enc-test.py run them; they have no
# --init argument because the same harness drives every image). `/run/systemd/system` is the same test as crate::init::detect.
IS_SYSTEMD = "[ -d /run/systemd/system ]"


def enabled_list_sh():
    """one line per service enabled at boot: `<name> | <runlevel>` (OpenRC rc-update show) or `<name> enabled` (systemd, `.service` stripped)"""
    return (f"if {IS_SYSTEMD}; then systemctl list-unit-files --state=enabled --no-legend --plain 2>/dev/null | sed -E 's/\\.(service|target|socket|timer) / /'; "
            "else rc-update show -v 2>/dev/null | grep -v '|[[:space:]]*$'; fi")


def failed_sh(pattern=""):
    """`crashed <name>` per crashed (OpenRC) / failed (systemd) service, optionally only names matching the ERE `pattern`"""
    g = f" | grep -E '{pattern}'" if pattern else ""
    return (f"if {IS_SYSTEMD}; then systemctl --failed --no-legend --plain 2>/dev/null | awk '{{print \"crashed\", $1}}'{g}; "
            f"else rc-status -a 2>/dev/null | grep -Ei 'crashed' | awk '{{print \"crashed\", $1}}'{g}; fi")


def running_list_sh(pattern=""):
    """one line per running service (name first), optionally only names matching the ERE `pattern`"""
    g = f" | grep -E '{pattern}'" if pattern else ""
    return (f"if {IS_SYSTEMD}; then systemctl list-units --type=service --state=running --no-legend --plain 2>/dev/null | awk '{{sub(/\\.service$/, \"\", $1); print $1, \"started\"}}'{g}; "
            f"else rc-status -s 2>/dev/null | grep -E 'started' | tr -s ' '{g}; fi")


def stopped_sh(pattern):
    """`stopped <name>` for every service matching `pattern` that is enabled at boot but not running (OpenRC rc-status -a; systemd is-active)"""
    return (f"if {IS_SYSTEMD}; then for u in $(systemctl list-unit-files --type=service --state=enabled --no-legend --plain 2>/dev/null | awk '{{print $1}}' | grep -E '{pattern}'); do "
            "systemctl -q is-active $u || echo \"stopped ${u%.service}\"; done; "
            f"else rc-status -a 2>/dev/null | grep -Ei 'stopped' | grep -E '{pattern}' | awk '{{print \"stopped\", $1}}'; fi")
