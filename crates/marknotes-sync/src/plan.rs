//! The sync planner: compares the local tree, the drive and the records saved the last time both agreed, and
//! returns the actions that bring them back in sync. Same decisions the Android and desktop engines made, except
//! that a note changed on both sides becomes a `Reconcile` action.

use std::collections::{HashMap, HashSet};

use crate::manifest::{parse_manifest, Manifest};
use crate::model::{Action, LocalEntry, RemoteEntry, SyncInput, SyncPlan, SyncRecord};
use crate::paths;

pub const CLASH_MESSAGE: &str = "is a file on one side and a folder on the other";
pub const CASE_MESSAGE: &str = "differs only in case from another path";
pub const DUPLICATE_MESSAGE: &str = "is listed more than once";

pub fn plan(input: &SyncInput) -> SyncPlan {
    Planner::new(input).run()
}

struct Planner<'a> {
    local_all: Vec<&'a LocalEntry>,
    remote_all: Vec<&'a RemoteEntry>,
    /// Entries that are not synced (other files, hidden entries): they only keep their folder from being deleted.
    local_ignored: Vec<&'a str>,
    remote_ignored: Vec<&'a str>,
    records: HashMap<&'a str, &'a SyncRecord>,
    record_order: Vec<&'a SyncRecord>,
    local_folders: HashSet<&'a str>,
    local_files: HashMap<&'a str, &'a LocalEntry>,
    remote_folders: HashMap<&'a str, &'a RemoteEntry>,
    remote_files: HashMap<&'a str, &'a RemoteEntry>,
    manifest: Option<Manifest>,
    /// Paths listed more than once on the same side, or recorded more than once.
    duplicates: HashSet<&'a str>,
    /// Folders a listing does not include although it includes entries inside them.
    implied_folders: HashSet<&'a str>,
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

fn is_synced(path: &str, is_folder: bool) -> bool {
    paths::is_syncable_path(path) && paths::is_synced_entry(path, is_folder)
}

/// `a/b/c` gives `a`, `a/b` and `a/b/c`.
fn prefixes(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .map(move |(i, _)| &path[..i])
        .chain(std::iter::once(path))
}

/// Paths that occur more than once in `paths`.
fn repeated<'a>(paths: impl Iterator<Item = &'a str>) -> HashSet<&'a str> {
    let mut seen = HashSet::new();
    paths.filter(|p| !seen.insert(*p)).collect()
}

/// Folders above the `entries` of one listing that the listing itself does not include.
fn implied<'a>(entries: impl Iterator<Item = &'a str>, folders: &HashSet<&str>) -> Vec<&'a str> {
    entries
        .flat_map(|p| prefixes(p).filter(move |prefix| prefix.len() < p.len()))
        .filter(|f| !folders.contains(f))
        .collect()
}

fn remote_changed(entry: &RemoteEntry, record: &SyncRecord) -> bool {
    entry.version != record.remote_version || entry.id != record.remote_id
}

