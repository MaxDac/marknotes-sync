//! The sync planner: compares the local tree, the drive and the records saved the last time both agreed, and
//! returns the actions that bring them back in sync. Same decisions the Android and desktop engines made, except
//! that a note changed on both sides becomes a `Reconcile` action.

use std::collections::{HashMap, HashSet};

use crate::manifest::{parse_manifest, Manifest};
use crate::model::{
    Action, LocalEntry, RemoteEntry, RemoteFolderDeletion, SyncInput, SyncPlan, SyncRecord,
};
use crate::paths;

pub const CLASH_MESSAGE: &str = "is a file on one side and a folder on the other";
pub const CASE_MESSAGE: &str = "differs only in case from another path";
pub const DUPLICATE_MESSAGE: &str = "is listed more than once";

pub fn plan(input: &SyncInput) -> SyncPlan {
    Planner::new(input).run()
}

/// File records to flag durably on every linked drive before planning from a complete local listing.
pub fn local_deletions(local: &[LocalEntry], records: &[SyncRecord]) -> Vec<String> {
    let files: HashSet<&str> = local
        .iter()
        .filter(|e| !e.is_folder)
        .map(|e| e.path.as_str())
        .collect();
    let mut paths: Vec<_> = records
        .iter()
        .filter(|r| !r.is_folder && !r.local_deleted && !files.contains(r.path.as_str()))
        .map(|r| r.path.clone())
        .collect();
    paths.sort_by(|a, b| paths::compare(a, b));
    paths.dedup();
    paths
}

/// Preserved folder records to retire atomically with their active notices on every linked drive
/// before planning. Keep descendant records, bases and ordinary folder history.
pub fn recreated_folders(local: &[LocalEntry], records: &[SyncRecord]) -> Vec<String> {
    let present: HashSet<&str> = local.iter().map(|e| e.path.as_str()).collect();
    let mut recreated: Vec<_> = records
        .iter()
        .filter(|r| r.is_folder && r.remote_folder_preserved && present.contains(r.path.as_str()))
        .map(|r| r.path.clone())
        .collect();
    recreated.sort_by(|a, b| paths::compare(a, b));
    recreated.dedup();
    recreated
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
    remote_folder_deletion: RemoteFolderDeletion,
    /// Paths listed more than once on the same side, or recorded more than once.
    duplicates: HashSet<&'a str>,
    /// Folders a listing does not include although it includes entries inside them.
    implied_folders: HashSet<&'a str>,
    skipped: HashSet<&'a str>,
    pending_local_folder_deletes: Vec<&'a str>,
    pending_remote_folder_deletes: Vec<(&'a str, &'a RemoteEntry)>,
    actions: Vec<Action>,
    renames: Vec<Action>,
}

