//! Unit tests for `merge.rs` (`merge3`, `merge_text`) and `reconcile.rs`. Each test is named after the rule it checks
//! and cites the section of `spec/note-format.md` it comes from. Tests marked "Characterisation" pin behaviour the
//! spec leaves open, so that any change to it is deliberate.

use std::collections::HashMap;

use marknotes_sync::merge::{MARKER_LOCAL, MARKER_REMOTE, MARKER_SEPARATOR};
use marknotes_sync::*;

/// `MAX_EDIT_DISTANCE` in merge.rs (private): above this many inserted plus deleted lines between two texts, the
/// diff gives up and treats them as having no line in common.
const MAX_EDIT_DISTANCE: usize = 2000;

const NEW_ID: &str = "m4n5p6q7";

fn choices(pairs: &[(i32, ConflictChoice)]) -> HashMap<i32, ConflictChoice> {
    pairs.iter().copied().collect()
}

fn numbered(prefix: &str, range: std::ops::Range<usize>) -> String {
    range.map(|i| format!("{prefix}{i}\n")).collect()
}

fn conflict_block(local: &str, remote: &str) -> String {
    format!("{MARKER_LOCAL}\n{local}{MARKER_SEPARATOR}\n{remote}{MARKER_REMOTE}\n")
}

fn assert_merge(base: Option<&str>, local: &str, remote: &str, text: &str, conflicts: i32) {
    let m = merge3(base, local, remote);
    assert_eq!(
        (m.text.as_str(), m.conflicts),
        (text, conflicts),
        "base {base:?}, local {local:?}, remote {remote:?}"
    );
    assert_eq!(m.has_conflicts, conflicts > 0);
}

/// Resolving every conflict the same way.
fn resolve_all(m: &MergeResult, choice: ConflictChoice) -> String {
    let all = (0..m.conflicts).map(|i| (i, choice)).collect();
    merge_text(&m.chunks, &all)
}

// ---------------------------------------------------------------------------------------------------------------
// merge3: shapes of input
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "Conflicts": two identical texts merge to themselves, whatever the base, even empty.
#[test]
fn identical_texts_merge_to_themselves() {
    for base in [None, Some(""), Some("x\n")] {
        assert_merge(base, "", "", "", 0);
        assert_merge(base, "a", "a", "a", 0);
        assert_merge(base, "a\r\nb", "a\r\nb", "a\r\nb", 0);
    }
    assert!(merge3(Some("x"), "", "").chunks.is_empty());
}

/// note-format.md, "Sync" > "Conflicts": a change on one side only merges cleanly, also from or to an empty text.
#[test]
fn one_sided_change_from_or_to_an_empty_text_is_clean() {
    assert_merge(Some(""), "a\n", "", "a\n", 0);
    assert_merge(Some(""), "", "a\n", "a\n", 0);
    assert_merge(Some("a\nb\n"), "", "a\nb\n", "", 0);
    assert_merge(Some("a\nb\n"), "a\nb\n", "", "", 0);
}

/// note-format.md, "Sync" > "Conflicts": both sides adding different text to an empty base overlap.
#[test]
fn different_additions_to_an_empty_base_conflict() {
    assert_merge(Some(""), "x\n", "y\n", &conflict_block("x\n", "y\n"), 1);
}

/// merge/merge3.json "no base, choice local": without a base every difference is a conflict, even against an
/// empty text.
#[test]
fn without_a_base_an_empty_side_is_a_conflict() {
    let m = merge3(None, "", "a\n");
    assert_eq!(m.conflicts, 1);
    assert_eq!(m.text, conflict_block("", "a\n"));
    assert_eq!(resolve_all(&m, ConflictChoice::Local), "");
    assert_eq!(resolve_all(&m, ConflictChoice::Remote), "a\n");
}

