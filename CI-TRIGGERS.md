# CI triggers (horcrux)

CI work runs **when its inputs change**, not on a clock (owner principle 2026-11-06: "if there is something to do, do it").

- Heavy workflows (nightly-*, live-iso-*, mutation, test-depth, ...) trigger on a push to `main` with `paths:` filters naming what they build or test.
  Docs/README/site-only pushes start none of them.
- Bursts coalesce: they use a `replace:` concurrency group, so the ci-supersede-sweeper keeps every started run and only the NEWEST unstarted one.
  A started run is never cancelled by a push.
- A cron stays only where the thing it checks has no change signal (upstream package trees/base images, external services, elapsed time).
  Each such cron has a comment in its workflow saying why. Priority (not the clock) handles contention: heavy workflows are matched by file name
  in the sweeper's -20 class (ctg-infrastructure `ollivanders/forgejo/ci-supersede-sweeper`).
- File names such as `nightly-*.yml` are kept on purpose (the sweeper's priority rule matches them); they are not nightly any more.

| workflow file | name | triggers |
|---|---|---|
| `ci.yml` | CI | push main tags (all paths); pull_request; cron 40 21 * * 1 (safety net, see comment in file); dispatch |
| `deploy-site.yml` | Deploy Horcrux site | push main (paths: 4 filters); dispatch |
| `docs.yml` | Documentation | push main (paths: 4 filters) |
| `guest-power.yml` | Guest save state + cluster drain (2 nodes, KVM) | push main (paths: 19 filters); dispatch |
| `ha-failover.yml` | HA failover (3 nodes, real QEMU) | push main (paths: 7 filters); dispatch |
| `incus-vm-console.yml` | Incus VM consoles (KVM) | push main (paths: 10 filters); dispatch |
| `live-iso-gnome-light-openrc-x86_64.yml` | Live ISO (gnome-light-openrc-x86_64) | push main tags (paths: 11 filters); dispatch |
| `live-iso-server-openrc-aarch64.yml` | Live ISO (server-openrc-aarch64) | push main tags (paths: 11 filters); dispatch |
| `live-iso-server-openrc-x86_64.yml` | Live ISO (server-openrc-x86_64) | push main tags (paths: 11 filters); dispatch |
| `live-iso.yml` | Live ISO (moved) | dispatch |
| `mutation.yml` | Mutation testing (nightly incremental, weekly full per group) | push main (paths: 8 filters); dispatch |
| `nightly-depth.yml` | Nightly depth (journeys, restart/crash, upgrade) | push main (paths: 8 filters); dispatch |
| `nightly-realistic-test.yml` | Nightly Realistic VM/Container/CNI Test | push main (paths: 8 filters); dispatch |
| `nightly-stress-test.yml` | Nightly Stress / Adversarial Test | push main (paths: 8 filters); dispatch |
| `overlay-sync.yml` | Overlay sync | push main (paths: 2 filters); dispatch |
| `release.yml` | Release | push  tags (all paths); dispatch |
| `vm-migration.yml` | VM migration + hardware (2 nodes, KVM) | push main (paths: 13 filters); dispatch |
