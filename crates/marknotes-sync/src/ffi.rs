//! UniFFI exports for the Kotlin bindings (package `io.github.maxdac.marknotes.sync`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::manifest::Manifest;
use crate::merge::{ConflictChoice, MergeChunk, MergeResult};
use crate::model::{ReconcileInput, ReconcileOutcome, SyncInput, SyncPlan, SyncRecord};

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

/// Order in which the drives are synced; see the Rust `Scheduler`.
#[derive(uniffi::Object)]
pub struct SyncScheduler {
    inner: Mutex<crate::scheduler::Scheduler>,
}

#[uniffi::export]
impl SyncScheduler {
    #[uniffi::constructor]
    pub fn new(drive_ids: Vec<String>, max_passes: i32) -> Arc<Self> {
        let max = u32::try_from(max_passes).unwrap_or(0);
        Arc::new(SyncScheduler {
            inner: Mutex::new(crate::scheduler::Scheduler::new(drive_ids, max)),
        })
    }

    pub fn next(&self) -> Option<String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).next()
    }

    pub fn report(&self, drive_id: String, local_changes: i32, failed: bool) {
        let changes = u32::try_from(local_changes).unwrap_or(0);
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report(&drive_id, changes, failed);
    }
}
