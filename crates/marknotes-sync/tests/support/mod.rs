//! Shared helpers for the integration tests of marknotes-sync. Include it from a test crate with `mod support;`.
//!
//! - [`gen`]: proptest generators (paths, trees, listings, texts, front matter, ids, manifests) and the
//!   deterministic proptest configuration.

pub mod gen;
