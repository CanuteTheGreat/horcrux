//! horcrux-storage: split out of horcrux-api (build-memory: no single
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
pub mod cloudinit;
pub mod storage;
pub mod template;