/// note-format.md, "Sync" > "Conflicts" (line-based merge): a last line without a line break is a line like any
/// other.
#[test]
fn texts_without_a_trailing_newline_merge_line_by_line() {
    assert_merge(Some("a\nb\nc"), "A\nb\nc", "a\nb\nC", "A\nb\nC", 0);
    assert_merge(Some("a\nx\nb"), "a\nx\nb\n", "A\nx\nb", "A\nx\nb\n", 0);
}

/// note-format.md, "Sync" > "Conflicts": unresolved regions are written with the marker lines, which start on their
/// own line even when a side's last line has no line break.
#[test]
fn markers_start_on_their_own_line_after_a_side_without_trailing_newline() {
    assert_merge(
        Some("a\nb\nc"),
        "a\nb\nL",
        "a\nb\nR",
        &format!("a\nb\n{}", conflict_block("L\n", "R\n")),
        1,
    );
}

/// note-format.md, "Sync" > "Conflicts" (line-based merge): CRLF lines merge like LF lines, keeping their endings.
#[test]
fn crlf_texts_merge_line_by_line() {
    assert_merge(
        Some("a\r\nb\r\nc\r\n"),
        "A\r\nb\r\nc\r\n",
        "a\r\nb\r\nC\r\n",
        "A\r\nb\r\nC\r\n",
        0,
    );
}

/// Characterisation (the spec gives the markers but not their line endings): markers always end with `\n`, also in
/// a CRLF text; the conflicting lines keep their own endings.
#[test]
fn markers_use_lf_in_crlf_texts() {
    assert_merge(
        Some("a\r\nb\r\nc\r\n"),
        "a\r\nL\r\nc\r\n",
        "a\r\nR\r\nc\r\n",
        &format!("a\r\n{}c\r\n", conflict_block("L\r\n", "R\r\n")),
        1,
    );
}

/// note-format.md, "Sync" > "Conflicts" (line-based merge): changing a line ending changes the line.
#[test]
fn a_line_ending_change_is_an_edit() {
    assert_merge(
        Some("a\nb\nc\n"),
        "a\r\nb\nc\n",
        "a\nb\nC\n",
        "a\r\nb\nC\n",
        0,
    );
}

/// Characterisation (the spec says "line-based three-way merge" without defining overlap): edits to adjacent lines,
/// with no unchanged line between them, are one conflicting region (diff3 behaviour).
#[test]
fn adjacent_edits_conflict() {
    let m = merge3(Some("a\nb\n"), "A\nb\n", "a\nB\n");
    assert_eq!(m.conflicts, 1);
    assert_eq!(
        m.chunks,
        vec![MergeChunk::Conflict {
            base: vec!["a\n".into(), "b\n".into()],
            local: vec!["A\n".into(), "b\n".into()],
            remote: vec!["a\n".into(), "B\n".into()],
        }]
    );
}

/// note-format.md, "Sync" > "Conflicts": an insertion at the start on one side and at the end on the other merge.
#[test]
fn inserts_at_the_start_and_at_the_end_merge() {
    assert_merge(Some("m\n"), "s\nm\n", "m\ne\n", "s\nm\ne\n", 0);
    assert_merge(Some("m\n"), "m\ne\n", "s\nm\n", "s\nm\ne\n", 0);
}

/// note-format.md, "Sync" > "Conflicts": different insertions at the same place overlap, at the start or at the end.
#[test]
fn different_inserts_at_the_same_place_conflict() {
    assert_merge(
        Some("m\n"),
        "x\nm\n",
        "y\nm\n",
        &format!("{}m\n", conflict_block("x\n", "y\n")),
        1,
    );
    assert_merge(
        Some("m\n"),
        "m\nx\n",
        "m\ny\n",
        &format!("m\n{}", conflict_block("x\n", "y\n")),
        1,
    );
}

/// note-format.md, "Sync" > "Conflicts": the same insertion on both sides is kept once.
#[test]
fn identical_inserts_are_kept_once() {
    assert_merge(
        Some("a\nb\nc\n"),
        "A\nb\nc\nz\n",
        "a\nb\nc\nz\n",
        "A\nb\nc\nz\n",
        0,
    );
}

