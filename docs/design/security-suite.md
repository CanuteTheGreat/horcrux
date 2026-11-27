# Built-in security suite

Status: phase 1 shipped (vulnerability scanning of container images and containers, host
hardening baseline with compliance report). Later phases are planned below, one tracked job each.

Horcrux is the management plane for every guest on a node. A hypervisor that is easy to run but
hard to keep secure is a liability, so security tooling is part of the product, not a list of
packages the operator has to find, wire up and watch. Everything here uses free and open-source
tooling, is a setting (node settings, `PUT /api/security/settings`, declarative `security:`),
works the same on OpenRC and systemd, and degrades to a clear "not installed, here is how" state
(HTTP 424 with a reason code) when a tool is absent.

## What the others ship (2026-11)

| Capability | Proxmox VE 9.x | VMware vSphere 9 / VCF | Nutanix AOS/AHV | Hyper-V / Azure Local | Horcrux |
|---|---|---|---|---|---|
| Host/guest firewall | yes (nftables proxmox-firewall, VM/CT rules, security groups, IPsets) | NSX (separate product), distributed firewall | Flow Network Security (microsegmentation, licensed) | Windows Firewall, SDN ACLs | yes (nftables, security groups, firewall objects); microsegmentation templates planned (phase 3) |
| Two-factor / SSO | TOTP, WebAuthn, recovery keys, OpenID Connect, LDAP/AD | SSO, MFA through identity federation | SAML, MFA via IdP | AD, Entra | TOTP/WebAuthn, PAM, LDAP, OIDC realms |
| API / admin access log | task log + syslog, no per-request API log | vCenter events, syslog | audit log | event log | per-request API access log (`/api/access-log`), audit log |
| Host hardening baseline + report | none (docs only) | Security Configuration and Hardening Guide, compliance through Aria Operations packs | SCMA: STIG baseline checked hourly and self-healed | Secured-core, OSConfig security baseline with drift control | **Horcrux baseline (phase 1)**: CIS-derived, hypervisor-safe, apply/revert, scheduled re-check with regression alerts, HTML/text/JSON report |
| Image / guest vulnerability scanning | none | none built in (Carbon Black / third-party) | none built in | Defender for Cloud (paid, cloud) | **phase 1: container images (Podman, Docker) and container root file systems (LXC, Incus); phase 2: VMs (stopped: read-only disk mount; running: guest-agent package inventory as an SBOM)** |
| Host CVE exposure | none (apt is the tool) | Lifecycle Manager patch baselines | LCM | Windows Update / Azure Update Manager | phase 2: Gentoo GLSA + ctg-vulnwatch-style feed matching for the host |
| Host IDS / integrity | none | none built in | none built in | Defender for Endpoint (paid) | **phase 2 (hx-sec-ids-integrity)**: Horcrux audit rule set + AIDE baseline/check, findings in Security > Events and notifications; phase 3: Falco-class container runtime detection |
| Guest secrets for provisioning | cloud-init passwords stored in the config | guest customization specs | cloud-init | unattend files | phase 3: secrets store references in cloud-init/unattend, never in the VM config |
| Pentest VM templates | none | none | none | none | phase 2: Kali, Parrot, Pentoo as first-class templates |
| Guest introspection | none | NSX Guest Introspection (deprecated) | none | none | phase 4 research (optional, off by default) |

Sources: Proxmox VE admin guide (pve-docs, chapters Firewall, User Management, Two-factor
authentication); Broadcom vSphere Security Configuration Guide; Nutanix Security Guide (SCMA);
Microsoft Learn: Azure Local security baseline and drift control. Claims about other products
describe their own documentation at the time of writing.

The gap the leaders leave open: on Proxmox and XCP-ng the operator gets a firewall and 2FA and
nothing that tells him whether the node or its workloads are actually in a good state. Nutanix
SCMA is the model for the baseline (checked continuously, drift reported); none of the
self-hosted platforms scan their own workloads.

## Decisions

