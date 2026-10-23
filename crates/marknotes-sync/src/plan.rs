//! The sync planner: compares the local tree, the drive and the records saved the last time both agreed, and
//! returns the actions that bring them back in sync. Same decisions the Android and desktop engines made, except
//! that a note changed on both sides becomes a `Reconcile` action.

use std::collections::{HashMap, HashSet};

use crate::manifest::{parse_manifest, Manifest};
use crate::model::{Action, LocalEntry, RemoteEntry, SyncInput, SyncPlan, SyncRecord};
use crate::paths;

pub const CLASH_MESSAGE: &str = "is a file on one side and a folder on the other";

pub fn plan(input: &SyncInput) -> SyncPlan {
    Planner::new(input).run()
}

struct Planner<'a> {
    local_all: Vec<&'a LocalEntry>,
    remote_all: Vec<&'a RemoteEntry>,
    records: HashMap<&'a str, &'a SyncRecord>,
    record_order: Vec<&'a SyncRecord>,
    local_folders: HashSet<&'a str>,
    local_files: HashMap<&'a str, &'a LocalEntry>,
    remote_folders: HashMap<&'a str, &'a RemoteEntry>,
    remote_files: HashMap<&'a str, &'a RemoteEntry>,
    manifest: Option<Manifest>,
    skipped: HashSet<&'a str>,
    pending_local_folder_deletes: Vec<&'a str>,
    pending_remote_folder_deletes: Vec<(&'a str, &'a RemoteEntry)>,
    actions: Vec<Action>,
}

fn sorted(set: HashSet<&str>, cmp: fn(&str, &str) -> std::cmp::Ordering) -> Vec<&str> {
    let mut v: Vec<&str> = set.into_iter().collect();
    v.sort_by(|a, b| cmp(a, b));
    v
}

fn has_pending_ancestor(path: &str, pending: &[&str]) -> bool {
    pending
        .iter()
        .any(|p| *p != path && paths::is_ancestor(p, path))
}

fn remote_changed(entry: &RemoteEntry, record: &SyncRecord) -> bool {
    entry.version != record.remote_version || entry.id != record.remote_id
}

impl<'a> Planner<'a> {
    fn new(input: &'a SyncInput) -> Self {
        let local_all: Vec<&LocalEntry> = input
            .local
            .iter()
            .filter(|e| {
                paths::is_syncable_path(&e.path) && paths::is_synced_entry(&e.path, e.is_folder)
            })
            .collect();
        let remote_all: Vec<&RemoteEntry> = input
            .remote
            .iter()
            .filter(|e| {
                paths::is_syncable_path(&e.path) && paths::is_synced_entry(&e.path, e.is_folder)
            })
            .collect();
        let mut records = HashMap::new();
        for r in &input.records {
            records.insert(r.path.as_str(), r);
        }
        let mut planner = Planner {
            local_folders: local_all
                .iter()
                .filter(|e| e.is_folder)
                .map(|e| e.path.as_str())
                .collect(),
            local_files: local_all
                .iter()
                .filter(|e| !e.is_folder)
                .map(|e| (e.path.as_str(), *e))
                .collect(),
            remote_folders: remote_all
                .iter()
                .filter(|e| e.is_folder)
                .map(|e| (e.path.as_str(), *e))
                .collect(),
            remote_files: remote_all
                .iter()
                .filter(|e| !e.is_folder)
                .map(|e| (e.path.as_str(), *e))
                .collect(),
            local_all,
            remote_all,
            record_order: records.values().copied().collect(),
            records,
            manifest: input.manifest.as_deref().and_then(parse_manifest),
            skipped: HashSet::new(),
            pending_local_folder_deletes: Vec::new(),
            pending_remote_folder_deletes: Vec::new(),
            actions: Vec::new(),
        };
        planner
            .record_order
            .sort_by(|a, b| paths::compare(&a.path, &b.path));
        planner
    }

