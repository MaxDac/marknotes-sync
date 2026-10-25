//! wasm-bindgen exports for the npm package. Objects cross the boundary as plain JSON-compatible JS values with
//! the same camelCase fields as the fixtures; see `bindings/npm/index.d.ts` for the TypeScript types.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::merge::{ConflictChoice, MergeChunk};
use crate::model::{LocalEntry, ReconcileInput, SyncInput, SyncRecord};

#[wasm_bindgen(js_name = localDeletions)]
pub fn local_deletions(local: JsValue, records: JsValue) -> Result<JsValue, JsError> {
    let local: Vec<LocalEntry> = from_js(local)?;
    let records: Vec<SyncRecord> = from_js(records)?;
    to_js(&crate::plan::local_deletions(&local, &records))
}

#[wasm_bindgen(js_name = foldCase)]
pub fn fold_case(path: &str) -> String {
    crate::paths::fold_case(path)
}

#[wasm_bindgen(js_name = recreatedFolders)]
pub fn recreated_folders(local: JsValue, records: JsValue) -> Result<JsValue, JsError> {
    let local: Vec<LocalEntry> = from_js(local)?;
    let records: Vec<SyncRecord> = from_js(records)?;
    to_js(&crate::plan::recreated_folders(&local, &records))
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = JSON, js_name = parse, catch)]
    fn json_parse(text: &str) -> Result<JsValue, JsValue>;
}

/// Results are built by `JSON.parse`, which defines every key as an own property. Building objects by assignment
/// (as serde-wasm-bindgen does) would turn a manifest path named `__proto__` into the object's prototype and lose it.
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    let json = serde_json::to_value(value).map_err(|e| JsError::new(&e.to_string()))?;
    numbers::check_safe(&json).map_err(|e| JsError::new(&e))?;
    json_parse(&json.to_string()).map_err(|_| JsError::new("could not build the result"))
}

/// Checks on JS numbers passed as plain arguments, which wasm-bindgen would otherwise wrap or truncate silently.
mod numbers {
    /// Largest integer a JS number holds exactly (`Number.MAX_SAFE_INTEGER`).
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

    /// A non-negative integer count up to `u32::MAX`.
    pub fn count(name: &str, value: f64) -> Result<u32, String> {
        if value.is_finite() && value.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(&value) {
            Ok(value as u32)
        } else {
            Err(format!(
                "{name} must be a non-negative integer up to {}: {value}",
                u32::MAX
            ))
        }
    }

    /// An integer number of milliseconds a JS number holds exactly.
    pub fn millis(name: &str, value: f64) -> Result<i64, String> {
        if value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER {
            Ok(value as i64)
        } else {
            Err(format!("{name} must be a safe integer: {value}"))
        }
    }

    /// Fails when a result holds an integer a JS number can't hold exactly (a manifest can carry any `i64` time).
    pub fn check_safe(value: &serde_json::Value) -> Result<(), String> {
        match value {
            serde_json::Value::Number(n) => {
                let safe = n.as_i64().map_or_else(
                    || n.as_u64().is_none() && n.as_f64().is_some_and(f64::is_finite),
                    |i| i.unsigned_abs() <= MAX_SAFE_INTEGER as u64,
                );
                if safe {
                    Ok(())
                } else {
                    Err(format!(
                        "{n} can't be represented as a JavaScript number (not a safe integer)"
                    ))
                }
            }
            serde_json::Value::Array(xs) => xs.iter().try_for_each(check_safe),
            serde_json::Value::Object(map) => map.values().try_for_each(check_safe),
            _ => Ok(()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn counts() {
            assert_eq!(count("n", 0.0), Ok(0));
            assert_eq!(count("n", 4_294_967_295.0), Ok(u32::MAX));
            for bad in [-1.0, -0.5, 1.5, 4_294_967_296.0, f64::NAN, f64::INFINITY] {
                assert!(count("n", bad).is_err(), "{bad}");
            }
            assert_eq!(count("n", -0.0), Ok(0));
        }

        #[test]
        fn millis_are_safe_integers() {
            assert_eq!(millis("t", 1_700_000_000_000.0), Ok(1_700_000_000_000));
            assert_eq!(millis("t", -MAX_SAFE_INTEGER), Ok(-9_007_199_254_740_991));
            for bad in [1.5, MAX_SAFE_INTEGER + 1.0, f64::NAN, f64::NEG_INFINITY] {
                assert!(millis("t", bad).is_err(), "{bad}");
            }
        }

        #[test]
        fn results_hold_only_safe_integers() {
            let ok = serde_json::json!({ "a": [9_007_199_254_740_991_i64, -9_007_199_254_740_991_i64, 1.5, "x", null] });
            assert_eq!(check_safe(&ok), Ok(()));
            for bad in [
                serde_json::json!(9_007_199_254_740_992_i64),
                serde_json::json!(-9_007_199_254_740_992_i64),
                serde_json::json!(i64::MIN),
                serde_json::json!(u64::MAX),
                serde_json::json!({ "files": { "a.md": { "syncedAt": i64::MAX } } }),
            ] {
                assert!(check_safe(&bad).is_err(), "{bad}");
            }
        }
    }
}

fn js_error(message: String) -> JsError {
    JsError::new(&message)
}

/// Nothing to set up. It exists because the reset function generated by wasm-bindgen's
/// `--experimental-reset-state-function` (used by `bindings/npm/recover.js`) calls the start function.
#[wasm_bindgen(start)]
pub fn start() {}

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
    let now_millis = numbers::millis("nowMillis", now_millis).map_err(js_error)?;
    Ok(crate::manifest::build_manifest(
        &records,
        device_id,
        now_millis,
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
    /// Throws when `max_passes` is not a non-negative integer.
    #[wasm_bindgen(constructor)]
    pub fn new(drive_ids: Vec<String>, max_passes: Option<f64>) -> Result<SyncScheduler, JsError> {
        let max = match max_passes {
            Some(value) => numbers::count("maxPasses", value).map_err(js_error)?,
            None => crate::scheduler::DEFAULT_MAX_PASSES,
        };
        Ok(SyncScheduler {
            inner: crate::scheduler::Scheduler::new(drive_ids, max),
        })
    }

    pub fn next(&mut self) -> Option<String> {
        self.inner.next()
    }

    /// Throws when `local_changes` is not a non-negative integer; nothing is recorded then.
    pub fn report(
        &mut self,
        drive_id: &str,
        local_changes: f64,
        failed: bool,
    ) -> Result<(), JsError> {
        let changes = numbers::count("localChanges", local_changes).map_err(js_error)?;
        self.inner.report(drive_id, changes, failed);
        Ok(())
    }
}