1. **Scanner: Grype by default, Trivy as the alternative.** Both are Apache-2.0, offline-capable
   and read Podman/Docker images and plain directories. Grype is in ::gentoo
   (`app-containers/grype`, with `syft`); Trivy is only in overlays. The scanner, its path, the
   database mirror (air-gapped nodes) and its cache directory are settings. Horcrux runs the
   scanner as a separate process: no scanner code is linked into Horcrux.
2. **Scans are resource-aware.** One scan at a time by default (`max_concurrent`), CPU niceness
   15 and the idle IO class set in the child before exec, a hard time limit, results pruned to
   `keep_scans`. The schedule (default every 24 h) derives "due" from the newest scan on record,
   so a restart never rescans everything at once.
3. **Phase 1 targets are what can be read without touching a guest**: Podman and Docker images
   (`podman:`/`docker:` sources, read through the runtime) and LXC/Incus container root file
   systems (`dir:` scan of `<lxc_path>/<name>/rootfs`, `<incus_path>/containers/<name>/rootfs`).
   Running VMs are phase 2: an offline scan of a disk needs a read-only snapshot export and must
   respect encrypted guests; a running guest is better served by the guest agent's package
   inventory. Neither is a quick add, so neither is faked now.
4. **The baseline is product content, not settings.** The rule set (`horcrux-baseline-1`, 44
   rules in kernel, network, files, ssh, accounts, mounts, hypervisor, services) ships with the
   release like a benchmark. What a node does with it is configurable: `hardening.exclude`,
   per-rule `overrides` (sysctl values), the sysctl drop-in path, schedule, notifications.
5. **Hypervisor-safe by construction.** The baseline deliberately leaves out CIS items that break
   a virtualization host: IPv4/IPv6 forwarding off (NAT and routed guest networks need it), strict
   reverse-path filtering (multi-homed and routed guests), disabling unprivileged user namespaces
   (rootless Podman, unprivileged LXC), `kernel.kexec_load_disabled` and
   `unprivileged_bpf_disabled=1` (one-way until reboot; the baseline uses the revertible value 2).
   A unit test refuses any forwarding or userns key in the rule table.
6. **Only reversible changes are automatic.** Apply changes kernel parameters (written to
   `/proc/sys` now and to `/etc/sysctl.d/60-horcrux-hardening.conf`, which OpenRC's `sysctl`
   service and `systemd-sysctl` both read at boot) and file modes (bits removed). The previous
   value of each is recorded in `<data_dir>/security/hardening-state.json`; Revert restores it and
   removes the drop-in when empty. SSH daemon, account, mount and package findings are reported
   with the exact fix and never changed by Horcrux: a wrong sshd change locks the operator out.
   Every apply is previewed (dry run) in the UI before it is confirmed.
7. **Hypervisor-specific checks first-class**: unmitigated CPU vulnerabilities
   (`/sys/devices/system/cpu/vulnerabilities`, incl. "SMT vulnerable") are a critical rule because
   they are the guest-to-guest boundary; the API access log being on is a rule.
8. **Alerts go through the notification system** as source `security` (works with the matcher
   routing of hx-notification-matchers): a finished scan with findings at or above
   `notify_min_severity` (default critical), and a scheduled baseline check that finds a rule
   newly failing.
9. **Score**: weighted share of passing rules over pass + fail + manual (critical 8, high 4,
   medium 2, else 1); not-applicable and excluded rules do not count.

## Phase 1 (shipped)

- API `horcrux-api/src/api/security/` (`mod.rs` routes + scheduler, `vulnscan.rs`,
  `hardening.rs`), shared types `horcrux-common/src/security.rs`, RBAC area `security`
  (read Sys.Audit, change Sys.Modify), declarative `security:` section.
- UI: Security > Overview / Vulnerabilities (targets, scan, scan all, recent scans, scan detail
  with severity/fix filters) / Hardening (score, filters, apply baseline, per-rule apply/revert,
  HTML/text report) / Settings. Linked from the System menu.
- Tests: unit tests for both scanner parsers, command building, target validation, the scan
  store, every baseline rule class on a fake root, apply/revert round trip, exclusions and
  overrides, report rendering, settings validation, declarative plan/export; end-to-end
  `tests/it/security_tests.rs` (real binary, stand-in scanner, fake host root).
