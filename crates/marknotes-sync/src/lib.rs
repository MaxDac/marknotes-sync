//! Shared sync core of the MarkNotes apps (Android, desktop, web).
//!
//! The core is sans-IO: it never touches files, drives, clocks or random numbers. Apps list their local tree and
//! the drive, call [`plan`], run the returned [`Action`]s with their own stores, call [`reconcile`] for notes that
//! changed on both sides and [`build_manifest`] at the end. The behaviour is specified in
//! [marknotes-spec](https://github.com/MaxDac/marknotes-spec) and checked against its fixtures.

mod case_folding;
pub mod front_matter;
pub mod hashing;
pub mod manifest;
pub mod merge;
pub mod model;
pub mod paths;
pub mod plan;
pub mod reconcile;
pub mod scheduler;

#[cfg(feature = "uniffi")]
mod ffi;
#[cfg(feature = "wasm")]
mod wasm;

pub use manifest::{
    build_manifest, parse_manifest, serialize_manifest, Manifest, ManifestEntry, MANIFEST_PATH,
};
pub use merge::{merge3, merge_text, ConflictChoice, MergeChunk, MergeResult};
pub use model::*;
pub use plan::plan;
pub use reconcile::reconcile;
pub use scheduler::Scheduler;

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();