    fn run(mut self) -> SyncPlan {
        let mut clashes: Vec<&str> = self
            .local_folders
            .iter()
            .filter(|p| self.remote_files.contains_key(*p))
            .chain(
                self.local_files
                    .keys()
                    .filter(|p| self.remote_folders.contains_key(*p)),
            )
            .copied()
            .collect();
        clashes.sort_by(|a, b| paths::compare(a, b));
        for p in clashes {
            self.skipped.insert(p);
            self.actions.push(Action::Error {
                path: p.to_string(),
                message: CLASH_MESSAGE.to_string(),
            });
        }

        // Phase 1: folders, parents first.
        let mut folder_paths: HashSet<&str> = self.local_folders.clone();
        folder_paths.extend(self.remote_folders.keys());
        folder_paths.extend(
            self.record_order
                .iter()
                .filter(|r| r.is_folder)
                .map(|r| r.path.as_str()),
        );
        for path in sorted(folder_paths, paths::compare_by_depth_then_path) {
            if !self.skipped.contains(path) {
                self.sync_folder(path);
            }
        }

        // Phase 2: notes and attachments.
        let mut file_paths: HashSet<&str> = self.local_files.keys().copied().collect();
        file_paths.extend(self.remote_files.keys());
        file_paths.extend(
            self.record_order
                .iter()
                .filter(|r| !r.is_folder)
                .map(|r| r.path.as_str()),
        );
        for path in sorted(file_paths, paths::compare) {
            if !self.skipped.contains(path) {
                self.sync_file(path);
            }
        }

        // Phase 3: folder deletions, top-most folders only.
        self.apply_folder_deletes();
        SyncPlan {
            actions: self.actions,
        }
    }

