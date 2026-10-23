//! Data exchanged between the apps and the core. Field names are camelCase in JSON (fixtures, WASM) and in the
//! generated Kotlin.

/// A file or folder in the local note tree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct LocalEntry {
    pub path: String,
    pub is_folder: bool,
    #[serde(default)]
    pub modified_millis: i64,
    /// Lowercase hex SHA-256 of the file bytes. Required for files; `None` for folders.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A file or folder on a drive.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub path: String,
    pub id: String,
    pub is_folder: bool,
    /// Revision of a file, `""` for folders.
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub modified_millis: i64,
}

/// What both sides looked like the last time they agreed on a path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct SyncRecord {
    pub path: String,
    pub is_folder: bool,
    pub remote_id: String,
    #[serde(default)]
    pub remote_version: String,
    #[serde(default)]
    pub local_hash: String,
    #[serde(default)]
    pub synced_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct SyncInput {
    pub local: Vec<LocalEntry>,
    pub remote: Vec<RemoteEntry>,
    pub records: Vec<SyncRecord>,
    /// Text of the drive's `.marknotes/manifest.json`, if any. Unreadable text is treated as absent.
    #[serde(default)]
    pub manifest: Option<String>,
}

/// One step of a sync plan. The executor runs the actions in order; when one fails it records
/// `"<path>: <message>"` as an error and goes on with the next.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// Report an error for `path` (nothing else is done for it).
    Error { path: String, message: String },
    /// Save a folder record.
    RecordFolder { path: String, remote_id: String },
    /// Create the folder on the drive, then save a folder record with the returned id.
    CreateRemoteFolder { path: String },
    /// Create the folder locally, then save a folder record.
    CreateLocalFolder { path: String, remote_id: String },
    /// Delete the record and the base copy of `path`.
    Forget { path: String },
    /// Upload the local file as a new drive file, then record it (and keep a base copy of notes).
    UploadNew { path: String },
    /// Upload the local file as a new revision of `remote`, then record it (and keep a base copy of notes).
    UploadRevision { path: String, remote: RemoteEntry },
    /// Download `remote`, write it locally, then record it (and keep a base copy of notes).
    Download { path: String, remote: RemoteEntry },
    /// Save a file record with `remote`'s id and version and `sha256` as the local hash; when `update_base`, keep
    /// the local bytes as the base copy.
    Record {
        path: String,
        remote: RemoteEntry,
        sha256: String,
        update_base: bool,
    },
    /// Delete the local file and forget it.
    DeleteLocal { path: String },
    /// Delete the drive file and forget it.
    DeleteRemote { path: String, remote: RemoteEntry },
    /// Delete the local folder tree, then delete the records of `forget` and the base copies under `path`.
    DeleteLocalFolder { path: String, forget: Vec<String> },
    /// Delete the drive folder, then delete the records of `forget` and the base copies under `path`.
    DeleteRemoteFolder {
        path: String,
        remote: RemoteEntry,
        forget: Vec<String>,
    },
    /// The note changed on both sides: download `remote` and call `reconcile`. `has_record` tells whether the
    /// base copy should be read.
    Reconcile {
        path: String,
        remote: RemoteEntry,
        has_record: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct SyncPlan {
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct ReconcileInput {
    pub path: String,
    pub local: String,
    pub local_modified_millis: i64,
    pub remote: String,
    pub remote_entry: RemoteEntry,
    #[serde(default)]
    pub base: Option<String>,
    /// Every local path, used to pick a free name for the conflicted copy.
    #[serde(default)]
    pub existing_paths: Vec<String>,
    /// A fresh note id for the conflicted copy.
    pub new_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReconcileOutcome {
    /// Both sides hold the same text: record it.
    Same,
    /// The edits merged cleanly: write `text` locally, upload it as a revision and record it.
    Merged { text: String },
    /// The edits overlap (or there is no base): `winner` is kept at `path`, the other version is saved as a new
    /// note at `copy_path` with `copy_text`.
    NewestWins {
        winner: Side,
        copy_path: String,
        copy_text: String,
    },
}
