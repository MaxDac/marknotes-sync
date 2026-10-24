//! A simulated drive (Dropbox/OneDrive-like): case-insensitive paths, ids and monotonic versions, conditional
//! revision uploads and deletions, create-only uploads of new files, and `.marknotes/manifest.json`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use marknotes_sync::{paths, RemoteEntry};

pub const MANIFEST_PATH: &str = ".marknotes/manifest.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveEntry {
    pub path: String,
    pub id: String,
    /// `v<n>`, from a per-drive counter that never goes back: every commit gets a new version.
    pub version: String,
    pub bytes: Vec<u8>,
    pub modified_millis: i64,
    pub is_folder: bool,
}

impl DriveEntry {
    pub fn remote(&self) -> RemoteEntry {
        RemoteEntry {
            path: self.path.clone(),
            id: self.id.clone(),
            is_folder: self.is_folder,
            version: if self.is_folder {
                String::new()
            } else {
                self.version.clone()
            },
            modified_millis: self.modified_millis,
        }
    }
}

/// How the next listing behaves. `Incomplete` models an adapter that knows it could not list everything (a
/// page failed, an entry could not be decrypted): it returns what it has with `complete: false`. The executor
/// contract says such a listing must fail the sync; only the buggy executor uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListingMode {
    Complete,
    Fails,
    Incomplete,
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub entries: Vec<RemoteEntry>,
    pub complete: bool,
    pub manifest: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Drive {
    pub name: String,
    /// Keyed by the case-folded path: the drive ignores case.
    entries: BTreeMap<String, DriveEntry>,
    counter: u64,
    /// Every file version ever committed: (folded path, version) → bytes. Only the oracle reads it.
    pub history: HashMap<(String, String), Vec<u8>>,
    pub manifest: Option<String>,
    manifest_version: u64,
    /// One-shot listing behaviour for the next `list`.
    pub next_listing: Option<ListingMode>,
    /// Folded paths whose next read returns damaged bytes (transport corruption).
    pub corrupt_next_read: BTreeSet<String>,
    pub supports_empty_delete: bool,
    pub recursive_delete_requests: usize,
}

fn key(path: &str) -> String {
    paths::fold_case(path)
}

impl Drive {
    pub fn new(name: &str) -> Self {
        Drive {
            name: name.to_string(),
            entries: BTreeMap::new(),
            counter: 0,
            history: HashMap::new(),
            manifest: None,
            manifest_version: 0,
            next_listing: None,
            corrupt_next_read: BTreeSet::new(),
            supports_empty_delete: true,
            recursive_delete_requests: 0,
        }
    }

    pub fn entry(&self, path: &str) -> Option<&DriveEntry> {
        self.entries.get(&key(path))
    }

    pub fn entries(&self) -> impl Iterator<Item = &DriveEntry> {
        self.entries.values()
    }

    pub fn bytes_at(&self, path: &str, version: &str) -> Option<&Vec<u8>> {
        self.history.get(&(key(path), version.to_string()))
    }