/// note-format.md, "Sync" > "Conflicts": both sides deleting the same lines is not a conflict.
#[test]
fn both_deleting_the_same_lines_is_clean() {
    assert_merge(
        Some("a\nb\nc\nd\ne\n"),
        "a\nc\nd\nE\n",
        "a\nc\nd\ne\n",
        "a\nc\nd\nE\n",
        0,
    );
}

/// note-format.md, "Sync" > "Conflicts": both sides making the same change is not a conflict, and the region is
/// reported as changed.
#[test]
fn both_making_the_same_change_is_clean() {
    let m = merge3(
        Some("a\nb\nc\nd\ne\n"),
        "a\nB\nc\nd\nE\n",
        "a\nB\nc\nd\ne\n",
    );
    assert_eq!((m.text.as_str(), m.conflicts), ("a\nB\nc\nd\nE\n", 0));
    assert!(m.chunks.iter().any(|c| matches!(
        c,
        MergeChunk::Resolved { lines, changed: true } if lines == &vec!["B\n".to_string()]
    )));
}

/// note-format.md, "Sync" > "Conflicts": deleting a line on one side while editing it on the other is a conflict
/// whose local side is empty.
#[test]
fn deleting_a_line_edited_on_the_other_side_conflicts() {
    let m = merge3(Some("a\nb\nc\n"), "a\nc\n", "a\nB\nc\n");
    assert_eq!(m.conflicts, 1);
    assert_eq!(m.text, format!("a\n{}c\n", conflict_block("", "B\n")));
    assert_eq!(
        merge_text(&m.chunks, &choices(&[(0, ConflictChoice::Both)])),
        "a\nB\nc\n"
    );
}

/// note-format.md, "Sync" > "Conflicts": a long text with a few edits far apart merges cleanly.
#[test]
fn long_text_with_few_edits_merges_cleanly() {
    let base: Vec<String> = (0..20_000).map(|i| format!("line {i}\n")).collect();
    let mut local = base.clone();
    local[100] = "LOCAL\n".into();
    let mut remote = base.clone();
    remote[15_000] = "REMOTE\n".into();
    remote.push("tail\n".into());
    let mut expected = local.clone();
    expected[15_000] = "REMOTE\n".into();
    expected.push("tail\n".into());
    assert_merge(
        Some(&base.concat()),
        &local.concat(),
        &remote.concat(),
        &expected.concat(),
        0,
    );
}

// ---------------------------------------------------------------------------------------------------------------
// merge3: the edit-distance bound (D10 pins the far side of it; these tests pin the cliff itself)
// ---------------------------------------------------------------------------------------------------------------

/// Two texts without a base whose only common line is `common`, with `before` lines on the drive's side before it.
/// The edit distance between them is 1000 + `before` + 500 (+ 500 local lines before, 500 after on each side).
fn no_base_pair(remote_before: usize) -> (String, String) {
    let local = format!(
        "{}common\n{}",
        numbered("l", 0..500),
        numbered("l", 500..1000)
    );
    let remote = format!(
        "{}common\n{}",
        numbered("r", 0..remote_before),
        numbered("r", 1000..1500)
    );
    (local, remote)
}

/// Characterisation of the edit-distance bound (the spec has none): at exactly `MAX_EDIT_DISTANCE` inserted plus
/// deleted lines, a line common to both texts is still found, so it splits the conflict in two.
#[test]
fn merge_at_the_edit_distance_bound_still_finds_common_lines() {
    let (local, remote) = no_base_pair(500);
    assert_eq!(500 + 500 + 500 + 500, MAX_EDIT_DISTANCE);
    let m = merge3(None, &local, &remote);
    assert_eq!(m.conflicts, 2);
    assert!(m.chunks.contains(&MergeChunk::Resolved {
        lines: vec!["common\n".into()],
        changed: false
    }));
    // Nothing is lost: resolving all regions one way gives back that side.
    assert_eq!(resolve_all(&m, ConflictChoice::Local), local);
    assert_eq!(resolve_all(&m, ConflictChoice::Remote), remote);
}