impl<'a> Planner<'a> {
    fn new(input: &'a SyncInput) -> Self {
        let (local_all, local_ignored): (Vec<&LocalEntry>, Vec<&LocalEntry>) = input
            .local
            .iter()
            .partition(|e| is_synced(&e.path, e.is_folder));
        let (remote_all, remote_ignored): (Vec<&RemoteEntry>, Vec<&RemoteEntry>) = input
            .remote
            .iter()
            .partition(|e| is_synced(&e.path, e.is_folder));
        let mut records = HashMap::new();
        for r in &input.records {
            records.insert(r.path.as_str(), r);
        }
        let mut duplicates = repeated(local_all.iter().map(|e| e.path.as_str()));
        duplicates.extend(repeated(remote_all.iter().map(|e| e.path.as_str())));
        duplicates.extend(repeated(input.records.iter().map(|r| r.path.as_str())));
        let listed_local_folders: HashSet<&str> = input
            .local
            .iter()
            .filter(|e| e.is_folder)
            .map(|e| e.path.as_str())
            .collect();
        let listed_remote_folders: HashSet<&str> = input
            .remote
            .iter()
            .filter(|e| e.is_folder)
            .map(|e| e.path.as_str())
            .collect();
        let mut implied_folders: HashSet<&str> = implied(
            input.local.iter().map(|e| e.path.as_str()),
            &listed_local_folders,
        )
        .into_iter()
        .collect();
        implied_folders.extend(implied(
            input.remote.iter().map(|e| e.path.as_str()),
            &listed_remote_folders,
        ));
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
            local_ignored: local_ignored.iter().map(|e| e.path.as_str()).collect(),
            remote_ignored: remote_ignored.iter().map(|e| e.path.as_str()).collect(),
            record_order: records.values().copied().collect(),
            records,
            manifest: input.manifest.as_deref().and_then(parse_manifest),
            duplicates,
            implied_folders,
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

    /// Paths that are a file on one side and a folder on the other, and paths that differ only in case from
    /// another one across the local tree, the drive and the records (with everything inside them): one error per
    /// path, the file/folder message winning.
    fn errors(&self) -> HashMap<&'a str, &'static str> {
        let mut errors = HashMap::new();
        let clashes = self
            .local_folders
            .iter()
            .filter(|p| self.remote_files.contains_key(*p))
            .chain(
                self.local_files
                    .keys()
                    .filter(|p| self.remote_folders.contains_key(*p)),
            );
        for p in clashes {
            errors.insert(*p, CLASH_MESSAGE);
        }

        let all: HashSet<&'a str> = self
            .local_all
            .iter()
            .map(|e| e.path.as_str())
            .chain(self.remote_all.iter().map(|e| e.path.as_str()))
            .chain(self.record_order.iter().map(|r| r.path.as_str()))
            .collect();
        for path in &all {
            if prefixes(path).any(|p| self.duplicates.contains(p)) {
                errors.entry(*path).or_insert(DUPLICATE_MESSAGE);
            }
        }
        let mut spellings: HashMap<String, HashSet<&str>> = HashMap::new();
        for path in &all {
            for prefix in prefixes(path) {
                spellings
                    .entry(paths::fold_case(prefix))
                    .or_default()
                    .insert(prefix);
            }
        }
        let colliding: HashSet<&str> = spellings
            .into_values()
            .filter(|s| s.len() > 1)
            .flatten()
            .collect();
        for path in all {
            if prefixes(path).any(|p| colliding.contains(p)) {
                errors.entry(path).or_insert(CASE_MESSAGE);
            }
        }
        errors
    }

    fn run(mut self) -> SyncPlan {
        let errors = self.errors();
        let mut error_paths: Vec<&str> = errors.keys().copied().collect();
        error_paths.sort_by(|a, b| paths::compare(a, b));
        for p in error_paths {
            self.skipped.insert(p);
            self.actions.push(Action::Error {
                path: p.to_string(),
                message: errors[p].to_string(),
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
        if self.implied_folders.contains(path) {
            // A listing that holds entries inside the folder but not the folder itself: it is certainly there, so
            // it is never deleted, and without its listing entry it can't be created or recorded either.
            return;
        }
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
                // Inside a folder deletion: step 3 deletes it and drops its record.
                if !has_pending_ancestor(path, &self.pending_local_folder_deletes) {
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
                if !has_pending_ancestor(path, &pending) {
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
            .filter(|r| !self.skipped.contains(r.path.as_str()))
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

    /// A folder that holds an entry that is not synced, or one skipped with an error, is kept: its synced contents
    /// are handled one by one.
    fn has_kept_entries<'p>(
        &self,
        folder: &str,
        ignored: &[&str],
        mut synced: impl Iterator<Item = &'p str>,
    ) -> bool {
        ignored.iter().any(|p| paths::is_ancestor(folder, p))
            || synced.any(|p| paths::is_ancestor(folder, p) && self.skipped.contains(p))
    }

    /// A local folder deleted on the drive is recreated there when it holds something the drive never had (a new
    /// folder, a new file or a locally edited one) or anything that is not synced.
    fn has_local_survivors(&self, folder: &str) -> bool {
        let synced = self.local_all.iter().map(|e| e.path.as_str());
        if self.has_kept_entries(folder, &self.local_ignored, synced) {
            return true;
        }
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
        let synced = self.remote_all.iter().map(|e| e.path.as_str());
        if self.has_kept_entries(folder, &self.remote_ignored, synced) {
            return true;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_collisions_inside_a_deleted_folder_keep_it() {
        // Deleted on the drive, but A/x.md and A/X.md collide: nothing inside A may be deleted or forgotten.
        let input: SyncInput = serde_json::from_value(serde_json::json!({
            "local": [
                { "path": "A", "isFolder": true, "modifiedMillis": 1 },
                { "path": "A/x.md", "isFolder": false, "modifiedMillis": 1, "sha256": "h1" }
            ],
            "remote": [],
            "records": [
                { "path": "A", "isFolder": true, "remoteId": "fA", "remoteVersion": "", "localHash": "", "syncedAt": 1 },
                { "path": "A/x.md", "isFolder": false, "remoteId": "r1", "remoteVersion": "v1", "localHash": "h1", "syncedAt": 1 },
                { "path": "A/X.md", "isFolder": false, "remoteId": "r2", "remoteVersion": "v1", "localHash": "h2", "syncedAt": 1 }
            ],
            "manifest": null
        }))
        .unwrap();
        let error = |path: &str| Action::Error {
            path: path.into(),
            message: CASE_MESSAGE.into(),
        };
        assert_eq!(
            plan(&input).actions,
            vec![
                error("A/X.md"),
                error("A/x.md"),
                Action::CreateRemoteFolder { path: "A".into() },
            ]
        );
    }
}
