//! Helm chart management (repositories, chart search, releases).
//!
//! Everything here drives the real `helm` binary (Gentoo: `app-admin/helm`,
//! pulled in by `USE=kubernetes`). Two things make that work for a
//! long-running service:
//!
//! * Helm's own state (repositories.yaml, the repo index cache, plugins)
//!   lives under Horcrux's data directory (`<data_dir>/helm/...`, see
//!   [`set_helm_home`]) instead of `$HOME`, which the OpenRC/systemd service
//!   user may not even have. Repositories therefore survive restarts and are
//!   shared by every cluster, like `helm repo` on a workstation.
//! * Release commands get the cluster's stored kubeconfig written to a
//!   private (0600) temporary file for the duration of the call (see
//!   [`cli::KubeTarget`]); nothing is left in /tmp afterwards.

pub mod cli;
pub mod releases;
pub mod repos;

pub use cli::{set_helm_home, KubeTarget};
pub use releases::{HelmInstallRequest, HelmRelease, HelmValues};
pub use repos::{AddRepoRequest, HelmChart, HelmRepository};