    fn folder_record(&self, path: &str) -> Option<&'a SyncRecord> {
        self.records.get(path).copied().filter(|r| r.is_folder)
    }

    fn file_record(&self, path: &str) -> Option<&'a SyncRecord> {
        self.records.get(path).copied().filter(|r| !r.is_folder)
    }

    fn local_hash(&self, path: &str) -> Option<&'a str> {
        self.local_files.get(path).and_then(|e| e.sha256.as_deref())
    }

    fn local_changed(&self, path: &str, record: &SyncRecord) -> bool {
        self.local_hash(path) != Some(record.local_hash.as_str())
    }

    fn manifest_hash(&self, remote: &RemoteEntry) -> Option<&str> {
        let e = self.manifest.as_ref()?.files.get(&remote.path)?;
        (e.version == remote.version).then_some(e.sha256.as_str())
    }

    fn sync_folder(&mut self, path: &'a str) {
        let in_local = self.local_folders.contains(path);
        let remote = self.remote_folders.get(path).copied();
        let record = self.folder_record(path);
        let p = path.to_string();
        match (in_local, remote) {
            (true, Some(remote)) => {
                if record.is_none_or(|r| r.remote_id != remote.id) {
                    self.actions.push(Action::RecordFolder {
                        path: p,
                        remote_id: remote.id.clone(),
                    });
                }
            }
            (true, None) => {
                if record.is_none() || self.has_local_survivors(path) {
                    self.actions.push(Action::CreateRemoteFolder { path: p });
                } else {
                    self.pending_local_folder_deletes.push(path);
                }
            }
            (false, Some(remote)) => {
                if record.is_none() || self.has_remote_survivors(path) {
                    self.actions.push(Action::CreateLocalFolder {
                        path: p,
                        remote_id: remote.id.clone(),
                    });
                } else {
                    self.pending_remote_folder_deletes.push((path, remote));
                }
            }
            (false, None) => self.actions.push(Action::Forget { path: p }),
        }
    }

    fn sync_file(&mut self, path: &'a str) {
        let in_local = self.local_files.contains_key(path);
        let remote = self.remote_files.get(path).copied();
        let record = self.file_record(path);
        let attachment = paths::is_attachment_path(path);
        match (in_local, remote) {
            (true, Some(remote)) if attachment => {
                self.attachment_on_both_sides(path, remote, record)
            }
            (true, Some(remote)) => self.note_on_both_sides(path, remote, record),
            (true, None) => self.local_only(path, record),
            (false, Some(remote)) => self.remote_only(path, remote, record),
            (false, None) => self.actions.push(Action::Forget {
                path: path.to_string(),
            }),
        }
    }

    fn record(&mut self, path: &str, remote: &RemoteEntry, sha256: &str, update_base: bool) {
        self.actions.push(Action::Record {
            path: path.to_string(),
            remote: remote.clone(),
            sha256: sha256.to_string(),
            update_base,
        });
    }

    fn note_on_both_sides(
        &mut self,
        path: &str,
        remote: &RemoteEntry,
        record: Option<&SyncRecord>,
    ) {
        let Some(record) = record else {
            self.compare_or_merge(path, remote, false);
            return;
        };
        let local_changed = self.local_changed(path, record);
        let mut remote_changed = remote_changed(remote, record);
        if remote_changed && self.manifest_hash(remote) == Some(record.local_hash.as_str()) {
            // A new drive revision with the content we already have (re-uploaded by another device).
            self.record(path, remote, &record.local_hash, false);
            remote_changed = false;
        }
        if local_changed && remote_changed {
            self.compare_or_merge(path, remote, true);
        } else if local_changed {
            self.actions.push(Action::UploadRevision {
                path: path.to_string(),
                remote: remote.clone(),
            });
        } else if remote_changed {
            self.actions.push(Action::Download {
                path: path.to_string(),
                remote: remote.clone(),
            });
        }
    }

    /// Attachments are named after the hash of their content, so both sides hold the same bytes: they are never
    /// merged, never conflict and never re-uploaded; the record is just refreshed when anything moved.
    fn attachment_on_both_sides(
        &mut self,
        path: &str,
        remote: &RemoteEntry,
        record: Option<&SyncRecord>,
    ) {
        let local_hash = self.local_hash(path).unwrap_or_default();
        if record.is_none_or(|r| remote_changed(remote, r) || local_hash != r.local_hash) {
            self.record(path, remote, local_hash, false);
        }
    }

    fn local_only(&mut self, path: &str, record: Option<&SyncRecord>) {
        let p = path.to_string();
        match record {
            Some(r) if !self.local_changed(path, r) => {
                if has_pending_ancestor(path, &self.pending_local_folder_deletes) {
                    self.actions.push(Action::Forget { path: p });
                } else {
                    self.actions.push(Action::DeleteLocal { path: p });
                }
            }
            _ => self.actions.push(Action::UploadNew { path: p }),
        }
    }

    fn remote_only(&mut self, path: &str, remote: &RemoteEntry, record: Option<&SyncRecord>) {
        let p = path.to_string();
        match record {
            Some(r) if !remote_changed(remote, r) => {
                let pending: Vec<&str> = self
                    .pending_remote_folder_deletes
                    .iter()
                    .map(|(p, _)| *p)
                    .collect();
                if has_pending_ancestor(path, &pending) {
                    self.actions.push(Action::Forget { path: p });
                } else {
                    self.actions.push(Action::DeleteRemote {
                        path: p,
                        remote: remote.clone(),
                    });
                }
            }
            _ => self.actions.push(Action::Download {
                path: p,
                remote: remote.clone(),
            }),
        }
    }

    /// Both sides changed (or were never synced): content the manifest says the drive already has is just
    /// recorded, everything else is reconciled by the executor with the downloaded text.
    fn compare_or_merge(&mut self, path: &str, remote: &RemoteEntry, has_record: bool) {
        let local_hash = self.local_hash(path);
        if local_hash.is_some() && self.manifest_hash(remote) == local_hash {
            let hash = local_hash.unwrap_or_default().to_string();
            self.record(path, remote, &hash, true);
        } else {
            self.actions.push(Action::Reconcile {
                path: path.to_string(),
                remote: remote.clone(),
                has_record,
            });
        }
    }

    fn forget_tree(&self, folder: &str) -> Vec<String> {
        self.record_order
            .iter()
            .filter(|r| r.path == folder || paths::is_ancestor(folder, &r.path))
            .map(|r| r.path.clone())
            .collect()
    }

    fn apply_folder_deletes(&mut self) {
        let local = self.pending_local_folder_deletes.clone();
        for path in &local {
            if !has_pending_ancestor(path, &local) {
                let forget = self.forget_tree(path);
                self.actions.push(Action::DeleteLocalFolder {
                    path: path.to_string(),
                    forget,
                });
            }
        }
        let remote = self.pending_remote_folder_deletes.clone();
        let remote_paths: Vec<&str> = remote.iter().map(|(p, _)| *p).collect();
        for (path, entry) in &remote {
            if !has_pending_ancestor(path, &remote_paths) {
                let forget = self.forget_tree(path);
                self.actions.push(Action::DeleteRemoteFolder {
                    path: path.to_string(),
                    remote: (*entry).clone(),
                    forget,
                });
            }
        }
    }

    /// A local folder deleted on the drive is recreated there when it holds something the drive never had (a new
    /// folder, a new file or a locally edited one).
    fn has_local_survivors(&self, folder: &str) -> bool {
        self.local_all.iter().any(|e| {
            if !paths::is_ancestor(folder, &e.path) {
                return false;
            }
            let record = self.records.get(e.path.as_str());
            if e.is_folder {
                record.is_none()
            } else {
                record.is_none_or(|r| e.sha256.as_deref() != Some(r.local_hash.as_str()))
            }
        })
    }

    fn has_remote_survivors(&self, folder: &str) -> bool {
        self.remote_all.iter().any(|e| {
            if !paths::is_ancestor(folder, &e.path) {
                return false;
            }
            let record = self.records.get(e.path.as_str());
            if e.is_folder {
                record.is_none()
            } else {
                record.is_none_or(|r| remote_changed(e, r))
            }
        })
    }
}