/// Characterisation of the edit-distance bound: one line over `MAX_EDIT_DISTANCE`, the texts are treated as having
/// nothing in common. Nothing is lost (note-format.md, "Sync" > "Conflicts": "the losing version is never lost"):
/// the single conflict holds both full texts.
#[test]
fn merge_one_over_the_edit_distance_bound_keeps_both_full_sides() {
    let (local, remote) = no_base_pair(501);
    let m = merge3(None, &local, &remote);
    assert_eq!(m.conflicts, 1);
    match &m.chunks[..] {
        [MergeChunk::Conflict {
            base,
            local: l,
            remote: r,
        }] => {
            assert!(base.is_empty());
            assert_eq!(l.concat(), local);
            assert_eq!(r.concat(), remote);
        }
        other => panic!("expected a single conflict, got {} chunks", other.len()),
    }
    assert_eq!(resolve_all(&m, ConflictChoice::Local), local);
    assert_eq!(resolve_all(&m, ConflictChoice::Remote), remote);
}

/// A base and a local text that differ by `500 + 500 + local_before + 500` lines in their middle, and a drive text
/// that only changes `x`, a line the local text keeps.
fn based_triple(local_before: usize) -> (String, String, String) {
    let middle = |x: &str| format!("s1\n{x}\ns2\n");
    let base = format!(
        "top\nsep\n{}{}{}end\n",
        numbered("b", 0..500),
        middle("x"),
        numbered("c", 0..500)
    );
    let local = format!(
        "top\nsep\n{}{}{}end\n",
        numbered("l", 0..local_before),
        middle("x"),
        numbered("m", 0..500)
    );
    let remote = format!(
        "top\nsep\n{}{}{}end\n",
        numbered("b", 0..500),
        middle("X"),
        numbered("c", 0..500)
    );
    (base, local, remote)
}

/// Characterisation of the edit-distance bound with a base: at exactly `MAX_EDIT_DISTANCE` between the base and the
/// local text, edits separated by unchanged lines merge cleanly.
#[test]
fn based_merge_at_the_edit_distance_bound_is_clean() {
    let (base, local, remote) = based_triple(500);
    let expected = format!(
        "top\nsep\n{}s1\nX\ns2\n{}end\n",
        numbered("l", 0..500),
        numbered("m", 0..500)
    );
    assert_merge(Some(&base), &local, &remote, &expected, 0);
}

/// Characterisation of the edit-distance bound with a base: one line over it, the base and the local text share
/// only their common prefix and suffix, so the drive's edit inside that span becomes one conflict. It holds both
/// full middles, so either choice gives back a whole side.
#[test]
fn based_merge_one_over_the_edit_distance_bound_conflicts_without_loss() {
    let (base, local, remote) = based_triple(501);
    let m = merge3(Some(&base), &local, &remote);
    assert_eq!(m.conflicts, 1);
    let inner = |s: &str| {
        s.strip_prefix("top\nsep\n")
            .and_then(|s| s.strip_suffix("end\n"))
            .unwrap()
            .to_string()
    };
    let conflict = m
        .chunks
        .iter()
        .find_map(|c| match c {
            MergeChunk::Conflict {
                base: b,
                local: l,
                remote: r,
            } => Some((b.concat(), l.concat(), r.concat())),
            _ => None,
        })
        .unwrap();
    assert_eq!(conflict, (inner(&base), inner(&local), inner(&remote)));
    assert_eq!(resolve_all(&m, ConflictChoice::Local), local);
    assert_eq!(resolve_all(&m, ConflictChoice::Remote), remote);
}

