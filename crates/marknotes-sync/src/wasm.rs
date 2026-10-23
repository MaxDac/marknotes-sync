//! wasm-bindgen exports for the npm package. Objects cross the boundary as plain JSON-compatible JS values with
//! the same camelCase fields as the fixtures; see `bindings/npm/index.d.ts` for the TypeScript types.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::merge::{ConflictChoice, MergeChunk};
use crate::model::{ReconcileInput, SyncInput, SyncRecord};

fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(|e| JsError::new(&e.to_string()))
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen]
pub fn plan(input: JsValue) -> Result<JsValue, JsError> {
    let input: SyncInput = from_js(input)?;
    to_js(&crate::plan::plan(&input))
}

#[wasm_bindgen]
pub fn reconcile(input: JsValue) -> Result<JsValue, JsError> {
    let input: ReconcileInput = from_js(input)?;
    to_js(&crate::reconcile::reconcile(&input))
}

#[wasm_bindgen]
pub fn merge3(base: Option<String>, local: &str, remote: &str) -> Result<JsValue, JsError> {
    to_js(&crate::merge::merge3(base.as_deref(), local, remote))
}

#[wasm_bindgen(js_name = mergeText)]
pub fn merge_text(chunks: JsValue, choices: JsValue) -> Result<String, JsError> {
    let chunks: Vec<MergeChunk> = from_js(chunks)?;
    let raw: HashMap<String, ConflictChoice> = if choices.is_undefined() || choices.is_null() {
        HashMap::new()
    } else {
        from_js(choices)?
    };
    let mut parsed = HashMap::new();
    for (k, v) in raw {
        let index: i32 = k
            .parse()
            .map_err(|_| JsError::new(&format!("invalid conflict index: {k}")))?;
        parsed.insert(index, v);
    }
    Ok(crate::merge::merge_text(&chunks, &parsed))
}

#[wasm_bindgen(js_name = parseManifest)]
pub fn parse_manifest(text: &str) -> Result<JsValue, JsError> {
    match crate::manifest::parse_manifest(text) {
        Some(m) => to_js(&m),
        None => Ok(JsValue::NULL),
    }
}

#[wasm_bindgen(js_name = serializeManifest)]
pub fn serialize_manifest(manifest: JsValue) -> Result<String, JsError> {
    Ok(crate::manifest::serialize_manifest(&from_js(manifest)?))
}

#[wasm_bindgen(js_name = buildManifest)]
pub fn build_manifest(
    records: JsValue,
    device_id: &str,
    now_millis: f64,
    previous: Option<String>,
) -> Result<Option<String>, JsError> {
    let records: Vec<SyncRecord> = from_js(records)?;
    Ok(crate::manifest::build_manifest(
        &records,
        device_id,
        now_millis as i64,
        previous.as_deref(),
    ))
}

#[wasm_bindgen(js_name = sha256Hex)]
pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::hashing::sha256_hex(bytes)
}

#[wasm_bindgen(js_name = conflictCopyName)]
pub fn conflict_copy_name(path: &str, existing_paths: Vec<String>) -> String {
    crate::paths::conflict_copy_name(path, &existing_paths)
}

#[wasm_bindgen(js_name = isSyncablePath)]
pub fn is_syncable_path(path: &str) -> bool {
    crate::paths::is_syncable_path(path)
}

#[wasm_bindgen(js_name = isSyncedEntry)]
pub fn is_synced_entry(path: &str, is_folder: bool) -> bool {
    crate::paths::is_synced_entry(path, is_folder)
}

#[wasm_bindgen(js_name = isAttachmentPath)]
pub fn is_attachment_path(path: &str) -> bool {
    crate::paths::is_attachment_path(path)
}

#[wasm_bindgen(js_name = readId)]
pub fn read_id(text: &str) -> Option<String> {
    crate::front_matter::read_id(text)
}

#[wasm_bindgen(js_name = withId)]
pub fn with_id(text: &str, id: &str) -> String {
    crate::front_matter::with_id(text, id)
}

/// Order in which the drives are synced.
#[wasm_bindgen]
pub struct SyncScheduler {
    inner: crate::scheduler::Scheduler,
}

#[wasm_bindgen]
impl SyncScheduler {
    #[wasm_bindgen(constructor)]
    pub fn new(drive_ids: Vec<String>, max_passes: Option<u32>) -> SyncScheduler {
        let max = max_passes.unwrap_or(crate::scheduler::DEFAULT_MAX_PASSES);
        SyncScheduler {
            inner: crate::scheduler::Scheduler::new(drive_ids, max),
        }
    }

    pub fn next(&mut self) -> Option<String> {
        self.inner.next()
    }

    pub fn report(&mut self, drive_id: &str, local_changes: u32, failed: bool) {
        self.inner.report(drive_id, local_changes, failed);
    }
}
