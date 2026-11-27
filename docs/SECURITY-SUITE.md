# Security: vulnerability scans and the hardening baseline

Security (System menu > Security, or `/security`) has four pages:

- **Overview**: critical and high findings across the newest scan of each target, how many
  targets have been scanned, and the node's hardening score. Every number links to the list
  behind it.
- **Vulnerabilities**: every container image (Podman, Docker), container (LXC, Incus) and virtual
  machine on this node, the newest scan of each, and buttons to scan one or all of them. A scan's
  page lists every finding with the package, the installed version and the version that fixes it.
- **Hardening**: the Horcrux baseline, checked live, with a score, per-rule status, what to do
  for each failing rule, **Apply baseline**, per-rule Apply and Revert, and a downloadable report
  (HTML for people, text, JSON for tools).
- **Settings**: everything below, also settable as the declarative `security:` section.

Design and the comparison with other platforms: [design/security-suite.md](design/security-suite.md).

## Host vulnerabilities (Gentoo)

The Overview page lists the packages installed on this node that a published Gentoo security
advisory (GLSA) names as vulnerable: advisory, severity, CVE ids, installed version, the version
that fixes it, and the update command. Horcrux reads the advisories from the synced Gentoo tree
(`/var/db/repos/gentoo/metadata/glsa`) and compares them with `/var/db/pkg`, with the same
rules as `glsa-check -t all`. It never changes anything; update with

    emerge --update --changed-use --newuse --deep --with-bdeps=y @world

or from System > Packages. Re-check with **Check now**; otherwise it runs every 24 hours, and a
new advisory that affects the node sends a `security` notification. Advisories you accept can be
listed under `host_cve.ignore`. A node without Portage shows that on the card instead of a list.

## Vulnerability scanning

Horcrux runs a free scanner as a separate program:

- **Grype** (Anchore, Apache-2.0), the default. Gentoo: `emerge app-containers/grype`, or build
  Horcrux with `USE=security`, which pulls it in.
- **Trivy** (Aqua Security, Apache-2.0), the alternative: pick it in Settings.

Without the scanner the Vulnerabilities page says so and links here; nothing else is affected.

What is scanned:

| Target | How |
|---|---|
| Podman / Docker image | read through the runtime (`podman:<ref>`, `docker:<ref>`) |
| LXC container | its root file system, `<lxc_path>/<name>/rootfs` (default `/var/lib/lxc`) |
| Incus container | its root file system, `<incus_path>/containers/<name>/rootfs` (default `/var/lib/incus`) |
| VM (stopped) | its disk attached read-only with `qemu-nbd --read-only` and the guest file system mounted read-only, scanned as a directory; the image is never written |
| VM (running) | the guest agent's package list (`horcrux-guest-agent --inventory`), turned into an SBOM the scanner reads; the host never opens the live disk |

Virtual machines are scanned without ever touching a running guest's disk. A **stopped** VM is
scanned from a read-only mount of its first disk (dpkg, rpm, apk, Portage or the Windows file
system); an **encrypted** guest disk (LUKS, BitLocker) is skipped with a clear reason — Horcrux
never unlocks a guest disk, so scan an encrypted guest while it is running instead. A **running**
VM is scanned from the Horcrux guest tools' package inventory over the guest-agent channel, so
nothing on the host reads the guest file system. Turn either off with `vuln_scan.vm_disk_scan`
(stopped) or `vuln_scan.vm_agent_scan` (running).

Scans run one at a time by default, at low CPU priority (niceness 15) and in the idle IO class,
and are stopped after 30 minutes. By default every target is scanned once a day; a restart does
not rescan everything, because the next scan is due a day after the newest one on record.

A finished scan with findings at or above **Notify at or above** (default: critical) sends a
`security` notification through Monitoring > Notifications.

### Nodes without internet

The scanner downloads its vulnerability database on first use and refreshes it before each scan.
For an air-gapped node, copy the database in and turn **Update the database before scanning**
off, or point **Database mirror** at your own copy (Grype: the URL of a `listing.json` /
`latest.json`; Trivy: an OCI repository such as `registry.example/trivy-db`). The database lives
in `<data_dir>/security/scanner-db` unless **db_dir** says otherwise.

## Hardening baseline

The Horcrux baseline (`horcrux-baseline-1`) is the CIS Linux benchmark adapted to a hypervisor:

| Category | Checks |
|---|---|
| kernel | ASLR, kernel pointer and log restriction, Yama ptrace scope, unprivileged eBPF, BPF JIT hardening, set-UID core dumps, protected hard links/symlinks/FIFOs/regular files |
| network | ICMP redirects (accept/send/secure), source routing, martian logging, SYN cookies, broadcast ping, bogus ICMP errors |
| files | modes of /etc/shadow, /etc/gshadow, /etc/passwd, /etc/group, sshd_config, /etc/crontab, the Horcrux config |
| ssh | root login with a password, empty passwords, MaxAuthTries, X11 forwarding |
| accounts | only root has UID 0, no empty passwords |
| mounts | /tmp and /dev/shm options |
| hypervisor | every CPU vulnerability the kernel knows is mitigated |
| services | host firewall loaded, time sync, API access log, auditd, a file integrity checker, no telnet/rsh/tftp servers |