    fn bump(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    fn parent_exists(&self, path: &str) -> bool {
        let parent = paths::parent(path);
        parent.is_empty() || self.entry(parent).is_some_and(|e| e.is_folder)
    }

    pub fn list(&mut self) -> Result<Listing, String> {
        let mode = self.next_listing.take().unwrap_or(ListingMode::Complete);
        if mode == ListingMode::Fails {
            return Err("listing failed".into());
        }
        Ok(self.listing(mode == ListingMode::Complete))
    }

    /// The listing as it is now, without consuming the one-shot listing mode.
    pub fn listing(&self, complete: bool) -> Listing {
        let mut entries: Vec<RemoteEntry> = self.entries.values().map(|e| e.remote()).collect();
        if !complete {
            // Drop every other file: what a listing that lost pages looks like.
            let mut keep = true;
            entries.retain(|e| {
                if e.is_folder {
                    return true;
                }
                keep = !keep;
                keep
            });
        }
        if self.manifest.is_some() {
            entries.push(RemoteEntry {
                path: ".marknotes".into(),
                id: "marknotes-folder".into(),
                is_folder: true,
                version: String::new(),
                modified_millis: 0,
            });
            entries.push(RemoteEntry {
                path: MANIFEST_PATH.into(),
                id: "manifest".into(),
                is_folder: false,
                version: format!("m{}", self.manifest_version),
                modified_millis: 0,
            });
        }
        Listing {
            entries,
            complete,
            manifest: self.manifest.clone(),
        }
    }

    /// Downloads `path`, which must still be at `version` (the version in the listing).
    pub fn read(&mut self, path: &str, version: &str) -> Result<(Vec<u8>, RemoteEntry), String> {
        let k = key(path);
        let e = self
            .entries
            .get(&k)
            .filter(|e| !e.is_folder)
            .ok_or("not found on the drive")?;
        if e.version != version {
            return Err("changed on the drive since the listing".into());
        }
        let mut bytes = e.bytes.clone();
        let remote = e.remote();
        if self.corrupt_next_read.remove(&k) {
            bytes.push(0xFF);
            if let Some(b) = bytes.first_mut() {
                *b ^= 0x5A;
            }
        }
        Ok((bytes, remote))
    }

    fn commit_file(&mut self, path: &str, id: String, bytes: Vec<u8>, mtime: i64) -> RemoteEntry {
        let version = format!("v{}", self.bump());
        self.history
            .insert((key(path), version.clone()), bytes.clone());
        let e = DriveEntry {
            path: path.to_string(),
            id,
            version,
            bytes,
            modified_millis: mtime,
            is_folder: false,
        };
        let remote = e.remote();
        self.entries.insert(key(path), e);
        remote
    }

    /// Uploads a new file. Create-only unless `overwrite` (Dropbox `add` without autorename, OneDrive
    /// `conflictBehavior=fail`); the parent folder must exist.
    pub fn create_file(
        &mut self,
        path: &str,
        bytes: Vec<u8>,
        mtime: i64,
        overwrite: bool,
    ) -> Result<RemoteEntry, String> {
        let existing = self.entry(path).cloned();
        match &existing {
            Some(e) if e.is_folder => return Err("is a folder on the drive".into()),
            Some(_) if !overwrite => return Err("already exists on the drive".into()),
            _ => {}
        }
        if !self.parent_exists(path) {
            return Err("parent folder is missing on the drive".into());
        }
        let (id, stored) = match existing {
            Some(e) => (e.id, e.path),
            None => {
                let n = self.bump();
                (format!("{}-id{n}", self.name), path.to_string())
            }
        };
        // Keep the spelling of an existing entry, like case-insensitive drives do.
        Ok(self.commit_file(&stored, id, bytes, mtime))
    }

    /// Uploads a new revision; with `if_version` it only applies to that version (and id).
    pub fn update_file(
        &mut self,
        path: &str,
        id: &str,
        if_version: Option<&str>,
        bytes: Vec<u8>,
        mtime: i64,
    ) -> Result<RemoteEntry, String> {
        let e = self
            .entry(path)
            .filter(|e| !e.is_folder)
            .cloned()
            .ok_or("not found on the drive")?;
        if let Some(v) = if_version {
            if e.version != v || e.id != id {
                return Err("precondition failed: the file changed on the drive".into());
            }
        }
        let stored = e.path.clone();
        Ok(self.commit_file(&stored, e.id, bytes, mtime))
    }

    pub fn delete_file(&mut self, path: &str, if_version: Option<&str>) -> Result<(), String> {
        let e = self
            .entry(path)
            .filter(|e| !e.is_folder)
            .ok_or("not found on the drive")?;
        if let Some(v) = if_version {
            if e.version != v {
                return Err("precondition failed: the file changed on the drive".into());
            }
        }
        self.entries.remove(&key(path));
        Ok(())
    }

    /// Creates a folder only if its path is absent.
    pub fn create_folder(&mut self, path: &str) -> Result<RemoteEntry, String> {
        if self.entry(path).is_some() {
            return Err("already exists on the drive".into());
        }
        if !self.parent_exists(path) {
            return Err("parent folder is missing on the drive".into());
        }
        let n = self.bump();
        let e = DriveEntry {
            path: path.to_string(),
            id: format!("{}-folder{n}", self.name),
            version: String::new(),
            bytes: Vec::new(),
            modified_millis: 0,
            is_folder: true,
        };
        let remote = e.remote();
        self.entries.insert(key(path), e);
        Ok(remote)
    }

    /// Deletes a folder with everything inside. Like Dropbox `delete_v2` and OneDrive, folder deletions are
    /// not conditional: the spec's drive adapter contract only makes file deletions conditional.
    pub fn delete_folder(&mut self, path: &str) -> Result<(), String> {
        self.recursive_delete_requests += 1;
        if !self.entry(path).is_some_and(|e| e.is_folder) {
            return Err("folder not found on the drive".into());
        }
        let k = key(path);
        self.entries
            .retain(|p, _| !(p == &k || paths::is_ancestor(&k, p)));
        Ok(())
    }

    /// Atomic nonrecursive deletion; a new descendant cannot be removed by this operation.
    pub fn delete_empty_folder(&mut self, path: &str, id: &str) -> Result<(), String> {
        if !self.supports_empty_delete {
            return Err("atomic empty-folder deletion unsupported".into());
        }
        if !self.entry(path).is_some_and(|e| e.is_folder && e.id == id) {
            return Err("folder changed on the drive".into());
        }
        let k = key(path);
        if self.entries.keys().any(|p| paths::is_ancestor(&k, p)) {
            return Err("folder is not empty on the drive".into());
        }
        self.entries.remove(&k);
        Ok(())
    }

    pub fn rename_case(
        &mut self,
        path: &str,
        to_path: &str,
        listed: &[RemoteEntry],
    ) -> Result<(), String> {
        if key(path) != key(to_path) {
            return Err("not a case-only rename".into());
        }
        let inside = |p: &str| p == path || paths::is_ancestor(path, p);
        let current: Vec<_> = self
            .entries
            .values()
            .filter(|e| inside(&e.path))
            .map(DriveEntry::remote)
            .collect();
        let expected: Vec<_> = listed.iter().filter(|e| inside(&e.path)).cloned().collect();
        if current != expected || current.is_empty() {
            return Err("rename precondition failed".into());
        }
        for old in current {
            let mut e = self.entries.remove(&key(&old.path)).unwrap();
            e.path = format!("{to_path}{}", &old.path[path.len()..]);
            if !e.is_folder {
                self.history
                    .insert((key(&e.path), e.version.clone()), e.bytes.clone());
            }
            self.entries.insert(key(&e.path), e);
        }
        Ok(())
    }

    pub fn write_manifest(&mut self, text: String) {
        self.manifest_version += 1;
        self.manifest = Some(text);
    }
}
