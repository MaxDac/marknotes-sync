//! Property-based tests of marknotes-sync: one property per invariant, named after its ID (see each doc comment for
//! the rule and `spec/note-format.md`, "Sync", for the behaviour they come from).
//!
//! Policy:
//! - Runs are deterministic: every property uses `support::gen::config()`, a fixed RNG seed (overridden by
//!   `MARKNOTES_PROPTEST_SEED`, which the nightly job rotates and logs) and `PROPTEST_CASES` cases (256 by default).
//!   The seed is printed in the output of a failing test.
//! - Failure persistence is disabled: no `proptest-regressions/` files are written. A failure found by a property is
//!   shrunk, checked against the spec, and committed as a concrete `#[test]`: in `tests/regressions.rs` once fixed,
//!   or here as `#[ignore = "DEFECT: ..."]` while the code still violates the spec.
//! - A generator domain is narrowed for one property only when the excluded inputs are genuinely outside that
//!   invariant, and the doc comment of the property says so.

mod support;

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use marknotes_sync::front_matter;
use marknotes_sync::manifest::{
    build_manifest, parse_manifest, serialize_manifest, Manifest, ManifestEntry,
};
use marknotes_sync::merge::{
    lines, merge3, merge_text, ConflictChoice, MergeChunk, MARKER_LOCAL, MARKER_REMOTE,
    MARKER_SEPARATOR,
};
use marknotes_sync::model::{
    Action, LocalEntry, ReconcileInput, ReconcileOutcome, RemoteEntry, Side, SyncInput, SyncRecord,
};
use marknotes_sync::paths;
use marknotes_sync::plan::{plan, CASE_MESSAGE, CLASH_MESSAGE, DUPLICATE_MESSAGE};
use marknotes_sync::reconcile::reconcile;
use marknotes_sync::scheduler::Scheduler;
use proptest::prelude::*;
use proptest::sample::select;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use support::gen::{self, is_synced, prefixes};

const BOM: char = '\u{feff}';

// ---------------------------------------------------------------------------------------------------------------
// Plan helpers
// ---------------------------------------------------------------------------------------------------------------

fn path_of(a: &Action) -> &str {
    match a {
        Action::Error { path, .. }
        | Action::RecordFolder { path, .. }
        | Action::CreateRemoteFolder { path }
        | Action::CreateLocalFolder { path, .. }
        | Action::Forget { path }
        | Action::UploadNew { path }
        | Action::UploadRevision { path, .. }
        | Action::Download { path, .. }
        | Action::Record { path, .. }
        | Action::DeleteLocal { path }
        | Action::DeleteRemote { path, .. }
        | Action::DeleteLocalFolder { path, .. }
        | Action::DeleteRemoteFolder { path, .. }
        | Action::Reconcile { path, .. } => path,
    }
}

fn forget_list(a: &Action) -> &[String] {
    match a {
        Action::DeleteLocalFolder { forget, .. } | Action::DeleteRemoteFolder { forget, .. } => {
            forget
        }
        _ => &[],
    }
}

fn remote_payload(a: &Action) -> Option<&RemoteEntry> {
    match a {
        Action::UploadRevision { remote, .. }
        | Action::Download { remote, .. }
        | Action::Record { remote, .. }
        | Action::DeleteRemote { remote, .. }
        | Action::DeleteRemoteFolder { remote, .. }
        | Action::Reconcile { remote, .. } => Some(remote),
        _ => None,
    }
}

fn is_error(a: &Action) -> bool {
    matches!(a, Action::Error { .. })
}

fn is_folder_delete(a: &Action) -> bool {
    matches!(
        a,
        Action::DeleteLocalFolder { .. } | Action::DeleteRemoteFolder { .. }
    )
}

/// Step 1 actions that make a folder exist on both sides.
fn is_folder_step(a: &Action) -> bool {
    matches!(
        a,
        Action::RecordFolder { .. }
            | Action::CreateRemoteFolder { .. }
            | Action::CreateLocalFolder { .. }
    )
}

/// Step 2 actions on a file.
fn is_file_step(a: &Action) -> bool {
    matches!(
        a,
        Action::UploadNew { .. }
            | Action::UploadRevision { .. }
            | Action::Download { .. }
            | Action::Record { .. }
            | Action::DeleteLocal { .. }
            | Action::DeleteRemote { .. }
            | Action::Reconcile { .. }
    )
}

fn repeated<'a>(paths: impl Iterator<Item = &'a str>) -> HashSet<&'a str> {
    let mut seen = HashSet::new();
    paths.filter(|p| !seen.insert(*p)).collect()
}

/// The errors the spec requires ("What is synced"): one per path, file/folder clash first, then
/// `is listed more than once` (for the path and everything inside it), then the case message (for paths that, or
/// a folder above which, differ only in case from another path across the listings and the records).
fn expected_errors(input: &SyncInput) -> BTreeMap<String, &'static str> {
    let local: Vec<&LocalEntry> = input
        .local
        .iter()
        .filter(|e| is_synced(&e.path, e.is_folder))
        .collect();
    let remote: Vec<&RemoteEntry> = input
        .remote
        .iter()
        .filter(|e| is_synced(&e.path, e.is_folder))
        .collect();
    let set = |folder: bool, l: bool| -> HashSet<&str> {
        if l {
            local
                .iter()
                .filter(|e| e.is_folder == folder)
                .map(|e| e.path.as_str())
                .collect()
        } else {
            remote
                .iter()
                .filter(|e| e.is_folder == folder)
                .map(|e| e.path.as_str())
                .collect()
        }
    };
    let (local_folders, local_files) = (set(true, true), set(false, true));
    let (remote_folders, remote_files) = (set(true, false), set(false, false));
    let clash: HashSet<&str> = local_folders
        .intersection(&remote_files)
        .chain(local_files.intersection(&remote_folders))
        .copied()
        .collect();
    let mut dup = repeated(local.iter().map(|e| e.path.as_str()));
    dup.extend(repeated(remote.iter().map(|e| e.path.as_str())));
    dup.extend(repeated(input.records.iter().map(|r| r.path.as_str())));
    let all: HashSet<&str> = local
        .iter()
        .map(|e| e.path.as_str())
        .chain(remote.iter().map(|e| e.path.as_str()))
        .chain(input.records.iter().map(|r| r.path.as_str()))
        .collect();
    let mut spellings: HashMap<String, HashSet<&str>> = HashMap::new();
    for p in &all {
        for q in prefixes(p) {
            spellings.entry(paths::fold_case(q)).or_default().insert(q);
        }
    }
    let mut out = BTreeMap::new();
    for p in all {
        let message = if clash.contains(p) {
            CLASH_MESSAGE
        } else if prefixes(p).any(|q| dup.contains(q)) {
            DUPLICATE_MESSAGE
        } else if prefixes(p).any(|q| spellings[&paths::fold_case(q)].len() > 1) {
            CASE_MESSAGE
        } else {
            continue;
        };
        out.insert(p.to_string(), message);
    }
    out
}