// ---------------------------------------------------------------------------------------------------------------
// merge_text: choices
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "Conflicts": each region is resolved as local, remote, both (local then remote) or
/// neither; a region without a choice is written with markers. Every combination over two regions.
#[test]
fn every_choice_combination_resolves_each_region_independently() {
    let m = merge3(
        Some("a\nb\nc\nd\ne\n"),
        "L1\nb\nc\nd\nL2\n",
        "R1\nb\nc\nd\nR2\n",
    );
    assert_eq!(m.conflicts, 2);
    let piece = |choice: Option<ConflictChoice>, l: &str, r: &str| match choice {
        None => conflict_block(l, r),
        Some(ConflictChoice::Local) => l.to_string(),
        Some(ConflictChoice::Remote) => r.to_string(),
        Some(ConflictChoice::Both) => format!("{l}{r}"),
        Some(ConflictChoice::Neither) => String::new(),
    };
    let options = [
        None,
        Some(ConflictChoice::Local),
        Some(ConflictChoice::Remote),
        Some(ConflictChoice::Both),
        Some(ConflictChoice::Neither),
    ];
    for c0 in options {
        for c1 in options {
            let mut map = HashMap::new();
            if let Some(c) = c0 {
                map.insert(0, c);
            }
            if let Some(c) = c1 {
                map.insert(1, c);
            }
            let expected = format!(
                "{}b\nc\nd\n{}",
                piece(c0, "L1\n", "R1\n"),
                piece(c1, "L2\n", "R2\n")
            );
            assert_eq!(merge_text(&m.chunks, &map), expected, "{c0:?} {c1:?}");
        }
    }
    assert_eq!(merge_text(&m.chunks, &HashMap::new()), m.text);
}

/// Characterisation (the spec indexes regions from 0 but says nothing of other indices): choices for indices that
/// name no region (negative or past the last one) are ignored.
#[test]
fn choices_for_indices_without_a_region_are_ignored() {
    let m = merge3(Some("a\nb\nc\n"), "a\nl\nc\n", "a\nr\nc\n");
    let stray = choices(&[
        (-1, ConflictChoice::Local),
        (1, ConflictChoice::Remote),
        (i32::MAX, ConflictChoice::Both),
        (i32::MIN, ConflictChoice::Neither),
    ]);
    assert_eq!(merge_text(&m.chunks, &stray), m.text);
    let clean = merge3(Some("a\nb\nc\n"), "A\nb\nc\n", "a\nb\nC\n");
    assert_eq!(
        merge_text(&clean.chunks, &choices(&[(0, ConflictChoice::Neither)])),
        "A\nb\nC\n"
    );
}

/// note-format.md, "Sync" > "Conflicts": "both" is local then remote, line by line, so a local side without a final
/// line break gets one before the remote lines.
#[test]
fn both_puts_the_remote_lines_on_their_own_lines() {
    let m = merge3(Some("x"), "l", "r");
    assert_eq!(
        merge_text(&m.chunks, &choices(&[(0, ConflictChoice::Both)])),
        "l\nr"
    );
    assert_eq!(
        merge_text(&m.chunks, &choices(&[(0, ConflictChoice::Local)])),
        "l"
    );
    assert_eq!(
        merge_text(&m.chunks, &choices(&[(0, ConflictChoice::Neither)])),
        ""
    );
}

// ---------------------------------------------------------------------------------------------------------------
// merge3: user text that looks like markers
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "Conflicts": conflicts are regions of the merge, not marker lines in the text, so
/// marker-like lines the user wrote don't count as conflicts and are kept verbatim.
#[test]
fn marker_lines_in_user_text_are_not_conflicts() {
    let base = format!("{MARKER_LOCAL}\na\n{MARKER_SEPARATOR}\nb\n{MARKER_REMOTE}\n");
    let local = format!("{MARKER_LOCAL}\nA\n{MARKER_SEPARATOR}\nb\n{MARKER_REMOTE}\n");
    let remote = format!("{MARKER_LOCAL}\na\n{MARKER_SEPARATOR}\nB\n{MARKER_REMOTE}\n");
    let m = merge3(Some(&base), &local, &remote);
    assert_eq!(m.conflicts, 0);
    assert_eq!(
        m.text,
        format!("{MARKER_LOCAL}\nA\n{MARKER_SEPARATOR}\nB\n{MARKER_REMOTE}\n")
    );
}