fn sorted(set: HashSet<&str>, cmp: fn(&str, &str) -> std::cmp::Ordering) -> Vec<&str> {
    let mut v: Vec<&str> = set.into_iter().collect();
    v.sort_by(|a, b| cmp(a, b));
    v
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
            remote_folder_deletion: input.remote_folder_deletion,
            duplicates,
            implied_folders,
            skipped: HashSet::new(),
            pending_local_folder_deletes: Vec::new(),
            pending_remote_folder_deletes: Vec::new(),
            actions: Vec::new(),
            renames: case_renames(input),
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
        for action in &self.renames {
            let (path, to_path) = match action {
                Action::RenameLocal { path, to_path, .. }
                | Action::RenameRemote { path, to_path, .. } => (path, to_path),
                _ => unreachable!(),
            };
            for p in self
                .local_all
                .iter()
                .map(|e| e.path.as_str())
                .chain(self.remote_all.iter().map(|e| e.path.as_str()))
                .chain(self.record_order.iter().map(|r| r.path.as_str()))
            {
                if p == path
                    || p == to_path
                    || paths::is_ancestor(path, p)
                    || paths::is_ancestor(to_path, p)
                {
                    self.skipped.insert(p);
                }
            }
        }
        let errors = self.errors();
        let mut error_paths: Vec<&str> = errors.keys().copied().collect();
        error_paths.sort_by(|a, b| paths::compare(a, b));
        for p in error_paths {
            if self.skipped.contains(p) {
                continue;
            }
            self.skipped.insert(p);
            self.actions.push(Action::Error {
                path: p.to_string(),
                message: errors[p].to_string(),
            });
        }

        self.actions.append(&mut self.renames);

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

        // Phase 3: local tree deletions and empty remote folders, deepest first.
        self.apply_folder_deletes();
        SyncPlan {
            actions: self.actions,
        }
    }

    fn folder_record(&self, path: &str) -> Option<&'a SyncRecord> {
        self.valid_record(path).filter(|r| r.is_folder)
    }

    fn file_record(&self, path: &str) -> Option<&'a SyncRecord> {
        self.valid_record(path).filter(|r| !r.is_folder)
    }

    fn valid_record(&self, path: &str) -> Option<&'a SyncRecord> {
        // Replacing a folder cannot transfer its old subtree's deletion authority to the new identity.
        if prefixes(path).filter(|p| *p != path).any(|parent| {
            self.remote_folders.get(parent).is_some_and(|remote| {
                self.records
                    .get(parent)
                    .is_some_and(|record| record.is_folder && record.remote_id != remote.id)
            })
        }) {
            return None;
        }
        self.records.get(path).copied()
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
                if record.is_none_or(|r| r.remote_id != remote.id || r.remote_folder_preserved) {
                    self.actions.push(Action::RecordFolder {
                        path: p,
                        remote_id: remote.id.clone(),
                    });
                }
            }
            (true, None) => {
                if record.is_none_or(|r| r.remote_folder_preserved)
                    || self.has_local_survivors(path)
                {
                    self.actions.push(Action::CreateRemoteFolder { path: p });
                } else {
                    self.pending_local_folder_deletes.push(path);
                }
            }
            (false, Some(remote)) => {
                if record.is_none_or(|r| r.remote_id != remote.id)
                    || self.has_remote_survivors(path)
                {
                    self.actions.push(Action::CreateLocalFolder {
                        path: p,
                        remote_id: remote.id.clone(),
                    });
                } else if self.remote_folder_deletion == RemoteFolderDeletion::AtomicEmptyOnly
                    || record.is_none_or(|r| !r.remote_folder_preserved)
                {
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
        if record.local_deleted
            && !remote_changed(remote, record)
            && self.local_hash(path) == Some(record.local_hash.as_str())
        {
            self.record(path, remote, &record.local_hash, false);
            return;
        }
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
        if record.is_none_or(|r| {
            r.local_deleted || remote_changed(remote, r) || local_hash != r.local_hash
        }) {
            self.record(path, remote, local_hash, false);
        }
    }

    fn local_only(&mut self, path: &str, record: Option<&SyncRecord>) {
        let p = path.to_string();
        match record {
            Some(r) if !r.local_deleted && !self.local_changed(path, r) => {
                self.actions.push(Action::DeleteLocal { path: p });
            }
            _ => self.actions.push(Action::UploadNew { path: p }),
        }
    }

    fn remote_only(&mut self, path: &str, remote: &RemoteEntry, record: Option<&SyncRecord>) {
        let p = path.to_string();
        match record {
            Some(r) if !remote_changed(remote, r) => {
                self.actions.push(Action::DeleteRemote {
                    path: p,
                    remote: remote.clone(),
                });
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

    fn apply_folder_deletes(&mut self) {
        let mut local = self.pending_local_folder_deletes.clone();
        local.sort_by(|a, b| {
            paths::depth(b)
                .cmp(&paths::depth(a))
                .then_with(|| paths::compare(a, b))
        });
        for path in &local {
            self.actions.push(Action::DeleteLocalFolder {
                path: path.to_string(),
                forget: vec![path.to_string()],
            });
        }
        let mut remote = self.pending_remote_folder_deletes.clone();
        remote.sort_by(|(a, _), (b, _)| {
            paths::depth(b)
                .cmp(&paths::depth(a))
                .then_with(|| paths::compare(a, b))
        });
        for (path, entry) in &remote {
            self.actions.push(match self.remote_folder_deletion {
                RemoteFolderDeletion::AtomicEmptyOnly => Action::DeleteRemoteFolder {
                    path: path.to_string(),
                    remote: (*entry).clone(),
                    forget: vec![path.to_string()],
                },
                RemoteFolderDeletion::Unsupported => Action::PreserveRemoteFolder {
                    path: path.to_string(),
                    remote: (*entry).clone(),
                },
            });
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
            let record = self.valid_record(&e.path);
            if e.is_folder {
                record.is_none()
            } else {
                record.is_none_or(|r| {
                    r.local_deleted || e.sha256.as_deref() != Some(r.local_hash.as_str())
                })
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
            let record = self.valid_record(&e.path);
            if e.is_folder {
                record.is_none_or(|r| r.remote_id != e.id)
            } else {
                record.is_none_or(|r| remote_changed(e, r))
            }
        })
    }
}

/// Infer only a unique, record-proven stale spelling. Folder renames require a complete, unchanged stale tree.
fn case_renames(input: &SyncInput) -> Vec<Action> {
    type CaseGroup<'a> = (
        Vec<&'a LocalEntry>,
        Vec<&'a RemoteEntry>,
        Vec<&'a SyncRecord>,
    );
    let mut groups: HashMap<String, CaseGroup<'_>> = HashMap::new();
    for e in &input.local {
        groups
            .entry(paths::fold_case(&e.path))
            .or_default()
            .0
            .push(e);
    }
    for e in &input.remote {
        groups
            .entry(paths::fold_case(&e.path))
            .or_default()
            .1
            .push(e);
    }
    for e in &input.records {
        groups
            .entry(paths::fold_case(&e.path))
            .or_default()
            .2
            .push(e);
    }
    let mut candidates: Vec<_> = groups.values().collect();
    candidates.sort_by(|a, b| {
        let a = a.0.first().map_or("", |e| e.path.as_str());
        let b = b.0.first().map_or("", |e| e.path.as_str());
        paths::compare_by_depth_then_path(a, b)
    });
    let mut covered: Vec<String> = Vec::new();
    let mut actions = Vec::new();
    for (local, remote, records) in candidates {
        let ([l], [r], [record]) = (local.as_slice(), remote.as_slice(), records.as_slice()) else {
            continue;
        };
        let metadata_only = l.path == r.path && record.path != l.path;
        if (l.path == r.path && !metadata_only)
            || l.is_folder != r.is_folder
            || l.is_folder != record.is_folder
            || !is_synced(&l.path, l.is_folder)
            || !is_synced(&r.path, r.is_folder)
            || covered
                .iter()
                .any(|p| paths::is_ancestor(p, &paths::fold_case(&l.path)))
        {
            continue;
        }
        // Do not infer a child rename while its parent's spelling is itself ambiguous.
        if paths::parent(&l.path) != paths::parent(&r.path) {
            continue;
        }
        let parent = paths::parent(&l.path);
        if !parent.is_empty()
            && prefixes(parent).any(|ancestor| {
                groups
                    .get(&paths::fold_case(ancestor))
                    .is_some_and(|(ls, rs, recs)| {
                        let spellings: HashSet<_> = ls
                            .iter()
                            .map(|e| e.path.as_str())
                            .chain(rs.iter().map(|e| e.path.as_str()))
                            .chain(recs.iter().map(|e| e.path.as_str()))
                            .collect();
                        ls.len() > 1
                            || rs.len() > 1
                            || recs.len() > 1
                            || spellings.len() > 1
                            || ls.iter().any(|e| !e.is_folder)
                            || rs.iter().any(|e| !e.is_folder)
                    })
            })
        {
            continue;
        }
        let stale_local = (record.path == l.path || (metadata_only && r.id == record.remote_id))
            && (l.is_folder || l.sha256.as_deref() == Some(record.local_hash.as_str()));
        let stale_remote = record.path == r.path
            && r.id == record.remote_id
            && (r.is_folder || r.version == record.remote_version);
        if !stale_local && !stale_remote {
            continue;
        }
        let (from, to) = if metadata_only {
            (&record.path, &l.path)
        } else if stale_local {
            (&l.path, &r.path)
        } else {
            (&r.path, &l.path)
        };
        if l.is_folder {
            // Every case-folded descendant counts, whatever its spelling: a descendant recorded or listed under
            // another spelling of the folder is not part of a one-to-one mapping and prevents the inference.
            let folded = paths::fold_case(&l.path);
            let relevant: Vec<_> = groups
                .iter()
                .filter(|(key, _)| paths::is_ancestor(&folded, key))
                .map(|(_, group)| group)
                .collect();
            if relevant.iter().any(|(ls, rs, recs)| {
                if let ([], [re], [rec]) = (ls.as_slice(), rs.as_slice(), recs.as_slice()) {
                    if rec.is_folder
                        && rec.remote_folder_preserved
                        && re.is_folder
                        && re.id == rec.remote_id
                        && is_synced(&re.path, true)
                        && is_synced(&rec.path, true)
                    {
                        return rec
                            .path
                            .strip_prefix(&format!("{}/", record.path))
                            .is_none_or(|suffix| re.path != format!("{}/{suffix}", r.path));
                    }
                }
                let ([le], [re], [rec]) = (ls.as_slice(), rs.as_slice(), recs.as_slice()) else {
                    return true;
                };
                let Some(suffix) = le.path.strip_prefix(&format!("{}/", l.path)) else {
                    return true;
                };
                if re.path != format!("{}/{suffix}", r.path)
                    || rec.path != format!("{}/{suffix}", record.path)
                    || le.is_folder != re.is_folder
                    || le.is_folder != rec.is_folder
                    || !is_synced(&le.path, le.is_folder)
                    || !is_synced(&re.path, re.is_folder)
                {
                    return true;
                }
                if stale_local {
                    (!le.is_folder && le.sha256.as_deref() != Some(rec.local_hash.as_str()))
                        || (metadata_only && re.id != rec.remote_id)
                } else {
                    re.id != rec.remote_id || (!re.is_folder && re.version != rec.remote_version)
                }
            }) {
                continue;
            }
        }
        covered.push(paths::fold_case(from));
        let action = if stale_local {
            Action::RenameLocal {
                path: from.clone(),
                to_path: to.clone(),
                is_folder: l.is_folder,
                remote: (*r).clone(),
                sha256: l.sha256.clone(),
            }
        } else {
            Action::RenameRemote {
                path: from.clone(),
                to_path: to.clone(),
                is_folder: l.is_folder,
                remote: (*r).clone(),
                sha256: None,
            }
        };
        actions.push(action);
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_case_rename_rejects_unproven_missing_descendants() {
        for (is_folder, preserved, id) in [
            (true, false, "child"),
            (true, true, "replacement"),
            (false, true, "child"),
        ] {
            let input: SyncInput = serde_json::from_value(serde_json::json!({
                "local": [{"path":"f","isFolder":true}],
                "remote": [
                    {"path":"F","id":"parent","isFolder":true,"version":"","modifiedMillis":0},
                    {"path":"F/Sub","id":id,"isFolder":is_folder,"version":"","modifiedMillis":0}
                ],
                "records": [
                    {"path":"F","isFolder":true,"remoteId":"parent","remoteVersion":"","localHash":"","syncedAt":0},
                    {"path":"F/Sub","isFolder":is_folder,"remoteId":"child","remoteVersion":"","localHash":"","syncedAt":0,"remoteFolderPreserved":preserved}
                ],
                "remoteFolderDeletion":"unsupported"
            })).unwrap();
            assert!(plan(&input)
                .actions
                .iter()
                .all(|a| matches!(a, Action::Error { .. })));
        }
    }

    #[test]
    fn replaced_ancestor_cannot_authorize_deleting_unchanged_descendants() {
        let input: SyncInput = serde_json::from_value(serde_json::json!({
            "local": [],
            "remote": [
                {"path":"F","id":"replacement","isFolder":true,"version":"","modifiedMillis":0},
                {"path":"F/a.md","id":"same-child","isFolder":false,"version":"v1","modifiedMillis":0}
            ],
            "records": [
                {"path":"F","isFolder":true,"remoteId":"old","remoteVersion":"","localHash":"","syncedAt":0,"remoteFolderPreserved":true},
                {"path":"F/a.md","isFolder":false,"remoteId":"same-child","remoteVersion":"v1","localHash":"h","syncedAt":0}
            ],
            "remoteFolderDeletion":"unsupported"
        })).unwrap();
        assert!(matches!(
            plan(&input).actions.as_slice(),
            [Action::CreateLocalFolder { .. }, Action::Download { .. }]
        ));
    }

    #[test]
    fn recreation_retires_only_preserved_folder_baselines_in_path_order() {
        let mut input: SyncInput = serde_json::from_value(serde_json::json!({
            "local":[{"path":"F","isFolder":true},{"path":"G","isFolder":true}],
            "remote":[],
            "records":[
                {"path":"G","isFolder":true,"remoteId":"g","remoteVersion":"","localHash":"","syncedAt":0,"remoteFolderPreserved":true},
                {"path":"F","isFolder":true,"remoteId":"f","remoteVersion":"","localHash":"","syncedAt":0},
                {"path":"F/a.md","isFolder":false,"remoteId":"a","remoteVersion":"v1","localHash":"h","syncedAt":0,"remoteFolderPreserved":true}
            ]
        })).unwrap();
        assert_eq!(recreated_folders(&input.local, &input.records), vec!["G"]);
        assert!(
            !input.records[1].remote_folder_preserved,
            "old records migrate as false"
        );
        assert_eq!(
            input.remote_folder_deletion,
            RemoteFolderDeletion::AtomicEmptyOnly
        );
        input.records[1].remote_folder_preserved = true;
        input.records.push(input.records[1].clone());
        assert_eq!(
            recreated_folders(&input.local, &input.records),
            vec!["F", "G"]
        );
    }

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
