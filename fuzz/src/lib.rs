//! The checks behind each fuzz target, kept here so that `cargo test --manifest-path fuzz/Cargo.toml` can replay them
//! on the spec fixtures, the saved corpus and pseudo-random inputs without libFuzzer (which needs nightly Rust and
//! doesn't link on every platform). Each `check_*` panics when an invariant fails.
use std::collections::HashMap;

use marknotes_sync::front_matter::{read_id, split, with_id};
use marknotes_sync::merge::lines;
use marknotes_sync::paths::{conflict_copy_name, fold_case, is_syncable_path, normalize, parent};
use marknotes_sync::{
    merge3, merge_text, parse_manifest, plan, reconcile, serialize_manifest, ConflictChoice,
    ReconcileInput, ReconcileOutcome, SyncInput, SyncPlan,
};

#[cfg(test)]
mod replay;

/// PLAN-TOTAL and PLAN-DET: any SyncInput that deserializes plans without panicking, the same way twice, and its JSON
/// form round-trips (what the bindings send).
pub fn check_plan(data: &[u8]) {
    let Ok(input) = serde_json::from_slice::<SyncInput>(data) else {
        return;
    };
    let first = plan(&input);
    assert_eq!(first, plan(&input));
    let json = serde_json::to_string(&first).unwrap();
    assert_eq!(serde_json::from_str::<SyncPlan>(&json).unwrap(), first);
}

/// RECON-NOLOSS: reconcile never panics, a merge result is the clean three-way merge, and when the newest wins the
/// losing text's body is kept in the copy.
pub fn check_reconcile(data: &[u8]) {
    let Ok(input) = serde_json::from_slice::<ReconcileInput>(data) else {
        return;
    };
    match reconcile(&input) {
        ReconcileOutcome::Same => assert_eq!(input.local, input.remote),
        ReconcileOutcome::Merged { text } => {
            let m = merge3(input.base.as_deref(), &input.local, &input.remote);
            assert!(!m.has_conflicts);
            assert_eq!(text, m.text);
        }
        ReconcileOutcome::NewestWins { copy_text, .. } => {
            let body = |t: &str| split(t).1.to_string();
            let kept = body(&copy_text);
            assert!(kept == body(&input.local) || kept == body(&input.remote));
        }
    }
}

/// MERGE-PRESERVE and MERGE-RENDER (base, local and remote separated by NUL bytes; two parts mean no base): the merge
/// never panics, its text is the rendering of its chunks, and resolving every conflict with "both" keeps every line
/// either side added.
pub fn check_merge(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let mut parts = text.splitn(3, '\0');
    let (Some(a), Some(b)) = (parts.next(), parts.next()) else {
        return;
    };
    let (base, local, remote) = match parts.next() {
        Some(c) => (Some(a), b, c),
        None => (None, a, b),
    };
    let m = merge3(base, local, remote);
    assert_eq!(m.text, merge_text(&m.chunks, &HashMap::new()));
    let both: HashMap<i32, ConflictChoice> = (0..m.conflicts as i32)
        .map(|i| (i, ConflictChoice::Both))
        .collect();
    let merged = merge_text(&m.chunks, &both);
    // "Both" ends the local side's last line, so lines are compared without their line endings.
    let content = |t: &str| -> Vec<String> {
        lines(t)
            .iter()
            .map(|l| l.trim_end_matches('\n').trim_end_matches('\r').to_string())
            .collect()
    };
    let merged_lines = content(&merged);
    let base_lines = base.map(content).unwrap_or_default();
    for side in [local, remote] {
        for line in content(side) {
            if !base_lines.contains(&line) {
                assert!(merged_lines.contains(&line), "lost {line:?}");
            }
        }
    }
}

/// MAN-REJECT and MAN-ROUNDTRIP: parsing never panics, and a manifest that parses serializes to text that parses back
/// to the same manifest and serializes the same way.
pub fn check_manifest(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(manifest) = parse_manifest(text) {
        let canonical = serialize_manifest(&manifest);
        let again = parse_manifest(&canonical).expect("a serialized manifest parses");
        assert_eq!(again, manifest);
        assert_eq!(serialize_manifest(&again), canonical);
    }
}

/// FM-ID: `with_id` never panics, the id reads back, the body is untouched, and setting the same id again changes
/// nothing.
pub fn check_front_matter(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = read_id(text);
    let out = with_id(text, "k7f3q2xa");
    assert_eq!(read_id(&out).as_deref(), Some("k7f3q2xa"));
    // A leading BOM stays first, before a block with_id adds (MaxDac/marknotes-spec#33).
    let body = |t: &str| split(t).1.trim_start_matches('\u{feff}').to_string();
    assert_eq!(body(&out), body(text));
    assert_eq!(with_id(&out, "k7f3q2xa"), out);
}

/// PATH-NORM and PATH-COPY (a path, then existing paths, one per line): normalization is idempotent, and the
/// conflicted copy name of a syncable path is syncable, free under case folding and in the same folder.
pub fn check_paths(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut lines = text.split('\n');
    let path = lines.next().unwrap_or_default();
    let existing: Vec<String> = lines.take(64).map(str::to_string).collect();
    if let Some(n) = normalize(path) {
        assert_eq!(normalize(&n).as_deref(), Some(n.as_str()));
    }
    if !is_syncable_path(path) {
        return;
    }
    let copy = conflict_copy_name(path, &existing);
    assert_eq!(parent(&copy), parent(path));
    assert!(existing.iter().all(|e| fold_case(e) != fold_case(&copy)));
    assert!(is_syncable_path(&copy), "{copy:?}");
}

/// A fuzz check: panics when an invariant fails on the input.
pub type Check = fn(&[u8]);

/// Every check, by fuzz target name.
pub const TARGETS: [(&str, Check); 6] = [
    ("plan", check_plan),
    ("reconcile", check_reconcile),
    ("merge", check_merge),
    ("manifest", check_manifest),
    ("front_matter", check_front_matter),
    ("paths", check_paths),
];