/// note-format.md, "Sync" > "Conflicts": a conflicting region whose lines look like markers is still one region,
/// and choosing a side gives exactly that side's lines.
#[test]
fn marker_lines_inside_a_conflict_are_plain_lines() {
    let local = format!("a\n{MARKER_SEPARATOR}\nc\n");
    let remote = format!("a\n{MARKER_REMOTE}\nc\n");
    let m = merge3(Some("a\nb\nc\n"), &local, &remote);
    assert_eq!(m.conflicts, 1);
    assert_eq!(resolve_all(&m, ConflictChoice::Local), local);
    assert_eq!(resolve_all(&m, ConflictChoice::Remote), remote);
}

// ---------------------------------------------------------------------------------------------------------------
// reconcile
// ---------------------------------------------------------------------------------------------------------------

fn reconcile_input(
    local: &str,
    local_millis: i64,
    remote: &str,
    remote_millis: i64,
    base: Option<&str>,
) -> ReconcileInput {
    ReconcileInput {
        path: "Work/Plan.md".into(),
        local: local.into(),
        local_modified_millis: local_millis,
        remote: remote.into(),
        remote_entry: RemoteEntry {
            path: "Work/Plan.md".into(),
            id: "r1".into(),
            is_folder: false,
            version: "v2".into(),
            modified_millis: remote_millis,
        },
        base: base.map(str::to_string),
        existing_paths: vec!["Work/Plan.md".into()],
        new_id: NEW_ID.into(),
    }
}

fn winner(outcome: &ReconcileOutcome) -> Side {
    match outcome {
        ReconcileOutcome::NewestWins { winner, .. } => *winner,
        other => panic!("expected newestWins, got {other:?}"),
    }
}

/// note-format.md, "Sync" > "Conflicts", step 1: the same text on both sides is recorded, whatever the base and the
/// modification times.
#[test]
fn same_text_is_recorded_whatever_the_base_and_times() {
    for base in [None, Some(""), Some("other\n")] {
        for (lm, rm) in [(0, 0), (5, 1), (1, 5)] {
            assert_eq!(
                reconcile(&reconcile_input("a\n", lm, "a\n", rm, base)),
                ReconcileOutcome::Same
            );
        }
    }
    assert_eq!(
        reconcile(&reconcile_input("", 1, "", 2, None)),
        ReconcileOutcome::Same
    );
}

/// note-format.md, "Sync" > "Conflicts": texts that differ only in line endings are not the same text: merged when
/// only one side changed from the base, otherwise the newest wins.
#[test]
fn line_ending_differences_are_not_the_same_text() {
    assert_eq!(
        reconcile(&reconcile_input("a\r\n", 1, "a\n", 2, Some("a\n"))),
        ReconcileOutcome::Merged {
            text: "a\r\n".into()
        }
    );
    assert_eq!(
        winner(&reconcile(&reconcile_input("a\r\n", 1, "a\n", 2, None))),
        Side::Remote
    );
}

/// note-format.md, "Sync" > "Conflicts", step 2: a clean merge is written, even when one side equals the base.
#[test]
fn clean_merge_with_one_side_unchanged_takes_the_other() {
    assert_eq!(
        reconcile(&reconcile_input("a\n", 9, "b\n", 1, Some("a\n"))),
        ReconcileOutcome::Merged { text: "b\n".into() }
    );
}

/// note-format.md, "Sync" > "Conflicts", step 3: an empty base copy is a base, so different additions conflict and
/// the newest wins.
#[test]
fn different_additions_to_an_empty_base_go_to_the_newest() {
    assert_eq!(
        reconcile(&reconcile_input("x\n", 2, "y\n", 1, Some(""))),
        ReconcileOutcome::NewestWins {
            winner: Side::Local,
            copy_path: "Work/Plan (conflicted copy).md".into(),
            copy_text: "---\nid: m4n5p6q7\n---\ny\n".into(),
        }
    );
}

