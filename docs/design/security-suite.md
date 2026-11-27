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
| Host IDS / integrity | none | none built in | none built in | Defender for Endpoint (paid) | phase 2: auditd rules + AIDE, phase 3: Falco-class container runtime detection |
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
- Packaging: the `security` USE flag on app-emulation/horcrux and app-emulation/horcrux-bin (default on) pulls `app-containers/grype-bin` (see "Scanner packaging" below).

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

## Phase 2: Host CVE exposure (shipped)

Security > Overview has a **Host vulnerabilities** card: which installed packages of the node
are named by a published Gentoo security advisory (GLSA), the advisory id, severity, CVE ids,
the versions that fix it, the update command and a link to System > Packages.

- Source: the GLSA XML files of the synced Gentoo tree (`host_cve.glsa_dir`, default
  `/var/db/repos/gentoo/metadata/glsa`) against the installed-package database
  (`host_cve.vdb_dir`, `/var/db/pkg`). It applies the rules of `glsa-check -t all` (Portage
  `glsa.py`): an installed package is affected when it matches a `<vulnerable>` range and no
  `<unaffected>` range; `slot` narrows both, the `r*` ranges compare only the revision of one
  version, `arch` limits an entry. It is done in-process (version order per PMS), so it needs
  no Python and no privileges; tests use a fake root (`HORCRUX_SYSROOT`).
