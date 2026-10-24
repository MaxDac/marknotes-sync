//! `.marknotes/manifest.json`: the map of synced files each app writes to a drive after a sync.

use std::collections::HashMap;

use serde_json::Value;

use crate::model::SyncRecord;
use crate::paths;

pub const MANIFEST_PATH: &str = ".marknotes/manifest.json";
pub const MANIFEST_FORMAT: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub sha256: String,
    pub version: String,
    pub synced_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub device_id: String,
    pub updated_at: i64,
    pub files: HashMap<String, ManifestEntry>,
}

fn as_millis(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
}

/// Parses a manifest; `None` when it is not JSON, has a newer `format`, or lacks `deviceId`, `updatedAt` or an
/// entry field. Unknown keys are ignored and a missing `files` key means no files (`files: null` is unreadable).
pub fn parse_manifest(text: &str) -> Option<Manifest> {
    let obj = serde_json::from_str::<Value>(text).ok()?;
    let obj = obj.as_object()?;
    let format = obj.get("format")?.as_f64()?;
    if format > MANIFEST_FORMAT as f64 {
        return None;
    }
    let device_id = obj.get("deviceId")?.as_str()?.to_string();
    let updated_at = as_millis(obj.get("updatedAt")?)?;
    let mut files = HashMap::new();
    match obj.get("files") {
        None => {}
        Some(Value::Object(raw)) => {
            for (path, e) in raw {
                let e = e.as_object()?;
                files.insert(
                    path.clone(),
                    ManifestEntry {
                        sha256: e.get("sha256")?.as_str()?.to_string(),
                        version: e.get("version")?.as_str()?.to_string(),
                        synced_at: as_millis(e.get("syncedAt")?)?,
                    },
                );
            }
        }
        Some(_) => return None,
    }
    Some(Manifest {
        device_id,
        updated_at,
        files,
    })
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("strings always serialize")
}

/// Compact JSON with keys `format, app, deviceId, updatedAt, files`, paths sorted, entry keys
/// `sha256, version, syncedAt`.
pub fn serialize_manifest(manifest: &Manifest) -> String {
    let mut paths_sorted: Vec<&String> = manifest.files.keys().collect();
    paths_sorted.sort_by(|a, b| paths::compare(a, b));
    let files: Vec<String> = paths_sorted
        .into_iter()
        .map(|p| {
            let e = &manifest.files[p];
            format!(
                "{}:{{\"sha256\":{},\"version\":{},\"syncedAt\":{}}}",
                json_string(p),
                json_string(&e.sha256),
                json_string(&e.version),
                e.synced_at
            )
        })
        .collect();
    format!(
        "{{\"format\":{MANIFEST_FORMAT},\"app\":\"MarkNotes\",\"deviceId\":{},\"updatedAt\":{},\"files\":{{{}}}}}",
        json_string(&manifest.device_id),
        manifest.updated_at,
        files.join(",")
    )
}

/// True when both maps describe the same files (same paths, `sha256` and `version`), ignoring `syncedAt`.
fn same_files(a: &HashMap<String, ManifestEntry>, b: &HashMap<String, ManifestEntry>) -> bool {
    a.len() == b.len()
        && a.iter().all(|(p, e)| {
            b.get(p)
                .is_some_and(|o| o.sha256 == e.sha256 && o.version == e.version)
        })
}

/// The manifest to write after a sync, built from the file records; `None` when its files are the same as in
/// `previous` (the manifest read before the sync), so nothing needs to be written.
pub fn build_manifest(
    records: &[SyncRecord],
    device_id: &str,
    now_millis: i64,
    previous: Option<&str>,
) -> Option<String> {
    let files: HashMap<String, ManifestEntry> = records
        .iter()
        .filter(|r| !r.is_folder)
        .map(|r| {
            (
                r.path.clone(),
                ManifestEntry {
                    sha256: r.local_hash.clone(),
                    version: r.remote_version.clone(),
                    synced_at: r.synced_at,
                },
            )
        })
        .collect();
    if let Some(prev) = previous.and_then(parse_manifest) {
        if same_files(&files, &prev.files) {
            return None;
        }
    }
    Some(serialize_manifest(&Manifest {
        device_id: device_id.to_string(),
        updated_at: now_millis,
        files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let text = r#"{"format":1,"app":"MarkNotes","deviceId":"d","updatedAt":1,"files":{"Work/a.md":{"sha256":"aa","version":"1","syncedAt":10},"b.md":{"sha256":"bb","version":"2","syncedAt":20}}}"#;
        let m = parse_manifest(text).unwrap();
        assert_eq!(serialize_manifest(&m), text);
    }

    #[test]
    fn rejects() {
        assert!(parse_manifest(r#"{"format":2,"deviceId":"d","updatedAt":1}"#).is_none());
        assert!(parse_manifest(r#"{"format":1,"updatedAt":1}"#).is_none());
        assert!(parse_manifest(r#"{"format":1,"deviceId":"d","updatedAt":1,"files":{"a.md":{"sha256":"x","version":1,"syncedAt":2}}}"#).is_none());
        assert_eq!(
            parse_manifest(r#"{"format":1,"deviceId":"d","updatedAt":1}"#)
                .unwrap()
                .files
                .len(),
            0
        );
    }

    #[test]
    fn build_skips_unchanged() {
        let rec = SyncRecord {
            path: "a.md".into(),
            is_folder: false,
            remote_id: "r".into(),
            remote_version: "1".into(),
            local_hash: "h".into(),
            synced_at: 5,

            local_deleted: false,
        };
        let folder = SyncRecord {
            path: "F".into(),
            is_folder: true,
            remote_version: String::new(),
            local_hash: String::new(),
            ..rec.clone()
        };
        let text = build_manifest(&[rec.clone(), folder], "d", 7, None).unwrap();
        assert_eq!(
            text,
            r#"{"format":1,"app":"MarkNotes","deviceId":"d","updatedAt":7,"files":{"a.md":{"sha256":"h","version":"1","syncedAt":5}}}"#
        );
        let later = SyncRecord {
            synced_at: 99,
            ..rec.clone()
        };
        assert_eq!(build_manifest(&[later], "other", 8, Some(&text)), None);
        let changed = SyncRecord {
            remote_version: "2".into(),
            ..rec
        };
        assert!(build_manifest(&[changed], "d", 9, Some(&text)).is_some());
    }
}
