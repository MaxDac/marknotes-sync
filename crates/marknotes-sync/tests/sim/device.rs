//! A simulated device: a durable part (what survives a crash or restart: the files, the sync records and base
//! copies per drive, the pending conflicts) and a volatile part (the sync in flight), discarded by a crash.

use std::collections::BTreeMap;

use marknotes_sync::{
    hashing::sha256_hex, paths, Action, ConflictCopyIntent, LocalEntry, SyncRecord,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Folder,
    File { bytes: Vec<u8>, mtime: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingConflict {
    pub drive: String,
    pub note_path: String,
    pub copy_path: String,
    pub base: Option<String>,
    pub intent: ConflictCopyIntent,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Durable {
    /// The local file system: case-sensitive, like Android's.
    pub files: BTreeMap<String, Node>,
    /// Drive name → path → record.
    pub records: BTreeMap<String, BTreeMap<String, SyncRecord>>,
    /// Drive name → path → base copy (notes only).
    pub bases: BTreeMap<String, BTreeMap<String, String>>,
    pub pending_conflicts: Vec<PendingConflict>,
    pub copy_intents: BTreeMap<(String, String), ConflictCopyIntent>,
}

/// The sync in flight: the plan and the listing it was computed from. Lost on a crash.
#[derive(Clone, Debug)]
pub struct InFlight {
    pub drive: usize,
    pub plan: Vec<Action>,
    pub next_action: usize,
    pub listed: BTreeMap<String, LocalEntry>,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub name: String,
    /// Indexes of the linked drives, in sync order.
    pub drives: Vec<usize>,
    /// Added to the world time: this device's clock.
    pub skew: i64,
    pub durable: Durable,
    pub volatile: Option<InFlight>,
}

impl Device {
    pub fn new(name: &str, drives: Vec<usize>) -> Self {
        Device {
            name: name.to_string(),
            drives,
            skew: 0,
            durable: Durable::default(),
            volatile: None,
        }
    }

    pub fn node(&self, path: &str) -> Option<&Node> {
        self.durable.files.get(path)
    }

    pub fn file(&self, path: &str) -> Option<(&[u8], i64)> {
        match self.node(path) {
            Some(Node::File { bytes, mtime }) => Some((bytes.as_slice(), *mtime)),
            _ => None,
        }
    }

    pub fn text(&self, path: &str) -> Option<String> {
        self.file(path)
            .map(|(b, _)| String::from_utf8_lossy(b).into_owned())
    }

    pub fn hash(&self, path: &str) -> Option<String> {
        self.file(path).map(|(b, _)| sha256_hex(b))
    }

    pub fn mkdir_p(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            self.durable
                .files
                .entry(prefix.clone())
                .or_insert(Node::Folder);
        }
    }

    /// Writes a file, creating its parent folders.
    pub fn write(&mut self, path: &str, bytes: Vec<u8>, mtime: i64) {
        self.mkdir_p(paths::parent(path));
        self.durable
            .files
            .insert(path.to_string(), Node::File { bytes, mtime });
    }

    /// Removes `path` and everything inside it; returns what was removed.
    pub fn remove_tree(&mut self, path: &str) -> Vec<(String, Node)> {
        let keys: Vec<String> = self
            .durable
            .files
            .keys()
            .filter(|k| *k == path || paths::is_ancestor(path, k))
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|k| self.durable.files.remove(&k).map(|n| (k, n)))
            .collect()
    }

    /// Lists the whole tree, ignored entries included, hashing every file.
    pub fn list(&self) -> Vec<LocalEntry> {
        self.durable
            .files
            .iter()
            .map(|(path, node)| match node {
                Node::Folder => LocalEntry {
                    path: path.clone(),
                    is_folder: true,
                    modified_millis: 0,
                    sha256: None,
                },
                Node::File { bytes, mtime } => LocalEntry {
                    path: path.clone(),
                    is_folder: false,
                    modified_millis: *mtime,
                    sha256: Some(sha256_hex(bytes)),
                },
            })
            .collect()
    }

    pub fn records(&self, drive: &str) -> Vec<SyncRecord> {
        self.durable
            .records
            .get(drive)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn record(&self, drive: &str, path: &str) -> Option<&SyncRecord> {
        self.durable.records.get(drive).and_then(|m| m.get(path))
    }

    pub fn base(&self, drive: &str, path: &str) -> Option<&String> {
        self.durable.bases.get(drive).and_then(|m| m.get(path))
    }
}