/// note-format.md, "Sync" > "Conflicts", step 3: the newest version wins by modification time; on a tie the drive
/// wins. Extreme and negative times included.
#[test]
fn newest_wins_by_modification_time_and_ties_go_to_the_drive() {
    for (lm, rm, expected) in [
        (2, 1, Side::Local),
        (1, 2, Side::Remote),
        (5, 5, Side::Remote),
        (0, 0, Side::Remote),
        (-1, -2, Side::Local),
        (0, -1, Side::Local),
        (i64::MAX, i64::MAX, Side::Remote),
        (i64::MIN, i64::MIN, Side::Remote),
        (i64::MIN, i64::MAX, Side::Remote),
        (i64::MAX, i64::MIN, Side::Local),
    ] {
        let out = reconcile(&reconcile_input("l\n", lm, "r\n", rm, Some("b\n")));
        assert_eq!(winner(&out), expected, "local {lm}, drive {rm}");
    }
}

/// note-format.md, "Sync" > "Conflicts", step 3: the copy holds the losing version with a new valid id set as in
/// "Note ids" (id line replaced, or added as first key, block line endings and byte order mark kept).
#[test]
fn the_copy_is_the_losing_text_with_the_fresh_id() {
    for (loser, expected) in [
        (
            "---\r\nid: k7f3q2xa\r\ntitle: T\r\n---\r\nlocal body\r\n",
            "---\r\nid: m4n5p6q7\r\ntitle: T\r\n---\r\nlocal body\r\n",
        ),
        ("\u{feff}plain\n", "\u{feff}---\nid: m4n5p6q7\n---\nplain\n"),
        ("---\nid: bad!\n---\nx\n", "---\nid: m4n5p6q7\n---\nx\n"),
        (
            "---\ntitle: T\n---\nx",
            "---\nid: m4n5p6q7\ntitle: T\n---\nx",
        ),
        ("", "---\nid: m4n5p6q7\n---\n"),
    ] {
        // The local version is older, so it loses.
        let out = reconcile(&reconcile_input(loser, 1, "different\n", 2, None));
        match out {
            ReconcileOutcome::NewestWins {
                winner, copy_text, ..
            } => {
                assert_eq!(winner, Side::Remote);
                assert_eq!(copy_text, expected, "{loser:?}");
                assert_eq!(front_matter::read_id(&copy_text).as_deref(), Some(NEW_ID));
            }
            other => panic!("{other:?}"),
        }
    }
}

/// note-format.md, "Sync" > "Conflicts", step 3: when the drive's version loses, the copy holds the drive's text.
#[test]
fn the_copy_holds_the_drive_text_when_the_drive_loses() {
    let out = reconcile(&reconcile_input(
        "---\nid: k7f3q2xa\n---\nlocal\n",
        2,
        "---\nid: k7f3q2xa\n---\nremote\n",
        1,
        None,
    ));
    assert_eq!(
        out,
        ReconcileOutcome::NewestWins {
            winner: Side::Local,
            copy_path: "Work/Plan (conflicted copy).md".into(),
            copy_text: "---\nid: m4n5p6q7\n---\nremote\n".into(),
        }
    );
}

/// note-format.md, "Sync" > "Conflicts", step 3: the copy path is next to the note and free under simple case
/// folding against every existing local path.
#[test]
fn the_copy_path_is_free_under_case_folding_in_the_same_folder() {
    let mut input = reconcile_input("l\n", 1, "r\n", 2, None);
    input.existing_paths = vec![
        "Work/Plan.md".into(),
        "work/plan (CONFLICTED COPY).md".into(),
        "WORK/PLAN (conflicted copy) 2.MD".into(),
        "Plan (conflicted copy) 3.md".into(),
        "Other/Plan (conflicted copy) 3.md".into(),
    ];
    match reconcile(&input) {
        ReconcileOutcome::NewestWins { copy_path, .. } => {
            assert_eq!(copy_path, "Work/Plan (conflicted copy) 3.md")
        }
        other => panic!("{other:?}"),
    }
}
