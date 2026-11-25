//! Horcrux storage plugin SDK.
//!
//! A Horcrux storage plugin is a separately packaged executable that the
//! manager runs once per operation. It reads ONE JSON [`Request`] from stdin
//! and writes ONE JSON [`Response`] to stdout, then exits. Nothing runs inside
//! the API process: a plugin that crashes, hangs or leaks only affects its
//! own call (the manager kills it at the operation's timeout).
//!
//! The protocol is plain JSON, so a plugin can be written in any language;
//! this crate is the Rust convenience layer:
//!
//! - [`protocol`]: the request/response types of protocol version
//!   [`API_VERSION`] (see docs/storage-plugin-api-changelog.md).
//! - [`schema`]: the config schema a plugin declares in `describe`, and the
//!   validation the manager applies to a pool's config before the plugin ever
//!   sees it (the web UI renders its pool form from the same schema).
//! - [`serve`]: run a type implementing [`StoragePlugin`] as a plugin
//!   executable.
//!
//! The guide is docs/storage-plugins.md in the Horcrux repository.

pub mod protocol;
pub mod schema;
mod server;

pub use protocol::*;
pub use schema::{
    check_schema, validate_config, Capabilities, DependsOn, EnumOption, Field, FieldError,
    FieldType, ValidatedConfig,
};
pub use server::{handle_request, serve, StoragePlugin};

/// The protocol version this SDK speaks. A plugin reports the version it
/// implements in `describe`; the manager refuses plugins whose version it
/// does not support (`supported_api_versions` in its storage plugin status).
pub const API_VERSION: u32 = 1;
