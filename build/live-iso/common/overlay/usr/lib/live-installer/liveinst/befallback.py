"""Boot-environment auto-fallback for `horcrux-be` (docs/design/boot-environments.md, phase 2).

A boot environment that was just activated (an upgrade rolled forward, a rollback, `horcrux-be activate`) is on TRIAL: GRUB counts every
boot of it in the GRUB environment block (`/boot/grub/grubenv`, plain ext4 so GRUB can write it) and, when it has been booted
`FALLBACK_TRIES` times without ever being declared healthy, boots the environment that was running before instead. A health service
(`horcrux-be-health`: an OpenRC script and a systemd unit running the same command) declares the boot good once the system is up.

grubenv variables (names without the `be-` menu id prefix):
  hx_trial   environment on trial (empty = nothing to prove)
  hx_tries   one `x` per boot of the trial environment so far (GRUB has no arithmetic: it appends, the cfg compares strings)
  hx_good    the environment to return to (the one that was running when the trial was armed)
  hx_failed  set by GRUB when it gave up on hx_trial; the next healthy boot turns it into an event + a settled `bootfs`

Nothing here touches the init system: GRUB counts, the health command is init-neutral Python, only the two tiny service files differ.
Pure helpers + thin wrappers over `grub-editenv` (the tests fake it).
"""
import json
import os
import socket
import subprocess
import time

GRUBENV = "grub/grubenv"          # relative to /boot
EVENTS = "/var/log/horcrux-be-events.jsonl"   # /var/log is a shared dataset: events survive every rollback and fallback
VARS = ("hx_trial", "hx_tries", "hx_good", "hx_failed")
FAILED_PROP = "horcrux:failed"


def enabled(c):
    return c.get("FALLBACK", "yes") == "yes"


def tries_max(c):
    try:
        return max(1, min(9, int(c.get("FALLBACK_TRIES", "3"))))
    except ValueError:
        return 3


def panic_secs(c):
    try:
        return max(0, min(600, int(c.get("PANIC_REBOOT", "10"))))
    except ValueError:
        return 10


def grub_preamble(c, active):
    """grub.cfg lines (after `search --set=root`): pick the default entry, counting boots of a trial environment.
    `active` is the pool's bootfs environment; the trial logic only applies while it IS the environment on trial."""
    default = f'set default="be-{active}"' if active else ""
    if not enabled(c) or not active:
        return [default] if default else []
    full = "x" * tries_max(c)
    env = '"${hx_env}"'
    return [
        'set hx_env="($root)/grub/grubenv"',
        f"load_env -f {env} hx_trial hx_tries hx_good hx_failed",
        default,
        f'if [ "${{hx_trial}}" = "{active}" ]; then',
        f'  if [ "${{hx_tries}}" = "{full}" -a -n "${{hx_good}}" ]; then',
        '    set default="be-${hx_good}"',
        '    set hx_failed="${hx_trial}"',
        f"    save_env -f {env} hx_failed",
        "  else",
        '    set hx_tries="${hx_tries}x"',
        f"    save_env -f {env} hx_tries",
        "  fi",
        "fi",
    ]


def _editenv(path, *args):
    return subprocess.run(["grub-editenv", path, *args], capture_output=True, text=True)


def env_path(boot):
    return os.path.join(boot, GRUBENV)


def env_read(boot="/boot"):
    p = env_path(boot)
    if not os.path.exists(p):
        return {}
    r = _editenv(p, "list")
    out = {}
    for line in r.stdout.splitlines():
        k, _, v = line.partition("=")
        if k in VARS:
            out[k] = v
    return out


def env_set(boot, **kv):
    """set (value) or remove (None) grubenv variables; creates the block when missing"""
    p = env_path(boot)
    if not os.path.exists(p):
        os.makedirs(os.path.dirname(p), exist_ok=True)
        _editenv(p, "create")
    sets = [f"{k}={v}" for k, v in kv.items() if v is not None]
    if sets:
        r = _editenv(p, "set", *sets)
        if r.returncode != 0:
            raise OSError(f"grub-editenv set: {r.stderr.strip()}")
    for k, v in kv.items():
        if v is None:
            _editenv(p, "unset", k)


def record_event(kind, _file=None, **kw):
    """append one JSON line; never raises (events are information, not control flow)"""
    path = _file or EVENTS
    ev = {"ts": int(time.time()), "kind": kind, **kw}
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "a") as f:
            f.write(json.dumps(ev) + "\n")
    except OSError:
        pass
    return ev


