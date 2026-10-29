# Horcrux roadmap to a stable 1.0 and a stable channel

Last reviewed: 2026-10-28. This page is the live checklist: update the status lines in the same commit that changes the evidence.

**Status today: pre-1.0 (Horcrux 0.1.x).** Horcrux is a Gentoo-based hypervisor (VMs and containers, web UI, clustering). It is not ready for production use. APIs, configuration and on-disk formats can still change. Daily ISOs exist for testing; they are not releases.

## What stays free

The community channel is, and stays, fully functional: every feature, the daily ISOs, the community binhost, the source (GPL-3.0). Nothing is held back or crippled to sell a subscription. A paid channel, if it ever exists, sells a tested, delayed stable stream of the same software plus best-effort email support. See [stable-channel.md](stable-channel.md).

## Order of work

1. Reach **stable 1.0** (criteria below).
2. Run it with **real free users** and a community (issue tracker, docs, feedback).
3. Only then build the **stable channel subscription**. Nothing in phase 3 is built before phases 1 and 2 are done.

## Phase 1: stable 1.0 exit criteria

All nine must be met. Status is checked against the repository, not against intentions.

| # | Criterion (measurable exit) | Status | Evidence |
|---|---|---|---|
| 1 | **Installer, GUI and TUI, complete on every supported architecture.** Supported set: x86_64 and aarch64 (riscv64 listed as unsupported unless a native builder exists). Exit: both installers pass the full QEMU matrix (UEFI, and BIOS on x86_64; unattended, serial, VGA/GUI) on each supported arch, daily, for 14 days running. | Partial. Guided installer is a text-mode wizard (VGA + serial) tested in CI on x86_64. The aarch64 ISO pipeline is in the workflow, but no aarch64 asset was on the `daily` release when checked (2026-10-28). riscv64 is not built. The graphical installer is in progress and not in the repo yet. | [ISO-INSTALL.md](ISO-INSTALL.md) (Architectures, Known gaps), [live-iso.yml](../.forgejo/workflows/live-iso.yml), [daily release](https://github.com/CanuteTheGreat/horcrux/releases/tag/daily) |
| 2 | **Unattended install, upgrade and rollback tested in CI.** Exit: a CI job installs the previous stable release, upgrades in place to the candidate, then rolls back, and asserts services, config and data are intact at each step. | Partial. Unattended install is tested on every daily build (2 of 5 QEMU runs). Upgrade is tested only as 'previous release's database migrated by HEAD'. There is no test that upgrades an installed system's packages in place, and no OS-level rollback (VM snapshot rollback is tested, but that is a guest feature). | [ISO-INSTALL.md](ISO-INSTALL.md#what-is-tested-automatically), [nightly-depth.yml](../.forgejo/workflows/nightly-depth.yml) (`depth_upgrade.rs`, `depth_vm_journey.rs`) |
| 3 | **Reboot and upgrade survival tests.** Exit: CI power-cuts or reboots an installed system at least 30 times, including right after an upgrade, and every service comes back with its configuration. | Partial. Every installer run reboots from the installed disk and probes the web UI over HTTPS. Running guests surviving API restarts/crashes and host-shutdown save-state/resume are tested. Not covered: repeated power-cut rounds on the installed host, and any reboot test after an upgrade. | [live-iso.yml](../.forgejo/workflows/live-iso.yml), [nightly-depth.yml](../.forgejo/workflows/nightly-depth.yml) (`depth_restart_crash.rs`), [guest-power.yml](../.forgejo/workflows/guest-power.yml) |
| 4 | **Portable binaries.** Exit: no build uses `target-cpu=native`; the shipped baseline runs on the oldest supported x86_64 (x86-64-v2) and a generic aarch64; CI boots the ISO on a restricted CPU model and a lab machine from each of Intel and AMD. | Partial. The ISO build pins `x86-64-v2` / generic arm64 / `rv64gc` and the QEMU test CPU is deliberately not `-cpu host`. A `target-cpu=native` build crashed on an Intel machine on 2026-10-28; the pin is in `main` but has not yet been confirmed on that Intel hardware. | [Dockerfile.app](../build/live-iso/project/Dockerfile.app), [ci-run.sh](../build/live-iso/common/ci-run.sh) |
| 5 | **Lab deployment, N weeks without a P1 bug.** N = 8. The clock starts when the lab runs a build that contains every fix for known P1 bugs, and resets on any P1. P1 = data loss, a security hole, a service that does not come back after reboot, or an install that fails on supported hardware. | Not met. The lab is the Horcrux laptops (ThinkPad 21KH, Surface Pro). ISO-INSTALL.md still says both are untested on the real device. The clock has not started. | [ISO-INSTALL.md](ISO-INSTALL.md#target-hardware-notes) |
| 6 | **Documentation complete.** Exit: install, upgrade/rollback, backup/restore, every UI page and CLI command documented; no page describes a feature that does not exist; stale session notes removed from the repo root; a link checker passes in CI. | Partial. `docs/` has install, ISO, API, CLI, RBAC, LXC, migration and save-state pages. The repo root still holds many historical session summaries that overstate completeness (the README says so). Needs a prune, a task-oriented admin guide, and a docs-vs-UI check. | [docs/](.), [README](../README.md#current-status) |
| 7 | **Security review done.** Exit: written threat model; `cargo audit`/`cargo deny` clean in CI; an independent review (third party, or at minimum a reviewer who did not write the code) with every critical and high finding fixed or explicitly accepted in writing. | Not met. `cargo audit` runs in CI and every route is RBAC-mapped by a test, but there is no threat model and no external or structured review. Secure Boot is unsupported (documented). | [ACCESS_CONTROL.md](ACCESS_CONTROL.md), [ci.yml](../.forgejo/workflows/ci.yml), [ISO-INSTALL.md](ISO-INSTALL.md#secure-boot) |
| 8 | **Signed release process.** Exit: signed git tags, a signed checksum file for each ISO and tarball, packages signed with a documented key, and a documented rotation and revocation procedure; CI refuses to publish an unsigned stable release. | Not met. `release.yml` builds binaries for 5 targets and the ISO job attaches a stable ISO with a SHA256 file. Nothing is cryptographically signed: no signed tags, no signed checksums, no signed packages. | [release.yml](../.forgejo/workflows/release.yml), [live-iso.yml](../.forgejo/workflows/live-iso.yml) |
| 9 | **Published support policy and EOL.** Exit: a public page stating which versions get fixes, for how long, how to report bugs and security issues, and what is out of scope. | Not met. No support policy and no version EOL policy exist. | none yet |

Summary at last review: 0 of 9 met, 5 partial, 4 not met. Known open bugs behind the partials are listed in the evidence column; none is hidden here.

## Phase 2: free users and community

Starts after 1.0. Exit before phase 3: the issue tracker is being used by people other than the maintainers, there is a documented way to ask for help, and the release cadence has held through at least two stable point releases. No user counts are claimed on this page; none exist yet.

## Phase 3: stable channel subscription (planned, after 1.0)

Model, agreed 2026-10-28, in the style of Proxmox:

- A paid, tested **stable update channel** (a subscription binhost/repo), plus **email support during business hours, best effort**. There is **no 24/7 SLA** and no response-time guarantee.
- The free community channel stays fully functional. No feature is removed or locked.
- Billing through Errant Solutions' Stripe account (test mode only until this phase starts).

What it requires technically (design in [stable-channel.md](stable-channel.md)):

- [ ] A separate binhost/repo with a **delayed, extra-tested promotion pipeline** from the community channel.
- [ ] **Per-subscription repo credentials** (revocable, rotatable).
- [ ] A **Stripe subscription wired to key issuance** (webhooks create, suspend and revoke keys).
- [ ] A **support inbox** through the Errant Solutions mail flow.
- [ ] A **published support policy**: business hours, best effort, scope, exclusions.
- [ ] **Terms of service with a limitation of liability.** A lawyer must review these before anything is sold. Nothing in this repository is legal advice or a final text.
- [ ] A signed release process (criterion 8) and the EOL policy (criterion 9), which phase 3 depends on.

## Not planned

Feature-gating the free channel, per-feature licensing, 24/7 coverage, telemetry as a condition of use.