It never turns off IP forwarding, strict reverse-path filtering or user namespaces: guests,
routed networks and rootless/unprivileged containers need them.

**Apply** changes only two kinds of things, and shows each change before it is made:

- kernel settings: written now and to `/etc/sysctl.d/60-horcrux-hardening.conf`, which OpenRC's
  `sysctl` service and `systemd-sysctl` read at every boot;
- file modes: the offending permission bits are removed.

Horcrux records the previous value of each, and **Revert** puts it back (and removes the drop-in
when nothing is left in it). SSH, account, mount and package findings say exactly what to do;
Horcrux never changes those itself, because a wrong SSH change can lock you out.

The baseline is re-checked once a day; a rule that starts failing sends a `security`
notification. The score weights critical rules 8, high 4, medium 2 and the rest 1.

To skip a rule (for example `kernel-ptrace-scope` on a host where you debug processes), add its id
to **Excluded rules**. To require a different value, use an override:

```yaml
security:
  hardening:
    exclude: [kernel-ptrace-scope]
    overrides:
      kernel-aslr: "2"
```

## Runtime detection (containers)

Optional and **off by default**. When on, Horcrux supervises one
[Falco](https://falco.org) process (Apache-2.0, modern eBPF probe, no kernel
module, kernel 5.8+ with BTF) with a curated rule set, keeps what it detects
under Security > Runtime and sends it through Notifications as source
`security`.

- Install Falco with the `runtime-detection` USE flag of Horcrux
  (`app-containers/falco-bin`), or point `runtime.falco_path` at your own.
- Curated rules (`GET /api/security/runtime/rules`): interactive shell in a
  container, a file below `/etc` opened for writing in a container (package
  managers are excluded), a privileged container's init starting, crypto-miner
  names and command lines, and outbound connections to typical mining-pool
  ports. Add your own rule files with `runtime.extra_rules_files`; turn a
  curated rule off with `runtime.disabled_rules`; ignore a noisy container
  (for example a build container) with `runtime.ignore_containers`.
- The process runs at `runtime.nice` inside a cgroup v2 group
  (`horcrux-runtime-detection`) limited to `runtime.cpu_limit_percent` and
  `runtime.memory_limit_mb`. Without cgroup v2 or root it still runs and the
  status says "no cgroup limits".
- Measured cost (spike on an 8-core AWS node, kernel 7.0, Falco 0.45.2, a
  worst-case fork/exec/file loop in a container, 25k-175k kernel events per
  second): 12-30 % of one core, about 135 MB resident, no kernel drops; the
  workload slowed by 0-4 % on a quiet host and 14-34 % while the host was also
  busy with CI. A normal node produces a few hundred events per second.

| Setting | Default | Meaning |
|---|---|---|
| `runtime.enabled` | `false` | run the detector |
| `runtime.falco_path` | `falco` | program (name in PATH or absolute path) |
| `runtime.plugin_dir` | `/usr/share/falco/plugins` | where `libcontainer.so` lives |
| `runtime.extra_rules_files` | `[]` | your own rule files (absolute paths, max 16) |
| `runtime.disabled_rules` | `[]` | curated rule names that are switched off |
| `runtime.ignore_containers` | `[]` | container names whose detections are dropped |
| `runtime.cpu_limit_percent` | `100` | cgroup CPU limit, % of one core (5-800) |
| `runtime.memory_limit_mb` | `1024` | cgroup memory limit (768-65536; 512 was killed at start) |
| `runtime.nice` | `10` | CPU niceness (0-19) |
| `runtime.min_priority` | `warning` | lowest priority kept (`notice`, `warning`, `error`, `critical`) |
| `runtime.notify_min_priority` | `error` | lowest priority that notifies |
| `runtime.notify_dedupe_secs` | `300` | the same rule in the same container notifies at most this often |
| `runtime.keep_events` | `5000` | detections kept (100-100000) |
| `runtime.restart_delay_secs` | `30` | wait before restarting an exited detector |

Changing the program, plugin directory, rule files, disabled rules or limits
restarts the detector; every other setting applies at once.

## Settings reference

`GET/PUT /api/security/settings`, declarative `security:` (`docs/design/declarative-config.md`).

