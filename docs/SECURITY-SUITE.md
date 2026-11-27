# Security: vulnerability scans and the hardening baseline

Security (System menu > Security, or `/security`) has five pages:

- **Overview**: critical and high findings across the newest scan of each target, how many
  targets have been scanned, and the node's hardening score. Every number links to the list
  behind it.
- **Vulnerabilities**: every container image (Podman, Docker), container (LXC, Incus) and virtual
  machine on this node, the newest scan of each, and buttons to scan one or all of them. A scan's
  page lists every finding with the package, the installed version and the version that fixes it.
- **Hardening**: the Horcrux baseline, checked live, with a score, per-rule status, what to do
  for each failing rule, **Apply baseline**, per-rule Apply and Revert, and a downloadable report
  (HTML for people, text, JSON for tools).
- **Secrets**: passwords, SSH keys and tokens for guest provisioning, stored encrypted and
  write-only (see "Guest secrets" below), with their audit trail.
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

## Guest secrets

Cloud-init passwords, SSH keys and tokens, and the password of a Windows answer file, are never
kept in a VM's configuration, in an export or in a backup. They live in an encrypted store
(Security > Secrets) and a VM refers to them by name:

- `secret:<name>` as a whole value (`user.plain_password: secret:admin-pw`, an `ssh_keys` entry),
- `{{secret:<name>}}` inside a longer string (a `runcmd` line that needs a token),
- a Windows gaming VM's `password: secret:<name>` in `POST /api/gaming/vms`.

A stored provisioning record (`PUT /api/security/provisioning/<vm>`, or `guest_secrets.provisioning`
in the declarative spec) is refused when a password is a literal or an SSH entry is neither a public
key nor a reference. Public keys may stay literal.

How a value travels:

1. It is saved write-only (`POST /api/security/guest-secrets` `{name, kind, description, value}`;
   `PUT .../<name>` with a `value` rotates it, version +1). No call returns a value.
2. It is encrypted with AES-256-GCM using a key in its own file (`guest_secrets.key_file`, default
   `<data_dir>/guest-secrets.key`, created mode 0600 on first use). The database holds only
   ciphertext, so a copy of the database, a declarative export or a backup reveals nothing; keep the
   key file out of database backups (without it the secrets cannot be read, and that is the point).
3. At the VM's first start (or `POST /api/security/provisioning/<vm>/seed`, or a cloud-init request
   that carries references) the references are resolved and the seed disc is built in a private
   0700 directory. The disc is kept 0600 next to the VM's disk. Every injection is in the audit trail
   (who, which secret, which VM) and counts in the secret's usage.
4. The disc is removed after the first boot: as soon as the guest agent answers, or after
   `guest_secrets.seed_grace_secs` of uptime (also when the VM stops after that time). Turn it off
   with `guest_secrets.remove_seed_after_first_boot`. The Windows answer disc is removed once the
   installed Windows' agent answers, as before.
5. On upgrade, the copies the cloud-init manager used to keep for ever (`cloudinit-<vm>.iso`) are
   deleted at startup and the seeds of running VMs are watched like new ones.

A secret older than `guest_secrets.rotation_warn_days` shows "rotation due". A secret that
provisioning still names cannot be deleted. The declarative `guest_secrets:` section lists secret
names (kind, description) and `provisioning` by VM name; a spec never carries a value, so a declared
secret that does not exist yet shows as blocked until its value is set.

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
| `guest_secrets.remove_seed_after_first_boot` | `true` | delete the cloud-init disc once the guest has booted |
| `guest_secrets.seed_grace_secs` | `600` | uptime after which the disc goes without a guest agent answer (30-86400) |
| `guest_secrets.seed_poll_secs` | `15` | seconds between checks (5-3600) |
| `guest_secrets.rotation_warn_days` | `90` | flag a value older than this for rotation; 0 = never |
| `guest_secrets.audit_keep` | `2000` | audit entries kept (100-100000) |
| `guest_secrets.max_value_bytes` | `4096` | largest accepted value (16-65536) |
| `guest_secrets.key_file` | empty | encryption key file; empty = `<data_dir>/guest-secrets.key` |
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
| `GET /api/security/guest-secrets`, `POST` | list (never values) / create `{name, kind, description, value}` | Sys.Audit / Sys.Modify |
| `GET /api/security/guest-secrets/:name`, `PUT`, `DELETE` | metadata / rotate or edit / delete (409 while provisioning names it) | Sys.Audit / Sys.Modify |
| `GET /api/security/guest-secrets-audit?name=&limit=` | audit trail | Sys.Audit |
| `GET /api/security/provisioning/:vm`, `PUT`, `DELETE` | a VM's provisioning (references only) | Sys.Audit / Sys.Modify |
| `POST /api/security/provisioning/:vm/seed` | inject now: build the seed disc | Sys.Modify |
| `GET /api/security/hardening` | check the baseline now | Sys.Audit |
| `GET /api/security/hardening/report?format=html\|text\|json` | report download | Sys.Audit |
| `POST /api/security/hardening/apply` `{rules?, dry_run?}` | apply the named (or every failing automatic) rule | Sys.Modify |
| `POST /api/security/hardening/revert` `{rules?, dry_run?}` | revert the named (or every applied) rule | Sys.Modify |

## Not planned: guest introspection

Reading a running guest's memory from the host (hidden processes, kernel hooks) was prototyped and measured; it is deferred because it needs per-kernel symbol profiles and a patched kernel interface that distributions do not ship. Details and numbers: docs/design/security-suite.md, "Phase 4 spike".