- Packaging: the `security` USE flag on app-emulation/horcrux pulls `app-containers/grype`.

## Phase 2: VM guest scanning (shipped)

New `ScanTargetKind::Vm`. A VM is never scanned by reading its live disk:

- **Running VM** (`vuln_scan.vm_agent_scan`, default on): the Horcrux guest
  agent reports the guest's installed packages — `horcrux-guest-agent
  --inventory` (new subcommand), run through the QEMU guest-agent channel
  (`guest-exec`). Linux reads dpkg, rpm, apk or Portage; Windows reads the
  "Uninstall" registry keys. The node turns the inventory into a CycloneDX
  SBOM (`GuestInventory::to_sbom`, package PURLs + `syft:distro:*`) that the
  scanner reads with `sbom:`. The host never opens the guest file system.
- **Stopped VM** (`vuln_scan.vm_disk_scan`, default on): the first disk is
  attached read-only through `qemu-nbd --read-only` (the image is never
  written), the guest file system is mounted read-only (`ro,noexec,nosuid,
  nodev`, no journal replay) and scanned with `dir:`, then unmounted and the
  nbd export disconnected (RAII guards clean up on every path). An encrypted
  volume — `crypto_LUKS` or BitLocker, by `blkid`/`lsblk` FSTYPE or the
  on-disk magic — is skipped with a clear reason; Horcrux never unlocks a
  guest disk. For an encrypted guest, scan it running through its agent.

Settings (`VulnScanSettings`): `vm_disk_scan`, `vm_agent_scan`,
`vm_mount_timeout_secs` (10-3600, default 120); `vm` is in `schedule_kinds`
by default, so VMs are scanned on the normal schedule with the same resource
caps. VM rows appear in Security > Vulnerabilities; the VM detail page shows a
Security tab/badge. The guest SBOM format is verified against real Grype
(reports real deb CVEs with fix versions from a hand-built CycloneDX SBOM).

## Phase 2: host intrusion detection and file integrity

- Code: `horcrux-api/src/api/security/ids.rs` (rule set rendering, `ausearch --format raw`
  parsing, AIDE config and JSON report, event store, query) and `ids_api.rs` (routes,
  scheduler, applying the audit settings); settings `audit`, `integrity`, `events` in
  `SecuritySettings` (API, UI, declarative `security:`).
- Audit: the rule set is product content rendered from `audit` settings into a
  `/etc/audit/rules.d/50-horcrux.rules` drop-in (augenrules merges it at boot on OpenRC
  and systemd), loaded at once with `auditctl -R`. Every rule carries a `hx-<kind>` key
  (identity, sshd, horcrux, binaries, sudo, modules, time, watch); turning audit off deletes
  only rules with those keys (`auditctl -D -k`), never other tools' rules, and the file.
  Watches are written only for paths that exist (auditctl refuses a watch whose parent is
  missing). `auditd` itself comes from sys-process/audit with its own OpenRC script and
  systemd unit; Horcrux enables and starts it through its init abstraction
  (`audit.manage_service`). Logins, failed authentications, sudo commands and account
  changes come from PAM/shadow records (USER_LOGIN, USER_AUTH failed, USER_CMD, ADD_USER, ...)
  and need no rule. `audit.immutable` (`-e 2`) is off by default: it protects the rules from
  root but makes turning them off need a reboot, so Horcrux refuses the change with a 409
  while the kernel says `enabled 2`.
- Collection: every `audit.collect_secs` (60) `ausearch --checkpoint ... --start
  checkpoint --format raw` at nice 10 and idle IO. ausearch re-reports the checkpointed
  event and a lost checkpoint (rotation, exit 10-12) replays the logs, so events are kept
  only when newer than a stored watermark (time, serial); turning audit on sets the
  watermark to now, so a node's older history is not imported.
- Integrity: AIDE configuration rendered into `<db_dir>/aide.conf` (`/usr /etc /boot`
  by default, VM disk, container and data directories excluded; paths are escaped to
  regexes anchored with `(/|$)`), JSON report (`report_format=json` must come before
  `report_url`, AIDE 0.18+), uncompressed database (AIDE may be built without zlib).
  Initialize = `aide --init` + rotate, Check = `aide --check` (exit 1-7 = differences,
  14+ = error), Accept current files = `aide --update` + rotate. One run at a time, nice 19
  and idle IO, hard timeout. A check records a summary event plus up to
  `integrity.max_file_events` per-file events (binaries, /boot, PAM, sudoers, sshd = high;
  other /etc = medium).
- Events: JSON lines in `<data_dir>/security/events.jsonl`, pruned to `events.keep`;
  Security > Events filters by source, kind, severity, user, time and text, links each
  user to their events and to the API audit log (`/audit?user=`). Notifications go out as
  event `security` (audit: one per collection run for `audit.notify_kinds`; integrity: one
  per check with differences).
- Hardening: `svc-auditd` passes when auditd runs with the Horcrux rules loaded,
  `svc-integrity` when integrity is on, a baseline exists and the last check is within
  twice the schedule (Manual when the tool is not installed, Fail when installed but off).
- Packaging: USE `ids` on app-emulation/horcrux pulls sys-process/audit and
  app-forensics/aide.
- Tests: unit tests (rule rendering incl. missing paths and merged /usr, raw audit parsing
  incl. hex values, multi-key records, PAM records, watermark, AIDE config and report,
  event store and query, settings validation), `tests/it/security_tests.rs`
  `ids_and_integrity_work_end_to_end` (real binary with stand-in tools), and
  `horcrux-api/tests/security_ids_e2e.sh` (real auditd + AIDE in a throwaway VM).

## Later phases (one job each)

| Phase | Item | Notes |
|---|---|---|
| 2 | Scanner packaging in images | current Grype as `app-containers/grype-bin` in the Horcrux overlay (::gentoo has an old ~amd64 0.86), binhost tier, server + GNOME images, `horcrux-bin` optional dependency, DB mirror docs for air-gapped nodes |
| 2 | VM guest scanning | **shipped**: see "Phase 2: VM guest scanning" below |
| 2 | Host CVE exposure | Gentoo GLSA (`glsa-check`) + package versions matched against the feed ctg-vulnwatch uses; shown on the Security overview, notification on new affecting advisories, ties into Packages > updates |
| 2 | Host IDS / integrity | **done** (see "Phase 2: host intrusion detection and file integrity" below) |
| 2 | Pentest VM template library | Kali, Parrot, Pentoo as first-class templates (download + checksum/signature check, cloud-init or answer files where the distro supports it, sensible hardware defaults, isolated network by default) |
| 3 | Security dashboard + events | one page joining access log (hx-api-access-log), audit log, failed logins, baseline drift and scan results; alerting through hx-notification-matchers |
| 3 | Guest provisioning secrets | cloud-init/unattend passwords and keys referenced from an encrypted secrets store, never stored in VM config or exports; rotation |
| 3 | Microsegmentation templates | per-guest default-deny profiles and "allow only from these groups" templates on the existing firewall objects, with a hit-count view |
| 3 | Container runtime detection | Falco-class (eBPF, Apache-2.0) rules for containers on the node, optional and resource-capped |
| 4 | Guest introspection (research) | spike done 2026-11: NO-GO for now (see "Phase 4 spike"); revisit when KVMI or a maintained per-kernel profile source exists |

## Phase 4 spike: guest introspection (VMI), result 2026-11

Question: can Horcrux offer optional, read-only, agentless checks of a running Linux guest (hidden processes, rootkit hooks in the kernel syscall table) by reading guest memory from the host? Prototype and raw outputs: docs/design/hx-vmi-spike/.

Setup: Debian 13 cloud guest, kernel 6.12.118, 2 vCPU, 2 GiB, KVM, QEMU 10.0 (Debian), on a shared AWS build host (load about 20 on 8 cores, so benchmark noise is large). Guest RAM is a `memory-backend-file,share=on`; the prototype maps that file read-only, walks the 4-level guest page tables from CR3, takes the KASLR slide from the #PF gate of the IDT (both from QMP `info registers`), then reads the task list and the syscall table.

Measured:

| Check | Result |
|---|---|
| Hidden process: `sleep` bind-mounted over with an empty dir in /proc (invisible to `ps`: 0 hits) | found by walking init_task->tasks (pid and comm both correct); cross-view diff against the guest's /proc is the detection |
| Syscall table, 460 entries, clean guest | 0 entries outside kernel text |
| Positive control: entry 39 overwritten (through the shared file) with a module address | reported at once, then restored |
| Full scan (page-table walk, 100 tasks, 460 entries), Python | 0.24 s to 0.41 s wall, no guest pause (live read) |
| Pause alternative: QMP stop+cont round trip | 33 ms mean, 46 ms max |
| Pause alternative: consistent full snapshot (`dump-guest-memory`, 2 GiB, paused) | 3.8 s guest paused |
| Guest CPU (sysbench cpu, 2 threads), median of runs | anon RAM 5914 ev/s, file-backed RAM 5693, file-backed + scan every second 5738 |
| Guest memory bandwidth (sysbench), median | 29182, 30282 and 29050 MiB/s |

The overhead is inside the noise of the shared host (run to run spread 3500 to 6300 ev/s); nothing here shows a cost above a few percent, and nothing proves it is below that either. A live read can see torn data (a task list changing during the walk): the checker must re-read and confirm a finding twice, or take the 33 ms pause for the pointer walk only.

Findings that decide the go/no-go:

1. LibVMI itself (current master builds fine with its file driver and the KVM driver enabled; library LGPL-3.0, tools and examples GPL-3.0, so linking from our Rust API is allowed, shipping GPL tools needs care). But its live KVM mode needs KVMI, which is not in any mainline, Debian or Gentoo kernel and needs a patched QEMU too. Without it LibVMI only works on a memory file or dump, i.e. the shared-file approach above, which is what we measured.
2. The kernel profile is the hard part. A scan needs exact struct offsets and symbol addresses for the guest's kernel build (a debug-symbol profile from the distro's dbgsym package or a BTF-derived profile). Debian's `/boot/System.map` is a one-line stub and the prototype had to take symbol offsets from the guest's own `/proc/kallsyms`, which a compromised guest could forge. A product needs a host-side profile store per distro kernel (hundreds of MB to GB), updated for every guest kernel update, and an UNKNOWN verdict when no profile matches. Task_struct field offsets were found by heuristics in the prototype; a product must not do that.
3. Introspection widens the host attack surface: a file-backed guest RAM is writable by anything that opens the file (the spike overwrote a syscall table entry that way), `info registers` over QMP gives register access to whoever holds the socket, and a parser walking attacker-controlled memory (page tables, linked lists) is a classic target. A product would run the reader unprivileged in its own sandbox with a read-only mapping, a bounded walk (step caps, no recursion) and no write path.
4. Constraints: breaks live migration expectations of shared-file RAM unless the node uses memfd/hugetlbfs with `share=on` consistently (already implied by vhost-user and ivshmem), is incompatible with the confidential-computing guests, gives no result for Windows (different structures, PatchGuard) or encrypted-RAM guests, and a rootkit that edits the task list or the page tables can fool a pure walker (it needs the cross-view and the hook checks above).
5. Quality of the signal: the two checks cover a narrow class (kernel rootkits that hide tasks or hook the syscall table). Modern rootkits use eBPF or ftrace and do not touch the table. The same value per effort is available from the guest agent plus host IDS (phase 2) with no profile problem.

Decision: NO-GO as a product feature in this release cycle. Revisit only if (a) KVMI or an equivalent lands in a mainline kernel, or (b) a maintained, signed per-kernel profile source exists for Debian, Ubuntu and Gentoo kernels. If revisited, ship it as an off-by-default, read-only node setting `security.guest_introspection` (`enabled: false`, `interval_secs`, `profile_dir`, `max_guests`) with an UNKNOWN verdict for any guest whose kernel has no profile, per-VM opt-in, and findings routed into Security > Events. The phase 4 row stays in the table as "researched, deferred".
