//! Extension point for declarative resource kinds.
//!
//! The built-in kinds (storage pools, SDN zones/VNets/subnets, VMs) are
//! planned and applied by `declarative/mod.rs` and `exec.rs`. Every other
//! kind is a `Kind` implementation in its own file under this directory:
//!
//! 1. `pub mod containers;` below and add `&containers::Containers` to
//!    `REGISTRY`. That is the only edit outside the new file.
//! 2. The kind owns one top-level spec section (`key()`), e.g. `containers`.
//!    The engine hands the kind that section as raw JSON (YAML parsed into
//!    `serde_json::Value`) and keeps it in `Spec.extra`, so the kind decides
//!    its own schema with serde.
//! 3. `gather` reads the live state of the kind **in the same shape as the
//!    spec section** (that makes `export` and the "text diff of spec vs live"
//!    in the UI work for free). `plan` is pure: it compares the spec section
//!    with the gathered live section and pushes `Change`s; `apply` carries one
//!    `Change` out against the real manager.
//! 4. Changes use the shared vocabulary (`Action::Create/Update/Delete/
//!    Start/Stop/Blocked`) and should carry per-field `FieldDiff`s (use
//!    `Diffs::field`) so drift reports "expected vs actual" per field.
//!    Whatever a kind cannot change in place is `Blocked` with a reason in
//!    its notes (`Diffs::note`), never silently dropped.
//!
//! Ordering: creates/updates run by ascending `rank()`, deletes by
//! descending rank. Built-in ranks: storage_pool 10, zone 20, vnet 30,
//! subnet 40, vm 50. Pick a number that puts the kind after what it
//! depends on (containers: 60; users/ACLs: 5; firewall: 70).
//!
//! `change_kinds()` lists the `Change.kind` strings the kind emits (usually
//! the singular of the key); `apply` is called only for those.
//!
//! A kind that errors in `gather` makes the plan carry an error for that
//! section (the spec is then not applied), so a failed read can never be
//! mistaken for "nothing exists".
use super::{Change, Live, Spec};
use crate::AppState;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// What a kind's `plan` produces.
#[derive(Debug, Default)]
pub struct KindPlan {
    /// Creates, updates, power changes and blocked differences.
    pub changes: Vec<Change>,
    /// Deletes (only when `prune` is on and the section is managed).
    pub deletes: Vec<Change>,
    /// Reasons the section cannot be applied at all.
    pub errors: Vec<String>,
}

#[async_trait]
pub trait Kind: Send + Sync {
    /// Top-level spec section, e.g. `containers`.
    fn key(&self) -> &'static str;
    /// The `Change.kind` values this kind emits.
    fn change_kinds(&self) -> &'static [&'static str];
    /// Dependency rank (see the module docs).
    fn rank(&self) -> u32;
    /// Checks that need nothing but the section itself.
    fn validate(&self, section: &Value) -> Result<(), String>;
    /// The live state, shaped like the spec section.
    async fn gather(&self, state: &Arc<AppState>) -> Result<Value, String>;
    /// Pure diff of `section` (the spec) against `live` (from `gather`).
    /// `whole` is the complete spec and live view for cross references.
    fn plan(
        &self,
        section: &Value,
        live: &Value,
        prune: bool,
        whole: (&Spec, &Live),
        out: &mut KindPlan,
    );
    /// Carry out one change. `Ok(Some(msg))` is shown in the apply result.
    async fn apply(
        &self,
        state: &Arc<AppState>,
        section: &Value,
        live: &Value,
        change: &Change,
    ) -> Result<Option<String>, String>;
}

/// Every non-built-in kind. Add `&yourkind::YourKind,` here.
static REGISTRY: &[&dyn Kind] = &[];

pub fn registry() -> &'static [&'static dyn Kind] {
    REGISTRY
}

pub fn find(key: &str) -> Option<&'static dyn Kind> {
    registry().iter().copied().find(|k| k.key() == key)
}

/// The kind that emits changes of this `Change.kind`.
pub fn owner_of(change_kind: &str) -> Option<&'static dyn Kind> {
    registry()
        .iter()
        .copied()
        .find(|k| k.change_kinds().contains(&change_kind))
}

/// Keys of every registered kind (for messages).
pub fn keys() -> Vec<&'static str> {
    registry().iter().map(|k| k.key()).collect()
}
