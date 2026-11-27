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
- **Events**: one timeline of failed logins and lockouts, denied and failing API calls, permission
  denials, account and policy changes, brute-force detections, baseline drift and new scan
  findings, with top failing clients and users, temporary blocks and click-through filters.
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

## Security activity, alerts and brute-force protection

**Security > Activity** (`/security/activity`) joins five sources into one view for a time range
(last hour, 24 hours, 7 days, 30 days):

| Source | What it adds |
|---|---|
| Audit log | failed logins, lockouts ("too many failures"), permission denials, password / two-factor / role changes, security policy and firewall rule changes |
| API access log (`/api/access-log`) | denied requests (401, 403), rate-limited (429), unknown paths (404) and server errors (5xx); the login endpoint itself is left to the audit log. Needs the access log on |
| Detector | brute-force detections, alert-rule firings, blocks and unblocks |
| Scans | a finished scan with critical or high findings |
| Hardening | rules that started failing since the last scheduled check (baseline drift) |

Every card, client, user and kind is a link that filters the list (`?kind=login_failed,lockout`,
`?client=203.0.113.9`, `?user=root`, `?q=text`, `?hours=168`). Unknown-path and server-error
events are counted for alert rules but hidden from the list unless you filter for them.
The detector's own events are kept in `<data_dir>/security-activity.jsonl`
(`detection.journal_max_entries`, default 5000); the audit log and the access log keep their own
retention.

### Brute-force detection

`detection.brute_force.failures` (N, default 5) failed logins or lockouts from one client within
`window_minutes` (M, default 10) is a brute-force detection: a critical `security` notification
(rule name "Brute force login", target = the client address), a timeline event, and, only if
you turned **block** on, a temporary block of that client. Detection runs every `check_secs`
(default 30 s); the same client fires once per window. Addresses on `allowlist` (addresses or
CIDR ranges) are never counted or blocked.

### Temporary blocks

Blocking is **off by default**. When on (or when you press Block on the Events page, or
`POST /api/security/blocks`), the client address is added to a set in its own nftables table
(`detection.block_table`, default `horcrux_secblock`, input hook, priority -150) with a timeout of
`block_minutes` (default 30). Everything from that address to this node is dropped, and the block
removes itself when the time is up; Unblock removes it early. The table is separate from the
Horcrux firewall, so rule changes never clear or reorder blocks, and blocks are put back from
`<data_dir>/security-blocks.json` after a restart. Safety: loopback, this node's own addresses,
allowlisted addresses and the address you are connected from are refused. Needs `nft`
(Gentoo: `net-firewall/nftables`); without it the page says so and nothing else changes.

### Alert rules

A rule counts events of chosen kinds in a time window and fires when the count reaches the limit
(per client, per user, or all together), then stays quiet for its cool-down. The starter rules:

| Id | Fires when |
|---|---|
| `access-denied-burst` | one client gets 30 denied API answers in 5 minutes |
| `path-scan` | one client hits 100 unknown API paths in 5 minutes |
| `permission-denials` | one user is refused 10 times in 10 minutes |

Rules are edited on Security > Settings (add, change, remove) or in the declarative `security:`
section. Every alert is a `security` notification, so Monitoring > Notifications > Matchers
can route it: its **rule** is the rule's name and its **target** the client address or user
(for example a matcher "event is security AND rule matches Brute force" to the on-call channel).

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
| `detection.enabled` | `true` | run the detector and alert rules |
| `detection.check_secs` | `30` | seconds between detector passes (10-3600) |
| `detection.journal_max_entries` | `5000` | detector, scan and drift events kept (100-100000) |
| `detection.block_table` | `horcrux_secblock` | nftables table that holds temporary blocks |
| `detection.brute_force.enabled` | `true` | detect repeated failed logins |
| `detection.brute_force.failures` | `5` | N failed logins ... |
| `detection.brute_force.window_minutes` | `10` | ... within M minutes from one client |
| `detection.brute_force.notify` | `true` | send a notification |
| `detection.brute_force.block` | `false` | also block the client in the host firewall |
| `detection.brute_force.block_minutes` | `30` | how long a block lasts |
| `detection.brute_force.allowlist` | `[]` | addresses / CIDR ranges never counted or blocked |
| `detection.rules` | 3 starter rules | list of `{id, name, enabled, kinds, path_group, group_by, threshold, window_minutes, severity, cooldown_minutes}`; kinds: `login_failed`, `lockout`, `access_denied`, `rate_limited`, `not_found`, `server_error`, `permission_denied`, `auth_change`, `policy_change`, `brute_force`, `suspicious`, `rule_fired`, `blocked`, `unblocked` |

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
| `GET /api/security/activity?hours=` | totals, timeline, top failing clients and users, recent events | Sys.Audit |
| `GET /api/security/activity/events?since=&until=&source=&kind=&severity=&client=&user=&q=&limit=&offset=` | events, newest first (`kind` may be a comma list) | Sys.Audit |
| `POST /api/security/activity/check` | run the detector now; returns what fired | Sys.Modify |
| `GET /api/security/blocks` | clients blocked now | Sys.Audit |
| `POST /api/security/blocks` `{ip, minutes?, reason?}` | block a client (400 for unsafe addresses, 424 `firewall-missing`) | Sys.Modify |
| `DELETE /api/security/blocks?ip=` | unblock (404 when not blocked) | Sys.Modify |

## Not planned: guest introspection

Reading a running guest's memory from the host (hidden processes, kernel hooks) was prototyped and measured; it is deferred because it needs per-kernel symbol profiles and a patched kernel interface that distributions do not ship. Details and numbers: docs/design/security-suite.md, "Phase 4 spike".