/// Shows a plan with its input when a property fails.
struct Shown<'a>(&'a SyncInput, &'a [Action]);

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "input: {}\nplan: {}",
            serde_json::to_string(self.0).unwrap_or_default(),
            serde_json::to_string(self.1).unwrap_or_default()
        )
    }
}

fn shuffled_input(input: SyncInput) -> impl Strategy<Value = (SyncInput, SyncInput)> {
    (
        Just(input.clone()),
        Just(input.local).prop_shuffle(),
        Just(input.remote).prop_shuffle(),
        Just(input.records).prop_shuffle(),
    )
        .prop_map(move |(original, local, remote, records)| {
            let manifest = original.manifest.clone();
            (
                original,
                SyncInput {
                    local,
                    remote,
                    records,
                    manifest,
                },
            )
        })
}

// ---------------------------------------------------------------------------------------------------------------
// Planner
// ---------------------------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(gen::config())]

    /// PLAN-TOTAL: `plan` never panics on arbitrary bounded inputs (any strings, duplicates, clashes, listings that
    /// are not prefix-closed, records of the wrong kind, any manifest text), and its plan serializes.
    #[test]
    fn plan_total_never_panics(input in gen::arbitrary_sync_input()) {
        let p = plan(&input);
        prop_assert!(serde_json::to_string(&p).is_ok());
    }

    /// PLAN-DET: the plan does not depend on the order of the local listing, the drive listing or the records,
    /// including inconsistent inputs (duplicates, clashes, missing folders).
    #[test]
    fn plan_det_permutation_invariant(
        (input, shuffled) in gen::inconsistent_sync_input(14).prop_flat_map(shuffled_input)
    ) {
        let a = plan(&input).actions;
        let b = plan(&shuffled).actions;
        prop_assert_eq!(&a, &b, "{}", Shown(&input, &a));
    }

    /// PLAN-ERR: the errors are those of the spec (one per path, by message precedence); an errored path gets no
    /// other action and is in no `forget` list; nothing inside a path errored as a duplicate or a case collision
    /// gets a non-error action (the root `""`, which only a corrupt record can name, excepted); no folder deletion
    /// removes an errored entry of that side.
    #[test]
    fn plan_err_errored_paths_are_skipped(input in gen::inconsistent_sync_input(14)) {
        let actions = plan(&input).actions;
        let shown = Shown(&input, &actions);
        let mut errors: BTreeMap<String, &str> = BTreeMap::new();
        for a in actions.iter().filter(|a| is_error(a)) {
            let Action::Error { path, message } = a else { unreachable!() };
            let known = [CLASH_MESSAGE, DUPLICATE_MESSAGE, CASE_MESSAGE];
            let message = known.into_iter().find(|m| m == message);
            prop_assert!(message.is_some(), "unknown message; {}", shown);
            prop_assert!(errors.insert(path.clone(), message.unwrap()).is_none(), "two errors for {path}; {}", shown);
        }
        let expected = expected_errors(&input);
        let expected: BTreeMap<String, &str> = expected.into_iter().collect();
        prop_assert_eq!(&errors, &expected, "{}", shown);
        for a in actions.iter().filter(|a| !is_error(a)) {
            let p = path_of(a);
            prop_assert!(!errors.contains_key(p), "errored {p} has {a:?}; {}", shown);
            for f in forget_list(a) {
                prop_assert!(!errors.contains_key(f.as_str()), "errored {f} forgotten by {a:?}; {}", shown);
            }
            for (e, m) in &errors {
                // An errored "" (a corrupt record for the root) is not a folder: the root holds every entry.
                if *m != CLASH_MESSAGE && !e.is_empty() {
                    prop_assert!(!paths::is_ancestor(e, p), "{p} is inside errored {e}: {a:?}; {}", shown);
                }
            }
            match a {
                Action::DeleteLocalFolder { path, .. } => {
                    for e in input.local.iter().filter(|e| errors.contains_key(e.path.as_str())) {
                        prop_assert!(!paths::is_ancestor(path, &e.path), "{a:?} deletes errored {}; {}", e.path, shown);
                    }
                }
                Action::DeleteRemoteFolder { path, .. } => {
                    for e in input.remote.iter().filter(|e| errors.contains_key(e.path.as_str())) {
                        prop_assert!(!paths::is_ancestor(path, &e.path), "{a:?} deletes errored {}; {}", e.path, shown);
                    }
                }
                _ => {}
            }
        }
    }

    /// PLAN-ORDER: errors first, by path (UTF-16 order); then folder steps by depth then path (parents before
    /// children); then file steps by path; folder deletions last, top-most only, with no file deletion inside a
    /// folder deletion of the same side.
    #[test]
    fn plan_order_parents_first_deletions_last(input in gen::sync_input(14)) {
        let actions = plan(&input).actions;
        let shown = Shown(&input, &actions);
        let errors = actions.iter().take_while(|a| is_error(a)).count();
        prop_assert!(actions[errors..].iter().all(|a| !is_error(a)), "error after other actions; {}", shown);
        for w in actions[..errors].windows(2) {
            prop_assert_eq!(paths::compare(path_of(&w[0]), path_of(&w[1])), Ordering::Less, "{}", shown);
        }
        let deletions = actions.iter().position(is_folder_delete).unwrap_or(actions.len());
        prop_assert!(actions[deletions..].iter().all(is_folder_delete), "folder deletion not last; {}", shown);

        let folder_steps: Vec<(usize, &str)> =
            actions.iter().enumerate().filter(|(_, a)| is_folder_step(a)).map(|(i, a)| (i, path_of(a))).collect();
        let file_steps: Vec<(usize, &str)> =
            actions.iter().enumerate().filter(|(_, a)| is_file_step(a)).map(|(i, a)| (i, path_of(a))).collect();
        for w in folder_steps.windows(2) {
            prop_assert_eq!(paths::compare_by_depth_then_path(w[0].1, w[1].1), Ordering::Less, "{}", shown);
        }
        for w in file_steps.windows(2) {
            prop_assert_ne!(paths::compare(w[0].1, w[1].1), Ordering::Greater, "{}", shown);
        }
        if let (Some(last_folder), Some(first_file)) = (folder_steps.last(), file_steps.first()) {
            prop_assert!(last_folder.0 < first_file.0, "folder step after file step; {}", shown);
        }
        // Errors are not steps: they are listed first and may name paths inside folders that get created.
        for (j, a) in actions.iter().enumerate().filter(|(_, a)| !is_error(a)) {
            for (i, folder) in &folder_steps {
                if paths::is_ancestor(folder, path_of(a)) {
                    prop_assert!(i < &j, "{a:?} before its folder {folder}; {}", shown);
                }
            }
        }

        let local_deletes: Vec<&str> = actions.iter()
            .filter(|a| matches!(a, Action::DeleteLocalFolder { .. })).map(path_of).collect();
        let remote_deletes: Vec<&str> = actions.iter()
            .filter(|a| matches!(a, Action::DeleteRemoteFolder { .. })).map(path_of).collect();
        for (deletes, file_kind) in [(&local_deletes, true), (&remote_deletes, false)] {
            for d in deletes.iter() {
                prop_assert!(
                    !deletes.iter().any(|o| paths::is_ancestor(o, d)),
                    "{d} deleted under another folder deletion; {}", shown
                );
                for a in &actions {
                    let inside = match a {
                        Action::DeleteLocal { path } if file_kind => paths::is_ancestor(d, path),
                        Action::DeleteRemote { path, .. } if !file_kind => paths::is_ancestor(d, path),
                        _ => false,
                    };
                    prop_assert!(!inside, "{a:?} inside folder deletion {d}; {}", shown);
                }
            }
        }
    }

    /// PLAN-ONE: at most one action per path; the only allowed pair is `record` then `uploadRevision` of the same
    /// drive entry, the record keeping the recorded local hash (a drive revision with content we already have,
    /// then the local change). Every action matches the listings: its drive entry is the listed one, and the
    /// entry it reads exists on that side.
    #[test]
    fn plan_one_mutation_per_path(input in gen::sync_input(14)) {
        let actions = plan(&input).actions;
        let shown = Shown(&input, &actions);
        let local: HashMap<&str, &LocalEntry> = input.local.iter().map(|e| (e.path.as_str(), e)).collect();
        let remote: HashMap<&str, &RemoteEntry> = input.remote.iter().map(|e| (e.path.as_str(), e)).collect();
        let records: HashMap<&str, &SyncRecord> = input.records.iter().map(|r| (r.path.as_str(), r)).collect();
        let mut by_path: HashMap<&str, Vec<&Action>> = HashMap::new();
        for a in actions.iter().filter(|a| !is_error(a)) {
            by_path.entry(path_of(a)).or_default().push(a);
        }
        for (p, v) in &by_path {
            if v.len() == 1 {
                continue;
            }
            prop_assert_eq!(v.len(), 2, "{} actions for {}; {}", v.len(), p, shown);
            match (v[0], v[1]) {
                (
                    Action::Record { remote: r1, sha256, update_base: false, .. },
                    Action::UploadRevision { remote: r2, .. },
                ) => {
                    prop_assert_eq!(r1, r2, "{}", shown);
                    let rec = records.get(p).map(|r| r.local_hash.as_str());
                    prop_assert_eq!(Some(sha256.as_str()), rec, "{}", shown);
                }
                _ => prop_assert!(false, "unexpected pair for {p}: {v:?}; {}", shown),
            }
        }
        let local_file = |p: &str| local.get(p).is_some_and(|e| !e.is_folder);
        let local_folder = |p: &str| local.get(p).is_some_and(|e| e.is_folder);
        let remote_folder = |p: &str| remote.get(p).is_some_and(|e| e.is_folder);
        for a in actions.iter().filter(|a| !is_error(a)) {
            let p = path_of(a);
            if let Some(r) = remote_payload(a) {
                prop_assert_eq!(r.path.as_str(), p, "{}", shown);
                prop_assert_eq!(remote.get(p).copied(), Some(r), "payload is not the listed entry; {}", shown);
            }
            let ok = match a {
                Action::UploadNew { .. } | Action::UploadRevision { .. } | Action::DeleteLocal { .. } => local_file(p),
                Action::Reconcile { .. } => local_file(p) && !remote[p].is_folder,
                Action::Download { .. } | Action::DeleteRemote { .. } => !remote[p].is_folder,
                Action::Record { sha256, .. } => {
                    local_file(p) && (local[p].sha256.as_deref() == Some(sha256.as_str())
                        || records.get(p).is_some_and(|r| &r.local_hash == sha256))
                }
                Action::CreateRemoteFolder { .. } | Action::DeleteLocalFolder { .. } => local_folder(p),
                Action::CreateLocalFolder { remote_id, .. } => {
                    remote_folder(p) && &remote[p].id == remote_id
                }
                Action::RecordFolder { remote_id, .. } => {
                    local_folder(p) && remote_folder(p) && &remote[p].id == remote_id
                }
                Action::DeleteRemoteFolder { .. } => remote_folder(p),
                Action::Forget { .. } => !local.contains_key(p) && !remote.contains_key(p) && records.contains_key(p),
                Action::Error { .. } => true,
            };
            prop_assert!(ok, "{a:?} does not match the listings; {}", shown);
        }
    }

    /// PLAN-IGNORED: entries that are not synced (other files, hidden entries, notes in `attachments/`, invalid
    /// names) get no action and are in no `forget` list, and the folders holding them are not deleted on their
    /// side.
    #[test]
    fn plan_ignored_entries_keep_their_folder(input in gen::sync_input(14)) {
        let actions = plan(&input).actions;
        let shown = Shown(&input, &actions);
        let synced: HashSet<&str> = input.local.iter().map(|e| (e.path.as_str(), e.is_folder))
            .chain(input.remote.iter().map(|e| (e.path.as_str(), e.is_folder)))
            .filter(|(p, f)| is_synced(p, *f)).map(|(p, _)| p).collect();
        let ignored_local: Vec<&str> = input.local.iter()
            .filter(|e| !is_synced(&e.path, e.is_folder)).map(|e| e.path.as_str()).collect();
        let ignored_remote: Vec<&str> = input.remote.iter()
            .filter(|e| !is_synced(&e.path, e.is_folder)).map(|e| e.path.as_str()).collect();
        for a in &actions {
            for p in ignored_local.iter().chain(&ignored_remote).filter(|p| !synced.contains(*p)) {
                prop_assert_ne!(path_of(a), *p, "{}", shown);
                prop_assert!(!forget_list(a).iter().any(|f| f == p), "{}", shown);
            }
            match a {
                Action::DeleteLocalFolder { path, .. } => {
                    for p in &ignored_local {
                        prop_assert!(!paths::is_ancestor(path, p), "{a:?} deletes ignored {p}; {}", shown);
                    }
                }
                Action::DeleteRemoteFolder { path, .. } => {
                    for p in &ignored_remote {
                        prop_assert!(!paths::is_ancestor(path, p), "{a:?} deletes ignored {p}; {}", shown);
                    }
                }
                _ => {}
            }
        }
    }

    /// PLAN-NOOP: both sides identical (including ignored entries) and every synced entry recorded as it is gives
    /// an empty plan, whatever the manifest. Domain: no paths that differ only in case, which are errors by spec.
    #[test]
    fn plan_noop_when_in_sync(input in gen::synced_input(14)) {
        let actions = plan(&input).actions;
        prop_assert!(actions.is_empty(), "{}", Shown(&input, &actions));
    }

    /// PLAN-CHANGE-WINS: a change wins over a deletion. A local file that is new or changed since its record, or
    /// a local folder never synced, is never deleted locally (by `deleteLocal` or inside a `deleteLocalFolder`);
    /// likewise on the drive for a new or changed drive file (id or version) or a drive folder never synced.
    #[test]
    fn plan_change_wins_over_deletion(input in gen::sync_input(14)) {
        let actions = plan(&input).actions;
        let shown = Shown(&input, &actions);
        let records: HashMap<&str, &SyncRecord> = input.records.iter().map(|r| (r.path.as_str(), r)).collect();
        let deletes = |a: &Action, local: bool, p: &str| match a {
            Action::DeleteLocal { path } if local => path == p,
            Action::DeleteRemote { path, .. } if !local => path == p,
            Action::DeleteLocalFolder { path, .. } if local => path == p || paths::is_ancestor(path, p),
            Action::DeleteRemoteFolder { path, .. } if !local => path == p || paths::is_ancestor(path, p),
            _ => false,
        };
        for e in input.local.iter().filter(|e| is_synced(&e.path, e.is_folder)) {
            let record = records.get(e.path.as_str()).filter(|r| r.is_folder == e.is_folder);
            let changed = match record {
                None => true,
                Some(r) => !e.is_folder && e.sha256.as_deref() != Some(r.local_hash.as_str()),
            };
            if changed {
                for a in &actions {
                    prop_assert!(!deletes(a, true, &e.path), "{a:?} deletes local change {}; {}", e.path, shown);
                }
            }
        }
        for e in input.remote.iter().filter(|e| is_synced(&e.path, e.is_folder)) {
            let record = records.get(e.path.as_str()).filter(|r| r.is_folder == e.is_folder);
            let changed = match record {
                None => true,
                Some(r) => !e.is_folder && (r.remote_version != e.version || r.remote_id != e.id),
            };
            if changed {
                for a in &actions {
                    prop_assert!(!deletes(a, false, &e.path), "{a:?} deletes drive change {}; {}", e.path, shown);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------------------------------------------

fn counts<'a>(lines: impl IntoIterator<Item = &'a str>) -> HashMap<&'a str, i64> {
    let mut m = HashMap::new();
    for l in lines {
        *m.entry(l).or_insert(0) += 1;
    }
    m
}

/// Whether `parts` occur in `whole` as contiguous, non-overlapping runs, in order.
fn ordered_runs(parts: &[&[String]], whole: &[&str]) -> bool {
    let mut pos = 0;
    for part in parts.iter().filter(|p| !p.is_empty()) {
        let found = (pos..=whole.len().saturating_sub(part.len())).find(|&i| {
            i + part.len() <= whole.len() && part.iter().zip(&whole[i..]).all(|(a, b)| a == b)
        });
        match found {
            Some(i) => pos = i + part.len(),
            None => return false,
        }
    }
    true
}

/// MERGE-PRESERVE for one merge: every line a parent added (beyond its count in the base) is in the result, as a
/// resolved line or a conflict side; conflict sides are runs of the parents, in order; without a base, taking
/// every conflict's local (remote) side gives back the local (remote) text.
fn check_preserve(base: Option<&str>, local: &str, remote: &str) -> Result<(), TestCaseError> {
    let m = merge3(base, local, remote);
    let (l, r) = (lines(local), lines(remote));
    let b = base.map(lines).unwrap_or_default();
    let mut out: HashMap<&str, i64> = HashMap::new();
    let mut locals: Vec<&[String]> = Vec::new();
    let mut remotes: Vec<&[String]> = Vec::new();
    let mut bases: Vec<&[String]> = Vec::new();
    let mut conflicts = 0;
    for c in &m.chunks {
        match c {
            MergeChunk::Resolved { lines, .. } => {
                for x in lines {
                    *out.entry(x).or_insert(0) += 1;
                }
            }
            MergeChunk::Conflict {
                base,
                local,
                remote,
            } => {
                conflicts += 1;
                for x in local.iter().chain(remote) {
                    *out.entry(x).or_insert(0) += 1;
                }
                locals.push(local);
                remotes.push(remote);
                bases.push(base);
            }
        }
    }
    prop_assert_eq!(m.conflicts, conflicts);
    prop_assert_eq!(m.has_conflicts, conflicts > 0);
    let cb = counts(b.iter().copied());
    for (parent, name) in [(&l, "local"), (&r, "remote")] {
        for (x, n) in counts(parent.iter().copied()) {
            let added = n - cb.get(x).copied().unwrap_or(0);
            prop_assert!(
                out.get(x).copied().unwrap_or(0) >= added,
                "{name} line {x:?} lost: {:?}",
                m.chunks
            );
        }
    }
    prop_assert!(
        ordered_runs(&locals, &l),
        "conflict local sides are not runs of local"
    );
    prop_assert!(
        ordered_runs(&remotes, &r),
        "conflict remote sides are not runs of remote"
    );
    if base.is_some() {
        prop_assert!(
            ordered_runs(&bases, &b),
            "conflict base sides are not runs of base"
        );
    } else {
        let all = |c: ConflictChoice| {
            (0..conflicts)
                .map(|i| (i, c))
                .collect::<HashMap<i32, ConflictChoice>>()
        };
        prop_assert_eq!(merge_text(&m.chunks, &all(ConflictChoice::Local)), local);
        prop_assert_eq!(merge_text(&m.chunks, &all(ConflictChoice::Remote)), remote);
    }
    Ok(())
}

fn terminated(s: String) -> String {
    if s.is_empty() || s.ends_with('\n') {
        s
    } else {
        s + "\n"
    }
}

/// The merged text as the spec defines it: resolved lines as they are; for conflict `i`, its local side, its remote
/// side, both (local then remote, on a new line), neither, or, when unresolved, both sides between the markers.
fn reference_render(chunks: &[MergeChunk], choices: &HashMap<i32, ConflictChoice>) -> String {
    let mut out = String::new();
    let mut i = 0;
    for c in chunks {
        match c {
            MergeChunk::Resolved { lines, .. } => out += &lines.concat(),
            MergeChunk::Conflict { local, remote, .. } => {
                let (l, r) = (local.concat(), remote.concat());
                out += &match choices.get(&i) {
                    Some(ConflictChoice::Local) => l,
                    Some(ConflictChoice::Remote) => r,
                    Some(ConflictChoice::Both) => terminated(l) + &r,
                    Some(ConflictChoice::Neither) => String::new(),
                    None => format!(
                        "{MARKER_LOCAL}\n{}{MARKER_SEPARATOR}\n{}{MARKER_REMOTE}\n",
                        terminated(l),
                        terminated(r)
                    ),
                };
                i += 1;
            }
        }
    }
    out
}

fn marker_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> usize {
    lines
        .into_iter()
        .filter(|l| {
            let content = l.strip_suffix('\n').unwrap_or(l);
            [MARKER_LOCAL, MARKER_SEPARATOR, MARKER_REMOTE].contains(&content)
        })
        .count()
}

fn conflict_choice() -> impl Strategy<Value = ConflictChoice> {
    select(vec![
        ConflictChoice::Local,
        ConflictChoice::Remote,
        ConflictChoice::Both,
        ConflictChoice::Neither,
    ])
}

proptest! {
    #![proptest_config(gen::config())]

    /// MERGE-IDENT: `merge3(b, x, x) = x` (with or without a base), `merge3(b, b, r) = r` and
    /// `merge3(b, l, b) = l`, all without conflicts.
    #[test]
    fn merge_ident_trivial_merges((b, l, r) in gen::text_triple()) {
        for (base, local, remote, expected) in [
            (Some(b.as_str()), &l, &l, &l),
            (None, &l, &l, &l),
            (Some(b.as_str()), &b, &r, &r),
            (Some(b.as_str()), &l, &b, &l),
        ] {
            let m = merge3(base, local, remote);
            prop_assert!(!m.has_conflicts, "{:?}", m.chunks);
            prop_assert_eq!(m.conflicts, 0);
            let all_resolved = m.chunks.iter().all(|c| matches!(c, MergeChunk::Resolved { .. }));
            prop_assert!(all_resolved);
            prop_assert_eq!(&m.text, expected);
        }
    }

    /// MERGE-PRESERVE: every line unique to either parent is in a resolved chunk or a conflict side, and conflict
    /// chunks hold both full sides (runs of the parents; without a base each parent is recovered entirely).
    #[test]
    fn merge_preserve_keeps_every_line((b, l, r) in gen::text_triple(), with_base in any::<bool>()) {
        check_preserve(with_base.then_some(b.as_str()), &l, &r)?;
    }

    /// MERGE-RENDER: `merge_text` is the concatenation the chunks and choices define, `MergeResult.text` is it with
    /// no choices, and markers are added only for unresolved conflicts (three marker lines each).
    #[test]
    fn merge_render_matches_chunks(
        (b, l, r) in gen::text_triple(),
        with_base in any::<bool>(),
        choices in proptest::collection::vec(proptest::option::of(conflict_choice()), 0..6),
    ) {
        let m = merge3(with_base.then_some(b.as_str()), &l, &r);
        prop_assert_eq!(&m.text, &merge_text(&m.chunks, &HashMap::new()));
        let choices: HashMap<i32, ConflictChoice> = choices
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.map(|c| (i as i32, c)))
            .collect();
        let text = merge_text(&m.chunks, &choices);
        prop_assert_eq!(&text, &reference_render(&m.chunks, &choices));
        let mut expected_markers = 0;
        let mut i = 0;
        for c in &m.chunks {
            match c {
                MergeChunk::Resolved { lines, .. } => expected_markers += marker_lines(lines.iter().map(String::as_str)),
                MergeChunk::Conflict { local, remote, .. } => {
                    let (ml, mr) = (
                        marker_lines(local.iter().map(String::as_str)),
                        marker_lines(remote.iter().map(String::as_str)),
                    );
                    expected_markers += match choices.get(&i) {
                        Some(ConflictChoice::Local) => ml,
                        Some(ConflictChoice::Remote) => mr,
                        Some(ConflictChoice::Both) => ml + mr,
                        Some(ConflictChoice::Neither) => 0,
                        None => ml + mr + 3,
                    };
                    i += 1;
                }
            }
        }
        prop_assert_eq!(marker_lines(lines(&text)), expected_markers, "{:?}", text);
    }
}

proptest! {
    // Each case diffs ~1000 lines at an edit distance around the bound: a few cases are enough.
    #![proptest_config(gen::config_fraction(64))]

    /// MERGE-PRESERVE across the maximum edit distance of `merge.rs` (2000): texts of ~1000 lines with nearly every
    /// line replaced on one or both sides, so that the diff is cut off on some cases and not on others.
    #[test]
    fn merge_preserve_across_edit_distance_bound((b, l, r) in gen::large_text_triple(), with_base in any::<bool>()) {
        check_preserve(with_base.then_some(b.as_str()), &l, &r)?;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Reconcile
// ---------------------------------------------------------------------------------------------------------------

fn millis() -> impl Strategy<Value = i64> {
    prop_oneof![3 => -2i64..3, 1 => any::<i64>()]
}

fn with_prefix(bom: bool, block: Option<String>, body: String) -> String {
    let bom = if bom { "\u{feff}" } else { "" };
    match block {
        Some(b) => format!("{bom}{b}{body}"),
        None => format!("{bom}{body}"),
    }
}

fn reconcile_input() -> impl Strategy<Value = ReconcileInput> {
    (
        gen::note_path(3),
        gen::text_triple(),
        proptest::collection::vec(
            proptest::option::weighted(0.6, gen::front_matter_block()),
            3,
        ),
        proptest::collection::vec(proptest::bool::weighted(0.15), 3),
        proptest::bool::weighted(0.7),
        millis(),
        millis(),
        gen::valid_id(),
    )
        .prop_flat_map(|t| {
            let existing = gen::existing_paths_near(t.0.clone());
            (Just(t), existing, proptest::bool::weighted(0.8))
        })
        .prop_map(
            |(
                (path, (b, l, r), blocks, boms, has_base, lm, rm, new_id),
                mut existing,
                include_path,
            )| {
                if include_path {
                    existing.push(path.clone());
                }
                let [bb, lb, rb]: [Option<String>; 3] = blocks.try_into().expect("three blocks");
                ReconcileInput {
                    local: with_prefix(boms[1], lb, l),
                    local_modified_millis: lm,
                    remote: with_prefix(boms[2], rb, r),
                    remote_entry: RemoteEntry {
                        path: path.clone(),
                        id: "r1".into(),
                        is_folder: false,
                        version: "v2".into(),
                        modified_millis: rm,
                    },
                    base: has_base.then(|| with_prefix(boms[0], bb, b)),
                    existing_paths: existing,
                    new_id,
                    path,
                }
            },
        )
}

/// The key lines of the front-matter block of `text` (without line endings), or `None` when there is no block.
fn header_keys(text: &str) -> Option<Vec<String>> {
    let (header, _) = front_matter::split(text);
    if header.is_empty() {
        return None;
    }
    let header = header.strip_prefix(BOM).unwrap_or(header);
    let ls: Vec<String> = header
        .split_inclusive('\n')
        .map(|l| l.strip_suffix('\n').unwrap_or(l))
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    Some(ls[1..ls.len() - 1].to_vec())
}

/// The body of `text` as the spec splits it: after the block (which holds a leading byte order mark), or the whole
/// text without its byte order mark when there is no block.
fn body(text: &str) -> &str {
    let (header, body) = front_matter::split(text);
    if header.is_empty() {
        text.strip_prefix(BOM).unwrap_or(text)
    } else {
        body
    }
}

/// The keys `with_id` must produce: the first `id:` line replaced, or `id` added as the first key.
fn keys_with_id(keys: Option<Vec<String>>, id: &str) -> Vec<String> {
    let mut keys = keys.unwrap_or_default();
    let line = format!("id: {id}");
    match keys.iter().position(|k| k.starts_with("id:")) {
        Some(i) => keys[i] = line,
        None => keys.insert(0, line),
    }
    keys
}

proptest! {
    #![proptest_config(gen::config())]

    /// RECON-NOLOSS: identical texts are `same`; a clean merge with a base is `merged`; otherwise the newest wins
    /// (a tie goes to the drive, spec "Conflicts" step 3) and the loser is kept as the copy: same body bytes, same
    /// other front-matter keys and byte order mark, the fresh `newId` as its id, at a path in the same folder that
    /// is free under simple case folding (the first free `n`, from 2).
    #[test]
    fn recon_noloss_loser_is_kept(input in reconcile_input()) {
        let out = reconcile(&input);
        if input.local == input.remote {
            prop_assert_eq!(out, ReconcileOutcome::Same);
            return Ok(());
        }
        if let Some(base) = &input.base {
            let m = merge3(Some(base), &input.local, &input.remote);
            if !m.has_conflicts {
                prop_assert_eq!(out, ReconcileOutcome::Merged { text: m.text });
                return Ok(());
            }
        }
        let ReconcileOutcome::NewestWins { winner, copy_path, copy_text } = out else {
            return Err(TestCaseError::fail(format!("expected newestWins, got {out:?}")));
        };
        let expected = if input.local_modified_millis > input.remote_entry.modified_millis {
            Side::Local
        } else {
            Side::Remote
        };
        prop_assert_eq!(winner, expected);
        let loser = if winner == Side::Local { &input.remote } else { &input.local };
        prop_assert_eq!(body(&copy_text), body(loser));
        prop_assert_eq!(copy_text.starts_with(BOM), loser.starts_with(BOM));
        prop_assert_eq!(header_keys(&copy_text), Some(keys_with_id(header_keys(loser), &input.new_id)));
        prop_assert_eq!(front_matter::read_id(&copy_text), Some(input.new_id.clone()));

        prop_assert_eq!(paths::parent(&copy_path), paths::parent(&input.path));
        let taken: HashSet<String> = input.existing_paths.iter().map(|p| paths::fold_case(p)).collect();
        prop_assert!(!taken.contains(&paths::fold_case(&copy_path)));
        let first_free = (1..).map(|n| gen::conflict_copy_candidate(&input.path, n))
            .find(|c| !taken.contains(&paths::fold_case(c)));
        prop_assert_eq!(Some(copy_path.clone()), first_free);
        prop_assert!(paths::is_syncable_path(&copy_path) && paths::is_note(&copy_path), "{copy_path:?}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------------------------------------------

/// A JSON value keeping the order of object keys.
#[derive(Debug)]
enum Ordered {
    Leaf,
    Object(Vec<(String, Ordered)>),
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_bool<E>(self, _: bool) -> Result<Ordered, E> {
                Ok(Ordered::Leaf)
            }
            fn visit_i64<E>(self, _: i64) -> Result<Ordered, E> {
                Ok(Ordered::Leaf)
            }
            fn visit_u64<E>(self, _: u64) -> Result<Ordered, E> {
                Ok(Ordered::Leaf)
            }
            fn visit_f64<E>(self, _: f64) -> Result<Ordered, E> {
                Ok(Ordered::Leaf)
            }
            fn visit_str<E>(self, _: &str) -> Result<Ordered, E> {
                Ok(Ordered::Leaf)
            }
            fn visit_unit<E>(self) -> Result<Ordered, E> {
                Ok(Ordered::Leaf)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
                while seq.next_element::<Ordered>()?.is_some() {}
                Ok(Ordered::Leaf)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut v = Vec::new();
                while let Some((k, x)) = map.next_entry::<String, Ordered>()? {
                    v.push((k, x));
                }
                Ok(Ordered::Object(v))
            }
        }
        d.deserialize_any(V)
    }
}

fn keys(o: &Ordered) -> Vec<&str> {
    match o {
        Ordered::Object(v) => v.iter().map(|(k, _)| k.as_str()).collect(),
        Ordered::Leaf => Vec::new(),
    }
}

fn as_millis(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// What `parse_manifest` must return for a manifest-like object, per the spec's "Sync manifest"; `None` when the
/// spec does not say (an `updatedAt` that is present but not a number).
fn expected_parse(v: &Value) -> Option<Option<Manifest>> {
    let o = v.as_object()?;
    let format_ok = o
        .get("format")
        .and_then(Value::as_f64)
        .is_some_and(|f| f <= 1.0);
    let device_id = o.get("deviceId").and_then(Value::as_str);
    let (Some(device_id), true) = (device_id, format_ok) else {
        return Some(None);
    };
    let updated_at = match o.get("updatedAt") {
        None => return Some(None),
        Some(u) => as_millis(u)?,
    };
    let mut files = HashMap::new();
    match o.get("files") {
        None => {}
        Some(Value::Object(m)) => {
            for (p, e) in m {
                let entry = e.as_object().and_then(|e| {
                    Some(ManifestEntry {
                        sha256: e.get("sha256")?.as_str()?.to_string(),
                        version: e.get("version")?.as_str()?.to_string(),
                        synced_at: as_millis(e.get("syncedAt")?)?,
                    })
                });
                match entry {
                    Some(entry) => files.insert(p.clone(), entry),
                    None => return Some(None),
                };
            }
        }
        Some(_) => return Some(None),
    }
    Some(Some(Manifest {
        device_id: device_id.to_string(),
        updated_at,
        files,
    }))
}

/// A previous manifest for `records`: absent, any text, or the records' manifest with a change that does or does
/// not matter (`syncedAt`, `deviceId`, `updatedAt` do not; a path, hash or version does).
fn previous_manifest(
    records: Vec<SyncRecord>,
) -> impl Strategy<Value = (Vec<SyncRecord>, Option<String>)> {
    let files: Vec<(String, ManifestEntry)> = records
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
    let derived = (
        0u8..8,
        any::<proptest::sample::Index>(),
        gen::valid_path(2),
        any::<i64>(),
    )
        .prop_map(move |(change, ix, extra, n)| {
            let mut files = files.clone();
            let has = !files.is_empty();
            match change {
                1 if has => ix.get_mut(&mut files).1.synced_at = n,
                2 if has => ix.get_mut(&mut files).1.sha256.push('x'),
                3 if has => ix.get_mut(&mut files).1.version.push('x'),
                4 if has => {
                    files.remove(ix.index(files.len()));
                }
                5 => files.push((
                    extra,
                    ManifestEntry {
                        sha256: "h".into(),
                        version: "v".into(),
                        synced_at: 0,
                    },
                )),
                _ => {}
            }
            let m = Manifest {
                device_id: format!("device {n}"),
                updated_at: n,
                files: files.into_iter().collect(),
            };
            let mut text = serialize_manifest(&m);
            if change == 6 {
                text = text.replace("\"format\":1", "\"format\":2");
            }
            Some(text)
        });
    (
        Just(records),
        prop_oneof![1 => Just(None), 1 => gen::json_text().prop_map(Some), 4 => derived],
    )
}

proptest! {
    #![proptest_config(gen::config())]

    /// MAN-ROUNDTRIP: `parse(serialize(m)) == m` for arbitrary Unicode and escaped paths, strings and `i64`
    /// timestamps, and serializing again gives the same text.
    #[test]
    fn man_roundtrip(m in gen::manifest()) {
        let text = serialize_manifest(&m);
        let parsed = parse_manifest(&text);
        prop_assert_eq!(parsed.as_ref(), Some(&m), "{}", text);
        prop_assert_eq!(serialize_manifest(&parsed.unwrap()), text);
    }

    /// MAN-CANON: compact JSON, keys `format, app, deviceId, updatedAt, files`, `format` 1 and `app` `MarkNotes`,
    /// `files` sorted by path in UTF-16 code-unit order, entry keys `sha256, version, syncedAt`.
    #[test]
    fn man_canon_sorted_compact(m in gen::manifest()) {
        let text = serialize_manifest(&m);
        let ordered: Ordered = serde_json::from_str(&text).expect("valid JSON");
        prop_assert_eq!(keys(&ordered), vec!["format", "app", "deviceId", "updatedAt", "files"]);
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        prop_assert_eq!(&value["format"], &serde_json::json!(1));
        prop_assert_eq!(&value["app"], &serde_json::json!("MarkNotes"));
        // Same length as serde_json's compact output: no whitespace outside strings.
        prop_assert_eq!(serde_json::to_string(&value).unwrap().len(), text.len());
        let Ordered::Object(top) = ordered else { unreachable!() };
        let files = &top[4].1;
        let paths_in_order = keys(files);
        prop_assert_eq!(paths_in_order.len(), m.files.len());
        for w in paths_in_order.windows(2) {
            prop_assert_eq!(paths::compare(w[0], w[1]), Ordering::Less, "{:?}", paths_in_order);
        }
        if let Ordered::Object(entries) = files {
            for (_, e) in entries {
                prop_assert_eq!(keys(e), vec!["sha256", "version", "syncedAt"]);
            }
        }
    }

    /// MAN-REJECT: `parse_manifest` never panics on arbitrary text, and on manifest-like objects it accepts and
    /// rejects exactly as the spec says (format a number up to 1, string `deviceId`, `updatedAt` present, `files`
    /// missing or an object of entries with string `sha256` and `version` and numeric `syncedAt`, unknown keys
    /// ignored).
    #[test]
    fn man_reject_never_panics(text in gen::json_text(), v in gen::manifest_like_json()) {
        let _ = parse_manifest(&text);
        let got = parse_manifest(&v.to_string());
        if let Some(expected) = expected_parse(&v) {
            prop_assert_eq!(got, expected, "{}", v);
        }
    }

    /// MAN-SKIP: `build_manifest` returns `None` iff the previous manifest parses and lists the same files (paths,
    /// `sha256` and `version`, ignoring `syncedAt`) as the file records; otherwise it returns the manifest of the
    /// file records (folders left out) with the given device id and time.
    #[test]
    fn man_skip_only_when_files_unchanged(
        (records, previous) in gen::records(8).prop_flat_map(previous_manifest),
        now in any::<i64>(),
    ) {
        let files: HashMap<String, ManifestEntry> = records
            .iter()
            .filter(|r| !r.is_folder)
            .map(|r| {
                (r.path.clone(), ManifestEntry {
                    sha256: r.local_hash.clone(),
                    version: r.remote_version.clone(),
                    synced_at: r.synced_at,
                })
            })
            .collect();
        let same = previous.as_deref().and_then(parse_manifest).is_some_and(|p| {
            p.files.len() == files.len()
                && files.iter().all(|(k, e)| {
                    p.files.get(k).is_some_and(|o| o.sha256 == e.sha256 && o.version == e.version)
                })
        });
        let built = build_manifest(&records, "this device", now, previous.as_deref());
        prop_assert_eq!(built.is_none(), same, "{:?}", previous);
        if let Some(text) = built {
            let expected = Manifest { device_id: "this device".into(), updated_at: now, files };
            prop_assert_eq!(parse_manifest(&text), Some(expected));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Front matter, paths, scheduler
// ---------------------------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(gen::config())]

    /// FM-ID: `read_id(with_id(t, id)) == id` for a valid id (and no id for an invalid one); the body, the other
    /// keys (first `id:` line replaced, else `id` first) and the byte order mark are kept; apart from its `id:`
    /// line the block is byte for byte the original one (fences, key lines, every line ending: "kept verbatim");
    /// `with_id` is idempotent.
    #[test]
    fn fm_id_round_trip(t in gen::note_text(), id in gen::valid_id(), bad in gen::invalid_id()) {
        let out = front_matter::with_id(&t, &id);
        prop_assert_eq!(front_matter::read_id(&out), Some(id.clone()));
        prop_assert_eq!(body(&out), body(&t));
        prop_assert_eq!(out.starts_with(BOM), t.starts_with(BOM));
        prop_assert_eq!(header_keys(&out), Some(keys_with_id(header_keys(&t), &id)));
        prop_assert_eq!(&front_matter::with_id(&out, &id), &out);
        let (before, _) = front_matter::split(&t);
        if !before.is_empty() {
            let (after, _) = front_matter::split(&out);
            prop_assert_eq!(without_first_id_line(after), without_first_id_line(before));
        }
        prop_assert_eq!(front_matter::read_id(&front_matter::with_id(&t, &bad)), None);
    }

    /// PATH-NORM: `normalize` is idempotent and yields a syncable path (or the root); a syncable path is its own
    /// normal form; `fold_case` is idempotent.
    #[test]
    fn path_norm_idempotent(p in gen::any_path()) {
        if let Some(n) = paths::normalize(&p) {
            let again = paths::normalize(&n);
            prop_assert_eq!(again.as_deref(), Some(n.as_str()));
            prop_assert!(n.is_empty() || paths::is_syncable_path(&n), "{n:?}");
        }
        if paths::is_syncable_path(&p) {
            prop_assert_eq!(paths::normalize(&p), Some(p.clone()));
        }
        let folded = paths::fold_case(&p);
        prop_assert_eq!(paths::fold_case(&folded), folded.clone());
    }

    /// PATH-COPY: `conflict_copy_name` returns the first free candidate (`(conflicted copy)`, then ` 2`, ` 3`, ...)
    /// in the same folder, free under simple case folding, and syncable when the note path is.
    #[test]
    fn path_copy_is_free_in_same_folder(
        (path, existing) in prop_oneof![
            3 => gen::note_path(3),
            1 => gen::valid_path(3),
            1 => Just("README".to_string()),
            1 => gen::any_path(),
        ].prop_flat_map(|p| (Just(p.clone()), gen::existing_paths_near(p)))
    ) {
        let copy = paths::conflict_copy_name(&path, &existing);
        prop_assert_eq!(paths::parent(&copy), paths::parent(&path));
        let taken: HashSet<String> = existing.iter().map(|p| paths::fold_case(p)).collect();
        prop_assert!(!taken.contains(&paths::fold_case(&copy)));
        let first_free = (1..).map(|n| gen::conflict_copy_candidate(&path, n))
            .find(|c| !taken.contains(&paths::fold_case(c)));
        prop_assert_eq!(Some(copy.clone()), first_free);
        if paths::is_syncable_path(&path) {
            prop_assert!(paths::is_syncable_path(&copy), "{copy:?}");
        }
    }

    /// SCHED-TERM: the scheduler terminates (at most drives × `maxPasses` syncs), returns only known drives, syncs
    /// each drive at most `maxPasses` times and at least once when `maxPasses >= 1`, whatever the reports.
    #[test]
    fn sched_term_bounded_passes(ids in gen::drive_ids(), max_passes in 0u32..5, reports in gen::drive_reports()) {
        let mut s = Scheduler::new(ids.clone(), max_passes);
        let bound = ids.len() * max_passes as usize;
        let mut passes: HashMap<String, u32> = HashMap::new();
        let mut steps = 0;
        while let Some(id) = s.next() {
            steps += 1;
            prop_assert!(steps <= bound, "more than {bound} syncs");
            prop_assert!(ids.contains(&id));
            *passes.entry(id.clone()).or_default() += 1;
            let (changes, failed) = reports[(steps - 1) % reports.len()];
            s.report(&id, changes, failed);
        }
        prop_assert_eq!(s.next(), None);
        for id in &ids {
            let n = passes.get(id).copied().unwrap_or(0);
            prop_assert!(n <= max_passes);
            if max_passes >= 1 {
                prop_assert!(n >= 1, "{id} never synced");
            }
        }
    }
}

/// PATH-NORM, exhaustively for single characters: `fold_case` is idempotent on every Unicode scalar value.
#[test]
fn path_norm_fold_case_idempotent_on_every_char() {
    for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
        let once = paths::fold_case(&c.to_string());
        assert_eq!(paths::fold_case(&once), once, "U+{:04X}", c as u32);
    }
}

/// The front-matter block (`split(..).0`) without its first `id:` key line, line ending included.
fn without_first_id_line(header: &str) -> String {
    let lines: Vec<&str> = header.split_inclusive('\n').collect();
    let id_line = (1..lines.len().saturating_sub(1)).find(|&i| {
        let l = lines[i].strip_suffix('\n').unwrap_or(lines[i]);
        l.strip_suffix('\r').unwrap_or(l).starts_with("id:")
    });
    lines
        .iter()
        .enumerate()
        .filter(|&(i, _)| Some(i) != id_line)
        .map(|(_, l)| *l)
        .collect()
}