- ctg-vulnwatch (the owner's fleet tool) reads the same GLSA feed and uses the same matching
  idea; its code is not shipped in the product (it is private tooling), the two stay
  consistent because both follow `glsa.py`. The `own-vulnwatch-*` jobs own the fleet side.
- Schedule `host_cve.schedule_hours` (24); a new advisory affecting the node sends a `security`
  notification (`host_cve.notify_on_new`). The ids already seen are kept in
  `<data_dir>/security/host-cve-seen.json`, so a restart does not notify again and the first
  check only records them. Non-Gentoo nodes (no `/var/db/pkg`) show why the card is empty.
- Nothing is changed on the host. `host_cve.update_command` is displayed
  (`emerge --update --changed-use --newuse --deep --with-bdeps=y @world`), never run.
- API `GET /api/security/host`, `POST /api/security/host/check`.

## Phase 3: container runtime detection (spike: GO, shipped)

Falco-class detection for containers on the node, optional (`security.runtime.enabled`,
default off) and resource-capped. User guide: docs/SECURITY-SUITE.md "Runtime detection".

**Choice.** Falco 0.45.2 (Apache-2.0) with the modern eBPF probe (built into the binary, no
kernel module, needs kernel 5.8+ and BTF) and the container plugin for container names and
images. Not in ::gentoo, so `app-containers/falco-bin` in the Horcrux overlay (binary tarball
from download.falco.org, checksummed; only the binary, the plugin and /etc/falco are installed,
falcoctl and the driver sources are not), pulled by the `runtime-detection` USE flag of
`app-emulation/horcrux`. Horcrux runs it as a child process with a generated `falco.yaml` and
its own curated rule file; nothing is linked into Horcrux and the node's /etc/falco is not read.

**Spike (2026-11-27, rw-man-06: 8 vCPU AWS, kernel 7.0.0-1018-aws, Docker 29, shared with CI).**
Workload: 4 workers in a container running `true`, `echo > file`, `cat`, `rm` in a loop (a
fork/exec/open-heavy worst case, 25k-175k kernel events/s; a normal node is a few hundred).
Falco in a container with `--privileged --cpus=1 --memory=1g`.

| Measure | Result |
|---|---|
| Falco CPU (default rule set, 175k events/s) | 26 % of one core; 12-30 % with the curated set |
| Falco memory | 133-137 MB resident, ~70 MiB cgroup after warm-up; **512 MiB cap was OOM-killed at start**, 1 GiB fine |
| Kernel event drops | 0 (`scap.n_drops`), output queue drops 0 |
| Fixed work (4 x 3000 loops), quiet host, 5 interleaved pairs | 4.97 / 4.99 / 4.94 / 5.00 / 4.94 s without vs 4.98 / 5.18 / 5.00 / 4.92 / 4.98 s with: +0 to +4 % |
| Same, host busy with CI (load 4-8) | 7.1 / 7.0 / 7.0 / 8.2 / 9.3 s without vs 8.0 / 8.0 / 9.4 / 12.9 / 10.9 s with: +14 to +34 % (noisy: other jobs share the host) |

The numbers are for a deliberately hostile loop; idle container hosts see well under 1 % CPU.

**Decision: GO**, as an optional feature, with these guard rails: off by default; a cgroup v2
group with `cpu.max` (default 100 % of a core) and `memory.max` (default 1 GiB, minimum 768
MiB); nice 10; events below `min_priority` dropped; per rule+container notification dedupe;
drops are visible (`kernel_drops` in the status and UI) so an overloaded node is reported, not
silently blind. Falco's default rules were judged on the same run: they caught
"Drop and execute new binary in container" and "Read sensitive file untrusted" but not a write
below /etc by a plain process, a privileged container start or a miner by name, and they flag
every CI container's `ssh`/`apt` as critical. Horcrux therefore ships its own small rule set
(prefix `HX`): interactive shell in a container, write below /etc in a container (package
managers excluded), privileged container init, miner process or command line, outbound
connection to a mining-pool port. Each fired in the spike run on real containers.

**Not in this phase**: Kubernetes audit, the full default Falco rule set (extra files can be
added), response actions (killing containers), host (non-container) detection, VM guests.

## Scanner packaging (shipped)

- `app-containers/grype-bin` in the Horcrux overlay (`gentoo/app-containers/grype-bin`): the upstream
  Grype release binary (Apache-2.0), amd64 + arm64, thin Manifest with the release digests (compared
  with the upstream `checksums.txt` when a version is cut), nvchecker entry for bumps. It exists
  because ::gentoo only has `app-containers/grype-0.86.1` (~amd64, no arm64, a database schema the
  current Grype database no longer serves). The two packages block each other (both own `/usr/bin/grype`).
- `app-emulation/horcrux[security]` and `app-emulation/horcrux-bin[+security]` depend on it; the server
  image list (`build/live-iso/project/packages.txt`) and the GNOME edition list name it explicitly.
  The binhost builds it from the overlay (gentoo-binhost `config/horcrux-overlay-packages.txt`).
- Check: `gentoo/ci/grype-bin-check.sh` (stage3 container): emerge from the overlay, `grype version`,
  the offline database recipe and a scan of `alpine:3.10` with the imported database only.
- Offline database: see docs/SECURITY-SUITE.md, "Nodes without internet".

## Later phases (one job each)

| Phase | Item | Notes |
|---|---|---|
| 2 | Scanner packaging in images | **shipped**: see "Scanner packaging" below |
| 2 | VM guest scanning | **shipped**: see "Phase 2: VM guest scanning" below |
| 2 | Host CVE exposure | **shipped**: see "Phase 2: Host CVE exposure" below |
| 2 | Host IDS / integrity | auditd with a Horcrux rule set (logins, privilege use, /etc changes, horcrux binaries), AIDE database init + scheduled check, findings in Security > Events and as notifications; OpenRC + systemd units |
| 2 | Pentest VM template library | Kali, Parrot, Pentoo as first-class templates (download + checksum/signature check, cloud-init or answer files where the distro supports it, sensible hardware defaults, isolated network by default) |
| 3 | Security dashboard + events | one page joining access log (hx-api-access-log), audit log, failed logins, baseline drift and scan results; alerting through hx-notification-matchers |
| 3 | Guest provisioning secrets | cloud-init/unattend passwords and keys referenced from an encrypted secrets store, never stored in VM config or exports; rotation |
| 3 | Microsegmentation templates | per-guest default-deny profiles and "allow only from these groups" templates on the existing firewall objects, with a hit-count view |
| 3 | Container runtime detection | **shipped**: see "Phase 3: container runtime detection" above |
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
