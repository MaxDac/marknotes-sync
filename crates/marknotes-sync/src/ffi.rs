//! UniFFI exports for the Kotlin bindings (package `io.github.maxdac.marknotes.sync`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::manifest::Manifest;
use crate::merge::{ConflictChoice, MergeChunk, MergeResult};
use crate::model::{LocalEntry, ReconcileInput, ReconcileOutcome, SyncInput, SyncPlan, SyncRecord};

#[uniffi::export]
pub fn local_deletions(local: Vec<LocalEntry>, records: Vec<SyncRecord>) -> Vec<String> {
    crate::plan::local_deletions(&local, &records)
}

#[uniffi::export]
pub fn plan(input: SyncInput) -> SyncPlan {
    crate::plan::plan(&input)
}

#[uniffi::export]
pub fn reconcile(input: ReconcileInput) -> ReconcileOutcome {
    crate::reconcile::reconcile(&input)
}

#[uniffi::export]
pub fn merge3(base: Option<String>, local: String, remote: String) -> MergeResult {
    crate::merge::merge3(base.as_deref(), &local, &remote)
}

#[uniffi::export]
pub fn merge_text(chunks: Vec<MergeChunk>, choices: HashMap<i32, ConflictChoice>) -> String {
    crate::merge::merge_text(&chunks, &choices)
}

#[uniffi::export]
pub fn parse_manifest(text: String) -> Option<Manifest> {
    crate::manifest::parse_manifest(&text)
}

#[uniffi::export]
pub fn serialize_manifest(manifest: Manifest) -> String {
    crate::manifest::serialize_manifest(&manifest)
}

#[uniffi::export]
pub fn build_manifest(
    records: Vec<SyncRecord>,
    device_id: String,
    now_millis: i64,
    previous: Option<String>,
) -> Option<String> {
    crate::manifest::build_manifest(&records, &device_id, now_millis, previous.as_deref())
}

#[uniffi::export]
pub fn sha256_hex(bytes: Vec<u8>) -> String {
    crate::hashing::sha256_hex(&bytes)
}

#[uniffi::export]
pub fn conflict_copy_name(path: String, existing_paths: Vec<String>) -> String {
    crate::paths::conflict_copy_name(&path, &existing_paths)
}

#[uniffi::export]
pub fn is_syncable_path(path: String) -> bool {
    crate::paths::is_syncable_path(&path)
}

#[uniffi::export]
pub fn is_synced_entry(path: String, is_folder: bool) -> bool {
    crate::paths::is_synced_entry(&path, is_folder)
}

#[uniffi::export]
pub fn is_attachment_path(path: String) -> bool {
    crate::paths::is_attachment_path(&path)
}

#[uniffi::export]
pub fn read_id(text: String) -> Option<String> {
    crate::front_matter::read_id(&text)
}

#[uniffi::export]
pub fn with_id(text: String, id: String) -> String {
    crate::front_matter::with_id(&text, &id)
}

/// An argument the core cannot accept (thrown in Kotlin as `SyncException.InvalidArgument`).
#[derive(Debug, PartialEq, Eq, uniffi::Error)]
#[uniffi(flat_error)]
pub enum SyncError {
    InvalidArgument(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::InvalidArgument(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SyncError {}

fn non_negative(name: &str, value: i32) -> Result<u32, SyncError> {
    u32::try_from(value)
        .map_err(|_| SyncError::InvalidArgument(format!("{name} must not be negative: {value}")))
}

/// Order in which the drives are synced; see the Rust `Scheduler`.
#[derive(uniffi::Object)]
pub struct SyncScheduler {
    inner: Mutex<crate::scheduler::Scheduler>,
}

#[uniffi::export]
impl SyncScheduler {
    /// Throws `SyncException.InvalidArgument` when `max_passes` is negative.
    #[uniffi::constructor]
    pub fn new(drive_ids: Vec<String>, max_passes: i32) -> Result<Arc<Self>, SyncError> {
        let max = non_negative("maxPasses", max_passes)?;
        Ok(Arc::new(SyncScheduler {
            inner: Mutex::new(crate::scheduler::Scheduler::new(drive_ids, max)),
        }))
    }

    pub fn next(&self) -> Option<String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).next()
    }

    /// Throws `SyncException.InvalidArgument` when `local_changes` is negative; nothing is recorded then.
    pub fn report(
        &self,
        drive_id: String,
        local_changes: i32,
        failed: bool,
    ) -> Result<(), SyncError> {
        let changes = non_negative("localChanges", local_changes)?;
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report(&drive_id, changes, failed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_rejects_negative_counts() {
        assert_eq!(
            SyncScheduler::new(vec!["a".into()], -1).err(),
            Some(SyncError::InvalidArgument(
                "maxPasses must not be negative: -1".into()
            ))
        );
        let s = SyncScheduler::new(vec!["a".into(), "b".into()], 3).unwrap();
        assert_eq!(s.next().as_deref(), Some("a"));
        assert!(s.report("a".into(), -1, false).is_err());
        assert_eq!(s.next().as_deref(), Some("b"));
        assert!(s.report("b".into(), 0, false).is_ok());
        assert_eq!(s.next(), None);
    }
}