def read_events(limit=30, path=None):
    path = path or EVENTS
    try:
        lines = open(path).read().splitlines()
    except OSError:
        return []
    out = []
    for l in lines[-limit:]:
        try:
            out.append(json.loads(l))
        except ValueError:
            continue
    return out


def state(c, boot="/boot"):
    """what the UI shows about the fallback machinery"""
    e = env_read(boot)
    return {"enabled": enabled(c), "tries_max": tries_max(c), "trial": e.get("hx_trial") or None,
            "tries": len(e.get("hx_tries", "")), "good": e.get("hx_good") or None, "failed": e.get("hx_failed") or None}


def arm(c, name, previous, boot="/boot", has_kernel=lambda be: True, healthy_service=lambda: service_enabled("horcrux-be-health")):
    """put `name` on trial with `previous` as the way back. No-op (returns False) when fallback is off, there is no different previous
    environment or it has no kernel to boot; activating the running environment itself clears any trial."""
    if not enabled(c):
        return False
    if not previous or previous == name or not has_kernel(previous) or not healthy_service():
        # no health service = nothing could ever end the trial, and GRUB would send a good environment back after N boots
        env_set(boot, hx_trial=None, hx_tries=None, hx_failed=None)
        return False
    env_set(boot, hx_trial=name, hx_tries=None, hx_good=previous, hx_failed=None)
    record_event("armed", be=name, previous=previous, tries_max=tries_max(c))
    return True


def settle(c, running, boot="/boot", set_bootfs=None, mark_failed=None, regen=None):
    """called by the health command once the running system is healthy; returns the event kind it recorded, or None.
      running == trial        -> boot-ok: the trial is over
      GRUB gave up (hx_failed) -> fallback: bootfs follows what actually booted, the failed environment is flagged
      anything else (user picked another menu entry by hand) -> leave the trial alone"""
    e = env_read(boot)
    trial, failed = e.get("hx_trial", ""), e.get("hx_failed", "")
    if not trial and not failed:
        return None
    if failed and running != failed:
        tries = len(e.get("hx_tries", "")) or tries_max(c)
        env_set(boot, hx_trial=None, hx_tries=None, hx_failed=None, hx_good=running)
        if set_bootfs:
            set_bootfs(running)
        if mark_failed:
            mark_failed(failed)
        if regen:
            regen()
        record_event("fallback", be=running, failed=failed, tries=tries,
                     detail=f"{failed} did not come up healthy in {tries} boots; booted {running} instead")
        return "fallback"
    if trial and running == trial:
        env_set(boot, hx_trial=None, hx_tries=None, hx_failed=None, hx_good=running)
        record_event("boot-ok", be=running, detail=f"{running} booted healthy; fallback trial over")
        if regen:
            regen()
        return "boot-ok"
    return None


def service_enabled(name, root="/"):
    """init-neutral: is `name` enabled to start at boot (an OpenRC runlevel symlink or a systemd .wants link)?"""
    import glob
    pats = [f"{root}/etc/runlevels/*/{name}", f"{root}/etc/systemd/system/*.wants/{name}.service"]
    return any(glob.glob(p) for p in pats)


def api_listening(port=8006, host="127.0.0.1"):
    try:
        with socket.create_connection((host, port), timeout=3):
            return True
    except OSError:
        return False


def health_problem(c, root="/", api=api_listening, run=subprocess.run):
    """None when the running system is healthy, else a one-line reason"""
    if run(["zpool", "list", "-H", "-o", "health", c.get("POOL", "rpool")], capture_output=True, text=True).stdout.strip() not in ("ONLINE", "DEGRADED"):
        return "the root pool is not online"
    if service_enabled("horcrux-api", root) and not api():
        return "horcrux-api is enabled but not answering on port 8006"
    cmd = c.get("HEALTH_CMD", "").strip()
    if cmd and run(cmd, shell=True, capture_output=True, text=True, timeout=120).returncode != 0:
        return f"HEALTH_CMD failed: {cmd}"
    return None


def wait_healthy(c, timeout=None, pause=5, root="/", sleep=time.sleep, problem=None):
    """(ok, reason): poll until healthy or the deadline (HEALTH_TIMEOUT, default 300 s)"""
    timeout = int(c.get("HEALTH_TIMEOUT", "300")) if timeout is None else timeout
    problem = problem or (lambda: health_problem(c, root))
    end, why = time.monotonic() + timeout, "no check ran"
    while True:
        why = problem()
        if why is None:
            return True, ""
        if time.monotonic() >= end:
            return False, why
        sleep(pause)
