//! All of horcrux-api's integration tests in ONE test binary (one link
//! instead of one per file; see AGENTS.md "Code layout"). Each former
//! tests/<name>.rs is the module <name>, so test names are
//! `<name>::<test>`: select a suite with `cargo test -p horcrux-api --test it
//! -- <name>::`. CI runs the suites one after another
//! (ci/run-it-suites.sh), as when they were separate binaries, because many
//! of them share one live server.
//!
//! nightly_realistic_tests.rs stays its own binary (its nightly workflow
//! selects it by name).

mod support;

mod alerts_tests;
mod auth_e2e_tests;
mod auth_rate_limit;
mod auth_realms_e2e;
mod backup_api_e2e;
mod backup_tests;
mod cluster_tests;
mod console_tests;
mod depth_restart_crash;
mod depth_upgrade;
mod depth_vm_journey;
mod integration_tests;
mod k8s_restart_tests;
mod migration_tests;
mod nightly_stress_tests;
mod oidc_integration_tests;
mod persistence_restart_tests;
mod rbac_matrix_tests;
mod sdn_tests;
mod storage_volume_tests;
mod vm_clone_tests;
