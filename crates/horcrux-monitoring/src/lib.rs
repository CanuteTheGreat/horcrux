//! horcrux-monitoring: split out of horcrux-api (build-memory: no single
//! huge rustc process). Paths stay `crate::<module>` through the
//! re-exports below, so moved code is unchanged.

#![allow(dead_code)]

#[allow(unused_imports)]
pub use horcrux_core::config;
#[allow(unused_imports)]
pub use horcrux_core::db;
#[allow(unused_imports)]
pub use horcrux_core::encryption;
#[allow(unused_imports)]
pub use horcrux_core::error;
#[allow(unused_imports)]
pub use horcrux_core::features;
#[allow(unused_imports)]
pub use horcrux_core::logging;
#[allow(unused_imports)]
pub use horcrux_core::secrets;
#[allow(unused_imports)]
pub use horcrux_core::validation;
pub mod alerts;
pub mod metrics;
pub mod metrics_explorer;
pub mod monitoring;
pub mod notification_channels;
pub mod observability;
pub mod prometheus;
pub mod webhooks;
