//! horcrux-nas: split out of horcrux-api (build-memory: no single
//! huge rustc process). Paths stay `crate::<module>` through the
//! re-exports below, so moved code is unchanged.

#![allow(dead_code)]


#[cfg(feature = "nas")]
pub mod nas;
