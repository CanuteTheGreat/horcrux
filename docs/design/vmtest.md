# vmtest - shared multi-VM test framework

Status: implemented (core + 2 ported tests + selftest). Lives in `tools/vmtest/` of the horcrux repo and is vendored
into patronus (`tools/vmtest/`, synced by `tools/vmtest/sync-to.sh`).

## Why

Four hand-built harnesses each re-implement QEMU command lines, serial scraping, SSH, timeouts and evidence:
`build/live-iso/common/qemu-test.py` (2.3 kLOC ISO install/boot tests), the HA suite (`horcrux-api/tests/hx_node.py`,
lab `hasuite.py`, `vmcluster/run-on-runner.sh`), Patronus netlab (`ci/netlab/run.sh` + lanes), and the e2e scripts.
None can express "N machines on a switch I can break", and `systemctl` / `rc-service` calls are scattered through them.
vmtest is the one place those live, in the style of NixOS `nixosTest`: declare machines, script them from Python.

## Rules it enforces

1. **Init-agnostic (OWNER RULE: 100% OpenRC and systemd).** A test says `node.wait_for_unit("horcrux-api")`.
   `guest.py` detects the init in the guest (`/run/systemd/system` -> systemd, else `rc-service` -> openrc) and renders
   start/stop/restart/enable/is-active/is-failed/boot-complete/logs/diagnostics for it. No `systemctl`/`rc-service`
   string may appear in a scenario (CI grep gate `tools/vmtest/tests/test_no_init_calls.py`). Different unit names per
   init: `NodeSpec.aliases={"sshd": {"systemd": "ssh"}}`.
2. **No root needed.** Networks are a user-space switch (`netsim.Hub`) joined by QEMU `-netdev socket`; no bridges, taps,
   netns or sudo. That is what makes it run inside an unprivileged Forgejo job container and on a lab laptop alike.
3. **Failure = evidence.** On any failure the runner collects, per node: serial log, QEMU stderr, VGA screenshot,
   guest `init-diag`/`status`/`net`/`proc`/`df`/`dmesg` text, per-network pcap + fault event log, `result.json`.
   Disks are deleted at the end (20-40 GB, regenerable) unless `--keep-disks`.
4. **No CPU `host` model** (native-opcode crashes on other hosts), conservative x86-64-v2 by default.
5. **Hard deadline** per test (`Test.timeout`, scaled by the TCG factor): kills every VM by pid, exit 124.
6. **Memory fit**: refuses guests whose total RAM exceeds the container cgroup cap (OOM-kill symptom is a silent hang).

## Layout

```
tools/vmtest/
  vmtest/spec.py      declarative: NodeSpec, Disk, Nic, Network, Test
  vmtest/qemu.py      one QEMU process: argv, serial unix socket + reader thread, QMP, screendump->PNG, send-key, savevm/loadvm
  vmtest/node.py      Node: the API below
  vmtest/guest.py     OpenRC / systemd command renderers, serial-shell framing, SshChannel
  vmtest/netsim.py    Hub: learning switch, partition/isolate/loss/delay/heal, pcap
  vmtest/machines.py  Machines `m`: nodes+networks, steps/check, snapshot_all, collect, teardown, result.json
  vmtest/cache.py     fetch(url, sha256) and ensure(key, build): provision once, reuse as qcow2 backing file
  vmtest/runner.py    `python3 -m vmtest run <scenario.py> --out DIR --arg k=v`
  scenarios/          selftest_alpine.py, install_smoke.py (+ ha_failover.py, netlab_*.py as they are ported)
  tests/              pure-python unit tests (no QEMU)
```

A scenario is a module with `define(args) -> Test`:

```python
def define(args):
    nodes = [NodeSpec(name=f"n{i}", image=base, nics=[Nic("user", forwards={"ssh": 22}), Nic("lan")], ssh_key=KEY) for i in (1,2,3)]
    def run(m):
        m.start_all()
        for n in m.nodes.values(): n.wait_for_ssh(); n.wait_for_unit("horcrux-api")
        m.network("lan").partition(["n1"], ["n2", "n3"])
        m.n2.wait_until_succeeds("horcrux ha status | grep -q 'leader.*n2'", timeout=90)
        m.network("lan").heal()
    return Test("ha-failover", nodes, networks=[Network("lan")], run=run)
```

## API (nixosTest names where they exist)

| Machines `m` | |
|---|---|
| `m.start_all()`, `m.nodes`, `m.<name>`, `m["name"]` | |
| `m.check(step, ok, detail)` / `m.step(...)` / `m.phase(title)` | record a result; `check` aborts on false |
| `m.snapshot_all(tag)` / `m.restore_all(tag)` | consistent multi-node checkpoint (freeze, savevm each, resume) |
| `m.network(name)` | Hub: `partition(g1, g2..)`, `isolate(n)`, `block(a,b)`, `set_loss(a,b,pct)`, `set_delay(a,b,ms)`, `heal()` |