| Key | Default | Meaning |
|---|---|---|
| `vuln_scan.enabled` | `true` | scanning on for this node |
| `vuln_scan.scanner` | `grype` | `grype` or `trivy` |
| `vuln_scan.scanner_path` | empty | program path; empty = the scanner's name in PATH |
| `vuln_scan.db_url` | empty | database mirror (see above) |
| `vuln_scan.db_dir` | empty | database directory; empty = `<data_dir>/security/scanner-db` |
| `vuln_scan.update_db` | `true` | refresh the database before a scan |
| `vuln_scan.schedule_hours` | `24` | hours between scheduled scans; 0 = on demand only |
| `vuln_scan.schedule_kinds` | all | `podman`, `docker`, `lxc`, `incus`, `vm` |
| `vuln_scan.max_concurrent` | `1` | scans at the same time (1-16) |
| `vuln_scan.nice` | `15` | CPU niceness of the scanner (0-19) |
| `vuln_scan.timeout_secs` | `1800` | a scan that runs longer is stopped |
| `vuln_scan.keep_scans` | `200` | finished scans kept |
| `vuln_scan.only_fixed` | `false` | leave out findings without a fix |
| `vuln_scan.notify_min_severity` | `critical` | `none`, `critical`, `high`, `medium`, `low` |
| `vuln_scan.lxc_path` | `/var/lib/lxc` | LXC container directory |
| `vuln_scan.incus_path` | `/var/lib/incus` | Incus data directory |
| `vuln_scan.vm_disk_scan` | `true` | scan stopped VMs from a read-only disk mount |
| `vuln_scan.vm_agent_scan` | `true` | scan running VMs from the guest agent's inventory |
| `vuln_scan.vm_mount_timeout_secs` | `120` | give up a stopped-VM disk mount after this long (10-3600) |
| `hardening.exclude` | `[]` | rule ids not checked or applied |
| `hardening.overrides` | `{}` | rule id -> wanted sysctl value |
| `hardening.sysctl_file` | `/etc/sysctl.d/60-horcrux-hardening.conf` | the managed drop-in |
| `hardening.schedule_hours` | `24` | hours between scheduled checks; 0 = on demand only |
| `hardening.notify_on_regression` | `true` | notify when a rule starts failing |
| `host_cve.enabled` | `true` | host advisory check on |
| `host_cve.glsa_dir` | `/var/db/repos/gentoo/metadata/glsa` | directory of GLSA XML files |
| `host_cve.vdb_dir` | `/var/db/pkg` | installed-package database |
| `host_cve.arch` | empty | architecture such as `amd64`; empty = detected |
| `host_cve.ignore` | `[]` | advisory ids not reported |
| `host_cve.update_command` | `emerge --update --changed-use --newuse --deep --with-bdeps=y @world` | shown as the way to fix |
| `host_cve.schedule_hours` | `24` | hours between scheduled checks; 0 = on demand only |
| `host_cve.notify_on_new` | `true` | notify when a new advisory affects the node |

## API

| Method and path | Purpose | Privilege |
|---|---|---|
| `GET /api/security/overview` | counts for the overview | Sys.Audit |
| `GET /api/security/settings`, `PUT` | read / change the settings (400 with the reason when invalid) | Sys.Audit / Sys.Modify |
| `GET /api/security/targets` | images and containers with their newest scan | Sys.Audit |
| `GET /api/security/scans?kind=&target=&limit=` | scans, newest first | Sys.Audit |
| `POST /api/security/scans` `{kind, target}` | start a scan (202; 424 `scanner-missing` / `scanning-disabled`) | Sys.Modify |
| `POST /api/security/scans/all` | scan every target (202) | Sys.Modify |
| `GET /api/security/scans/:id`, `DELETE` | one scan with its findings / remove it (409 while running) | Sys.Audit / Sys.Modify |
| `GET /api/security/host`, `POST /api/security/host/check` | host advisory exposure (cached) / re-check now | Sys.Audit / Sys.Modify |
| `GET /api/security/hardening` | check the baseline now | Sys.Audit |
| `GET /api/security/hardening/report?format=html\|text\|json` | report download | Sys.Audit |
| `POST /api/security/hardening/apply` `{rules?, dry_run?}` | apply the named (or every failing automatic) rule | Sys.Modify |
| `POST /api/security/hardening/revert` `{rules?, dry_run?}` | revert the named (or every applied) rule | Sys.Modify |
| `GET /api/security/runtime` | detector status (running, version, CPU, memory, drops, limits) | Sys.Audit |
| `GET /api/security/runtime/events?limit=&min_priority=&rule=&container=` | kept detections, newest first | Sys.Audit |
| `DELETE /api/security/runtime/events` | clear the kept detections | Sys.Modify |
| `GET /api/security/runtime/rules` | the curated rule file | Sys.Audit |

## Not planned: guest introspection

Reading a running guest's memory from the host (hidden processes, kernel hooks) was prototyped and measured; it is deferred because it needs per-kernel symbol profiles and a patched kernel interface that distributions do not ship. Details and numbers: docs/design/security-suite.md, "Phase 4 spike".