| Node | |
|---|---|
| `start()`, `power_off()`, `crash()`, `freeze()/thaw()`, `reboot()`, `reset()`, `set_link(nic, up)` | power/fault |
| `execute(cmd) -> (rc, out)`, `succeed(*cmds)`, `fail(*cmds)` | commands; serial shell until ssh is up, then ssh (`via=`) |
| `wait_until(fn)`, `wait_until_succeeds(cmd)`, `wait_until_fails(cmd)` | polling, timeouts scaled for TCG |
| `wait_for_shell()`, `wait_for_ssh()`, `wait_for_boot()` | serial login handled automatically |
| `wait_for_unit(n)`, `wait_for_unit_stopped(n)`, `start_unit/stop_unit/restart_unit/enable_unit/disable_unit`, `unit_active`, `unit_log` | **init-agnostic** |
| `wait_for_open_port(p)`, `wait_for_file(path)`, `put_file`, `get_file` | |
| `http(label, path, method, body)` | host-side request through a user-NIC port forward (self-signed TLS ok) |
| `screenshot(name)`, `send_key`, `type_text`, `vnc` spec flag | VGA via QMP screendump (PNG) / localhost VNC for a human |
| `snapshot(tag)` / `restore(tag)`, `eject_media()`, `reset_nvram()` | |
| `q.expect(regex)`, `q.send(text)` | raw serial for installers that have no shell yet |

## Fast reruns

* `cache.fetch(url, sha256)` pins and verifies ISOs/base images.
* `cache.ensure(key, build)` builds a provisioned qcow2 once (e.g. "installed horcrux, key in authorized_keys"); every
  node then gets a throw-away qcow2 overlay (`image=`), so the cached base is never written. Key = ISO sha + answer file + vmtest version.
* `m.snapshot_all` / `restore_all` give in-run rewind (qcow2 only; UEFI NVRAM is a qcow2 pflash so savevm works).
* `VMTEST_CACHE` (default `~/.cache/vmtest`), `cache.prune()` age/size cap.

## Time scaling

KVM present and arch == host: x1. TCG: x4 on every wait and deadline (`VMTEST_SLOW` / `--slow` override).

## Running

* **Local / lab laptop**: `python3 -m vmtest run scenarios/selftest_alpine.py --out /tmp/o`. Needs qemu-system-x86_64,
  qemu-img, OVMF (uefi), openssh-client, mke2fs/openssl (install_smoke). Lab laptops: take the lab-laptop-scheduler lease first.
* **rw-man-01 / agent runs**: inside `agent-workdir`, in the `isoqa-qemu:1` image with `--device /dev/kvm`.
* **Forgejo CI**: `.forgejo/workflows/vmtest.yml` on label `kvm-x86_64`, container `isoqa-qemu`, same invocation; artifacts =
  the `--out` dir minus disks (uploaded on failure).

## Port plan for the remaining tests

| Existing | Becomes | Needs from vmtest | Order |
|---|---|---|---|
| `qemu-test.py` auto flow matrix (bios-cdrom ext4, uefi-usb zfs mirror, ...) | `install_smoke.py` + `--arg fs=...,zfs_raid=...,static_ip=...` | second disk, static IP; **done for ext4** | 1 |
| `qemu-test.py --flow tui/tui-vga/gui`, `fallback` | `install_tui.py` (uses `q.send`, `n.send_key`, `n.screenshot`) | VGA text-mode pixel helpers (move `highlight_pixels/cyan_pixels` into `qemu.py`) | 3 |
| `--restarts`, `--headless-gates`, `--tpm`, `--console-open` | scenario args / `swtpm` helper in `qemu.py` | swtpm lifecycle, `rtc_base`, `set_link` (exists) | 4 |
| `zfs-enc-test.py`, `be-boot-space-test.py`, `boot-modes-test.py` | scenarios on top of install_smoke's cached installed disk (`cache.ensure`) | keyboard passphrase typing (exists), `q.expect` on VGA | 4 |
| `horcrux-api/tests/ha_failover_e2e.py` + `hx_node.py` | `ha_failover.py`, 3 nodes on `Network("cluster")` using `partition/isolate` instead of iptables | cached installed image as base; `hx_node.py` becomes ~30 lines of `Node.http` | 2 |
| lab `hasuite.py` / `vmcluster/run-on-runner.sh` | same `ha_failover.py` run with `--arg iso=`; lab-laptop runner just calls the CLI | none | 2 |
| Patronus netlab lanes (`ci/netlab/*`) | `netlab_<lane>.py`: 2-3 Patronus nodes + client nodes on hubs; replaces netns/veth | multiple NICs per node (exists), `Hub` frame faults | 2 |
| per-product e2e under `horcrux-api/tests` | scenarios using `Node.http` + `wait_for_unit` | none | 5 |

Each port deletes its old harness only after the new scenario has been green in CI for 5 consecutive runs.

## Limits / known gaps

* aarch64 argv is carried over from qemu-test.py but not exercised by the selftest.
* `Hub` forwards at user-space speed (fine for control-plane tests; not for throughput tests, use a bridge network there).
* A hub-only NIC has no DHCP; test scripts address it statically (as the selftest does) or run dnsmasq on a node.
* savevm with swtpm or host-forward state: ports survive (same QEMU), TPM state does not; do not snapshot TPM tests.
