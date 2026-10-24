//! End-to-end simulation of marknotes-sync: the real core run by a reference executor of the spec over several
//! devices and drives, with failures, crashes, mid-sync edits and concurrent devices, under an independent
//! no-loss oracle (see `sim/mod.rs` for the state model, the fault points and the oracle's definition).
//!
//! Each `sim_*` test checks one invariant (`SIM-*` IDs, cited by `docs/executor-contract.md`); the `story_*` tests
//! are real-world stories; `gap_*` tests characterise behaviour the spec allows but that loses or duplicates data;
//! `randomized` runs seeded random histories (`MARKNOTES_SIM_SEED`, `MARKNOTES_SIM_RUNS`).

mod sim;

use std::collections::BTreeSet;

use marknotes_sync::{front_matter, hashing::sha256_hex, paths, plan::CASE_MESSAGE, Action};
use sim::device::Node;
use sim::drive::ListingMode;
use sim::exec::{kind, SessionOutcome};
use sim::oracle::{attachment_hash, body_lines, is_note_path};
use sim::rng::Rng;
use sim::world::{Fault, FaultAt, FaultKind, When, EDITS};
use sim::{panics, quiesce, shared_note, sync, sync_all, Edit, World};

// ----- helpers -----

fn line(text: &str, i: usize) -> String {
    body_lines(text)[i].to_string()
}

fn text(w: &World, dev: usize, path: &str) -> String {
    w.devices[dev]
        .text(path)
        .unwrap_or_else(|| panic!("{path} missing on dev{dev}\n{}", w.describe()))
}

fn drive_text(w: &World, r: usize, path: &str) -> Option<String> {
    w.drives[r]
        .entry(path)
        .map(|e| String::from_utf8_lossy(&e.bytes).into_owned())
}

fn copies(w: &World, dev: usize) -> Vec<String> {
    w.devices[dev]
        .durable
        .files
        .keys()
        .filter(|p| p.contains("(conflicted copy)"))
        .cloned()
        .collect()
}

fn errors(outs: &[SessionOutcome]) -> Vec<String> {
    outs.iter().flat_map(|o| o.errors.clone()).collect()
}

fn kinds(outs: &[SessionOutcome]) -> Vec<&'static str> {
    outs.iter()
        .flat_map(|o| o.actions.iter().map(kind))
        .collect()
}

fn finish(w: &mut World) {
    quiesce(w);
    w.assert_converged();
    w.assert_no_violations();
}

fn before_action(dev: usize, kind: &'static str) -> When {
    When::BeforeAction {
        dev,
        kind: Some(kind),
        path: None,
    }
}

// ----- invariants -----

/// SIM-NOLOSS: across conflicts, deletions, crashes and failures on three devices, every version a user wrote
/// that is still live stays on some device or drive (checked after every durable transition).
#[test]
fn sim_noloss() {
    let mut w = World::new(1, &[&[0], &[0], &[0]], 7);
    let a = shared_note(&mut w, 0, "a.md");
    shared_note(&mut w, 1, "F/b.md");
    w.replace_line(0, "a.md", &line(&a, 0));
    w.replace_line(1, "a.md", &line(&a, 0));
    w.replace_line(2, "a.md", &line(&a, 0));
    let edited = w.edit(2, "F/b.md", Edit::Append);
    w.delete(0, "F");
    w.fail_at(1, "write copy", FaultKind::Crash);
    sync(&mut w, 0);
    sync(&mut w, 1);
    w.fail_at(2, "upload copy", FaultKind::LostResponse);
    sync(&mut w, 2);
    sync(&mut w, 1);
    w.assert_no_violations();
    finish(&mut w);
    assert_eq!(
        text(&w, 0, "F/b.md"),
        edited,
        "the edit beat the folder deletion"
    );
}

/// SIM-ORDER: the conflict substeps run in the spec's order (copy, then overwrite and record, then upload the
/// copy), so a failure before the conflicted copy exists leaves both originals as they were, and a crash at any
/// point after it loses nothing.
#[test]
fn sim_order() {
    for drive_wins in [true, false] {
        let setup = |w: &mut World| {
            let t = shared_note(w, 0, "p.md");
            let first = line(&t, 0);
            if drive_wins {
                w.replace_line(0, "p.md", &first);
                w.replace_line(1, "p.md", &first);
            } else {
                w.replace_line(1, "p.md", &first);
                w.replace_line(0, "p.md", &first);
            }
            sync(w, 1);
        };
        let mut w = World::pair();
        setup(&mut w);
        let (local, remote) = (text(&w, 0, "p.md"), drive_text(&w, 0, "p.md").unwrap());
        w.fail_at(0, "write copy", FaultKind::Fail);
        let out = sync(&mut w, 0);
        assert!(
            errors(&out)
                .iter()
                .any(|e| e.contains("conflicted copy could not be written")),
            "{out:?}"
        );
        assert_eq!(
            text(&w, 0, "p.md"),
            local,
            "the local original is untouched"
        );
        assert_eq!(
            drive_text(&w, 0, "p.md").unwrap(),
            remote,
            "the drive original is untouched"
        );
        assert!(copies(&w, 0).is_empty());
        finish(&mut w);
        assert_eq!(copies(&w, 0).len(), 1);

        for label in [
            "write copy",
            "save pending conflict",
            "write local",
            "drive upload revision",
            "save record",
            "upload copy",
        ] {
            let mut w = World::pair();
            setup(&mut w);
            w.fail_at(0, label, FaultKind::Crash);
            sync(&mut w, 0);
            finish(&mut w);
        }
    }
}

/// The story `sim_fail` sweeps: dev1's next sync runs every kind of action.
fn rich_story() -> World {
    let mut w = World::pair();
    let mut texts = std::collections::BTreeMap::new();
    for p in [
        "a.md", "b.md", "c.md", "c2.md", "m.md", "e.md", "z.md", "F/x.md", "G/y.md",
    ] {
        texts.insert(p, w.create(0, p));
    }
    let att = w.add_attachment(0);
    w.mkdir(0, "E");
    quiesce(&mut w);
    // A conflict dev1 loses (its edit is older), and one it wins (its edit is newer).
    w.replace_line(1, "c2.md", &line(&texts["c2.md"], 0));
    w.edit(0, "a.md", Edit::Append);
    w.delete(0, "b.md");
    w.delete(0, "G");
    w.delete(0, "z.md");
    w.create(0, "H/new.md");
    w.replace_line(0, "c.md", &line(&texts["c.md"], 0));
    w.replace_line(0, "c2.md", &line(&texts["c2.md"], 0));
    w.edit(0, "m.md", Edit::Append);
    w.mkdir(0, "J");
    let shared = w.add_attachment(0);
    sync(&mut w, 0);
    w.replace_line(1, "c.md", &line(&texts["c.md"], 0));
    w.edit(1, "m.md", Edit::Prepend);
    w.create(1, "d.md");
    w.edit(1, "e.md", Edit::Append);
    w.delete(1, "F");
    w.delete(1, "z.md");
    w.delete(1, &att);
    w.mkdir(1, "K");
    w.mkdir(1, "J");
    // The same attachment, added on dev1 too: on both sides, never synced.
    let bytes = w.devices[0].file(&shared).unwrap().0.to_vec();
    let t = w.time;
    w.devices[1].write(&shared, bytes, t);
    w.log("dev1 adds the same attachment");
    w
}

/// SIM-FAIL: a failure, a lost response or a crash after any substep of a sync, then a restart from durable state,
/// loses nothing, and once failures stop the devices converge (each sync with a new scheduler). Records are
/// dropped only after a deletion succeeded.
#[test]
fn sim_fail() {
    let mut clean = rich_story();
    let start = clean.steps;
    let outs = sync(&mut clean, 1);
    let n = clean.steps - start;
    let seen: BTreeSet<&str> = kinds(&outs).into_iter().collect();
    for k in [
        "recordFolder",
        "createRemoteFolder",
        "createLocalFolder",
        "forget",
        "uploadNew",
        "uploadRevision",
        "download",
        "record",
        "deleteLocal",
        "deleteRemote",
        "deleteLocalFolder",
        "deleteRemoteFolder",
        "reconcile",
    ] {
        assert!(seen.contains(k), "the story does not cover {k}: {seen:?}");
    }
    finish(&mut clean);

    for kind in [FaultKind::Fail, FaultKind::LostResponse, FaultKind::Crash] {
        for k in 1..=n {
            let mut w = rich_story();
            w.fault = Some(Fault {
                at: FaultAt::Step(w.steps + k),
                kind,
            });
            sync(&mut w, 1);
            assert!(!w.faults_hit.is_empty(), "{kind:?} at {k} was not hit");
            if let Some(msg) = panics(|| finish(&mut w)) {
                panic!("{kind:?} after substep {k} ({:?}): {msg}", w.faults_hit);
            }
        }
    }

    // Records are dropped only after a successful deletion: a failed folder deletion keeps them, so the next sync
    // deletes again instead of recreating the folder.
    for (label, folder, inner, again) in [
        ("drive delete folder", "F", "F/x.md", "deleteRemoteFolder"),
        ("delete local folder", "G", "G/y.md", "deleteLocalFolder"),
    ] {
        let mut w = rich_story();
        w.fail_at(1, label, FaultKind::Fail);
        sync(&mut w, 1);
        assert!(
            w.devices[1].record("drive0", folder).is_some(),
            "{folder} record kept"
        );
        assert!(
            w.devices[1].record("drive0", inner).is_some(),
            "{inner} record kept"
        );
        let next: Vec<&str> = w.plan_for(1, 0).actions.iter().map(kind).collect();
        assert!(next.contains(&again), "{label}: next plan {next:?}");
        finish(&mut w);
        assert!(w.devices[0].node(folder).is_none());
    }
}

/// SIM-MIDSYNC: edits made between the listing and a local write or deletion are never overwritten or deleted
/// (the step is skipped and picked up by the next sync); uploads send the bytes read at upload time and record
/// their hash.
#[test]
fn sim_midsync() {
    // A download skipped because the note was edited after the listing.
    let mut w = World::pair();
    shared_note(&mut w, 0, "p.md");
    w.edit(1, "p.md", Edit::Append);
    sync(&mut w, 1);
    w.hook(before_action(0, "download"), |w| {
        w.edit(0, "p.md", Edit::Prepend);
    });
    let out = sync(&mut w, 0);
    assert_eq!(out[0].skipped, ["p.md"]);
    assert!(out[0].errors.is_empty(), "a skip is not an error");
    finish(&mut w);
    assert!(copies(&w, 0).is_empty(), "the next sync merged both edits");

    // The same before a local deletion and before a reconcile.
    for hook_kind in ["deleteLocal", "reconcile"] {
        let mut w = World::pair();
        shared_note(&mut w, 0, "p.md");
        if hook_kind == "deleteLocal" {
            w.delete(1, "p.md");
        } else {
            w.edit(1, "p.md", Edit::Append);
            w.edit(0, "p.md", Edit::Prepend);
        }
        sync(&mut w, 1);
        w.hook(before_action(0, hook_kind), |w| {
            w.edit(0, "p.md", Edit::Append);
        });
        let out = sync(&mut w, 0);
        assert_eq!(out[0].skipped, ["p.md"], "{hook_kind}");
        finish(&mut w);
        assert!(w.devices[0].file("p.md").is_some());
    }

    // A new child (or an edit) inside a folder being deleted locally: the folder deletion is skipped.
    for nested_edit in [false, true] {
        let mut w = World::pair();
        w.create(0, "F/a.md");
        w.create(0, "F/S/b.md");
        quiesce(&mut w);
        w.delete(0, "F");
        sync(&mut w, 0);
        w.hook(before_action(1, "deleteLocalFolder"), move |w| {
            if nested_edit {
                w.edit(1, "F/S/b.md", Edit::Append);
            } else {
                w.create(1, "F/S/new.md");
            }
        });
        let out = sync(&mut w, 1);
        assert_eq!(out[0].skipped, ["F"]);
        finish(&mut w);
        let kept = if nested_edit {
            "F/S/b.md"
        } else {
            "F/S/new.md"
        };
        assert!(
            w.devices[0].file(kept).is_some(),
            "{kept} survived everywhere"
        );
    }

    // An upload sends the bytes read at upload time and records their hash.
    let mut w = World::pair();
    shared_note(&mut w, 0, "p.md");
    w.edit(0, "p.md", Edit::Append);
    w.hook(before_action(0, "uploadRevision"), |w| {
        w.edit(0, "p.md", Edit::Append);
    });
    sync(&mut w, 0);
    let now = text(&w, 0, "p.md");
    assert_eq!(drive_text(&w, 0, "p.md").unwrap(), now);
    assert_eq!(
        w.devices[0].record("drive0", "p.md").unwrap().local_hash,
        sha256_hex(now.as_bytes())
    );
    assert!(w.pending_actions(0, 0).is_empty());
    finish(&mut w);
}

/// SIM-CONDITIONAL: a revision upload or a deletion racing another device's write fails its precondition; the
/// step error keeps the record, and the next sync sees the other device's change.
#[test]
fn sim_conditional() {
    let mut w = World::pair();
    shared_note(&mut w, 0, "p.md");
    w.edit(0, "p.md", Edit::Append);
    w.edit(1, "p.md", Edit::Prepend);
    let before = w.devices[0].record("drive0", "p.md").unwrap().clone();
    w.hook(before_action(0, "uploadRevision"), |w| {
        sync(w, 1);
    });
    let out = sync(&mut w, 0);
    assert!(
        errors(&out)
            .iter()
            .any(|e| e.contains("precondition failed")),
        "{out:?}"
    );
    assert_eq!(
        w.devices[0].record("drive0", "p.md"),
        Some(&before),
        "record kept"
    );
    finish(&mut w);
    let merged = text(&w, 0, "p.md");
    assert!(
        copies(&w, 0).is_empty() && body_lines(&merged).len() == 4,
        "{merged}"
    );

    let mut w = World::pair();
    shared_note(&mut w, 0, "p.md");
    w.delete(0, "p.md");
    let edited = w.edit(1, "p.md", Edit::Append);
    w.hook(before_action(0, "deleteRemote"), |w| {
        sync(w, 1);
    });
    let out = sync(&mut w, 0);
    assert!(
        errors(&out)
            .iter()
            .any(|e| e.contains("precondition failed")),
        "{out:?}"
    );
    assert!(
        w.devices[0].record("drive0", "p.md").is_some(),
        "record kept"
    );
    finish(&mut w);
    assert_eq!(
        text(&w, 0, "p.md"),
        edited,
        "the edit won over the deletion"
    );
}

/// SIM-MTIME: uploads carry the note's modification time, downloads set the drive's, merges take the time they
/// are written and conflicted copies keep the losing version's: syncing never makes a note newer.
#[test]
fn sim_mtime() {
    let mut w = World::new(2, &[&[0, 1], &[0], &[1]], 3);
    w.create(1, "p.md");
    let t0 = w.devices[1].file("p.md").unwrap().1;
    w.time += 3_600_000;
    sync(&mut w, 1);
    assert_eq!(w.drives[0].entry("p.md").unwrap().modified_millis, t0);
    w.time += 3_600_000;
    sync(&mut w, 0);
    assert_eq!(
        w.devices[0].file("p.md").unwrap().1,
        t0,
        "download keeps the edit time"
    );
    assert_eq!(
        w.drives[1].entry("p.md").unwrap().modified_millis,
        t0,
        "relayed to the second drive"
    );
    sync(&mut w, 2);
    assert_eq!(w.devices[2].file("p.md").unwrap().1, t0);

    // A merge is a write: it takes the current time.
    w.edit(1, "p.md", Edit::Append);
    w.edit(0, "p.md", Edit::Prepend);
    sync(&mut w, 1);
    let merge_starts = w.time;
    sync_session(&mut w, 0, 0);
    let m = w.devices[0].file("p.md").unwrap().1;
    assert!(m > merge_starts && m <= w.time, "merge written at its time");
    assert_eq!(w.drives[0].entry("p.md").unwrap().modified_millis, m);
    finish(&mut w);

    // A conflicted copy keeps the losing version's time (see `gap_conflicted_copy_mtime`).
    let t = text(&w, 0, "p.md");
    w.replace_line(1, "p.md", &line(&t, 0));
    let loser_at = w.devices[1].file("p.md").unwrap().1;
    w.replace_line(0, "p.md", &line(&t, 0));
    sync(&mut w, 0);
    sync(&mut w, 1);
    let copy = copies(&w, 1).pop().unwrap();
    assert_eq!(w.devices[1].file(&copy).unwrap().1, loser_at);
    finish(&mut w);
}

fn sync_session(w: &mut World, dev: usize, r: usize) {
    sim::exec::sync_session(w, dev, r).expect("no crash");
}

/// SIM-MANIFEST: when another device re-uploads the content a device has recorded (a new drive version with the
/// same hash) and the local note is unchanged, the plan only records it: nothing is downloaded or rewritten.
#[test]
fn sim_manifest() {
    let mut w = World::pair();
    let original = shared_note(&mut w, 0, "p.md");
    let v1 = w.devices[0]
        .record("drive0", "p.md")
        .unwrap()
        .remote_version
        .clone();
    w.edit(1, "p.md", Edit::Append);
    sync(&mut w, 1);
    w.set_text(1, "p.md", &original);
    sync(&mut w, 1);
    let entry = w.drives[0].entry("p.md").unwrap().clone();
    assert_ne!(entry.version, v1);
    let before = w.devices[0].file("p.md").map(|(b, m)| (b.to_vec(), m));
    let plan = w.plan_for(0, 0).actions;
    let want = sha256_hex(original.as_bytes());
    assert!(
        matches!(plan.as_slice(), [Action::Record { path, sha256, .. }] if path == "p.md" && *sha256 == want),
        "{plan:?}"
    );
    let out = sync(&mut w, 0);
    assert_eq!(out[0].local_changes, 0);
    assert_eq!(
        w.devices[0].file("p.md").map(|(b, m)| (b.to_vec(), m)),
        before
    );
    assert_eq!(
        w.devices[0]
            .record("drive0", "p.md")
            .unwrap()
            .remote_version,
        entry.version
    );
    finish(&mut w);
}

/// SIM-CONVERGE: with fair participation, complete listings and operations that eventually succeed, every device
/// and drive ends with the same paths, types and bytes, and the next plan is empty.
#[test]
fn sim_converge() {
    let mut w = World::new(1, &[&[0], &[0], &[0]], 11);
    for (d, p) in [(0, "a.md"), (1, "F/b.md"), (2, "G/H/c.md"), (0, "F/d.md")] {
        w.create(d, p);
    }
    w.add_attachment(1);
    w.add_ignored(2, "G/notes.txt");
    w.mkdir(0, "Empty");
    w.drives[0].next_listing = Some(ListingMode::Fails);
    sync(&mut w, 0);
    w.fail_at(1, "drive upload new", FaultKind::Fail);
    for d in [1, 2, 0, 1, 0, 2] {
        sync(&mut w, d);
    }
    w.edit(0, "F/b.md", Edit::Append);
    w.edit(2, "F/b.md", Edit::Prepend);
    w.delete(1, "G/H");
    w.create_plain(2, "legacy.md");
    finish(&mut w);
    assert!(
        w.devices[1].node("G").is_some(),
        "G holds an ignored file on dev2"
    );
    assert!(
        w.devices[0].file("G/notes.txt").is_none(),
        "ignored files are not synced"
    );
}

/// SIM-IDEMP: after a fully successful pass the next plan is empty (also checked by the executor after every
/// clean session in every test).
#[test]
fn sim_idemp() {
    let mut w = World::new(2, &[&[0, 1], &[0]], 5);
    let t = w.create(0, "a.md");
    w.create(0, "F/b.md");
    w.add_attachment(0);
    w.mkdir(0, "E");
    let outs = sync_all(&mut w, 0).unwrap();
    assert!(outs.iter().all(|o| o.errors.is_empty()));
    assert!(w.pending_actions(0, 0).is_empty() && w.pending_actions(0, 1).is_empty());
    sync(&mut w, 1);
    assert!(w.pending_actions(1, 0).is_empty());
    w.replace_line(0, "a.md", &line(&t, 0));
    w.replace_line(1, "a.md", &line(&t, 0));
    sync(&mut w, 0);
    sync(&mut w, 1);
    assert!(w.pending_actions(1, 0).is_empty(), "after a conflict too");
    sync_all(&mut w, 0).unwrap();
    assert!(w.pending_actions(0, 0).is_empty() && w.pending_actions(0, 1).is_empty());
    let again = sync_all(&mut w, 0).unwrap();
    assert!(again.iter().all(|o| o.actions.is_empty()), "{again:?}");
    finish(&mut w);
}

/// SIM-MULTIDRIVE: a device linked to several drives relays changes between them within the 3-pass cap; the cap
/// can leave a drive behind until the next sync when every pass brings new changes.
#[test]
fn sim_multidrive() {
    let mut w = World::new(2, &[&[0, 1], &[0], &[1]], 9);
    w.create(1, "a.md");
    sync(&mut w, 1);
    w.create(2, "b.md");
    sync(&mut w, 2);
    let outs = sync_all(&mut w, 0).unwrap();
    assert_eq!(outs.iter().map(|o| o.drive).collect::<Vec<_>>(), [0, 1, 0]);
    assert!(w.pending_actions(0, 0).is_empty() && w.pending_actions(0, 1).is_empty());
    finish(&mut w);

    // Every pass brings a new note from the other side: after 3 passes per drive, drive0 is left behind.
    let mut n1 = 0;
    w.hook_repeat(
        When::SessionStart {
            dev: 0,
            drive: Some(0),
        },
        move |w| {
            n1 += 1;
            w.create(1, &format!("from1-{n1}.md"));
            sync(w, 1);
        },
    );
    let mut n2 = 0;
    w.hook_repeat(
        When::SessionStart {
            dev: 0,
            drive: Some(1),
        },
        move |w| {
            n2 += 1;
            w.create(2, &format!("from2-{n2}.md"));
            sync(w, 2);
        },
    );
    let outs = sync_all(&mut w, 0).unwrap();
    assert_eq!(
        outs.iter().map(|o| o.drive).collect::<Vec<_>>(),
        [0, 1, 0, 1, 0, 1]
    );
    assert!(w.devices[0].file("from2-3.md").is_some());
    assert!(
        w.drives[0].entry("from2-3.md").is_none(),
        "drive0 is behind until the next sync"
    );
    w.hooks.clear();
    sync_all(&mut w, 0).unwrap();
    assert!(w.drives[0].entry("from2-3.md").is_some());
    finish(&mut w);
}

// ----- stories -----

/// Two devices edit different lines of a note: the three-way merge keeps both, without a copy.
#[test]
fn story_edit_edit_clean_merge() {
    let mut w = World::pair();
    shared_note(&mut w, 0, "p.md");
    let a = w.edit(0, "p.md", Edit::Append);
    let b = w.edit(1, "p.md", Edit::Prepend);
    sync(&mut w, 0);
    sync(&mut w, 1);
    finish(&mut w);
    let merged = text(&w, 0, "p.md");
    assert!(
        merged.contains(&line(&a, 2)) && merged.contains(&line(&b, 0)),
        "{merged}"
    );
    assert!(copies(&w, 0).is_empty());
}

/// Two devices edit the same line: the newest edit wins at the note's path and the other becomes a conflicted copy
/// with a new id; both ways round.
#[test]
fn story_edit_edit_conflict() {
    for drive_newer in [true, false] {
        let mut w = World::pair();
        let t = shared_note(&mut w, 0, "p.md");
        let first = line(&t, 0);
        let (older, newer) = if drive_newer { (0, 1) } else { (1, 0) };
        let old_line = w.replace_line(older, "p.md", &first);
        let new_line = w.replace_line(newer, "p.md", &first);
        sync(&mut w, 1);
        let out = sync(&mut w, 0);
        assert!(kinds(&out).contains(&"reconcile"));
        finish(&mut w);
        let note = text(&w, 0, "p.md");
        assert!(
            note.contains(&new_line) && !note.contains(&old_line),
            "{note}"
        );
        let [copy] = <[String; 1]>::try_from(copies(&w, 0)).unwrap();
        assert_eq!(copy, "p (conflicted copy).md");
        let copy_text = text(&w, 0, &copy);
        assert!(copy_text.contains(&old_line));
        let id = front_matter::read_id(&copy_text).unwrap();
        assert!(front_matter::is_valid_id(&id));
        assert_ne!(Some(id), front_matter::read_id(&note));
        assert_eq!(
            w.devices[0].durable.pending_conflicts.len(),
            1,
            "dev0 made the copy"
        );
    }
}

/// One device edits a note the other deleted: the edit wins, whichever syncs first.
#[test]
fn story_edit_delete() {
    for deleter_first in [true, false] {
        let mut w = World::pair();
        shared_note(&mut w, 0, "p.md");
        w.delete(0, "p.md");
        let edited = w.edit(1, "p.md", Edit::Append);
        if deleter_first {
            sync(&mut w, 0);
            sync(&mut w, 1);
        } else {
            sync(&mut w, 1);
            sync(&mut w, 0);
        }
        finish(&mut w);
        assert_eq!(text(&w, 0, "p.md"), edited);
    }
    // An unchanged note deleted on one side is deleted everywhere.
    let mut w = World::pair();
    shared_note(&mut w, 0, "p.md");
    w.delete(1, "p.md");
    finish(&mut w);
    assert!(w.devices[0].file("p.md").is_none() && w.drives[0].entry("p.md").is_none());
}

/// A folder deleted on one device while a note inside it is edited, or a note is added to it, on the other.
#[test]
fn story_folder_delete_vs_inner_change() {
    for (new_child, deleter_first) in [(false, true), (false, false), (true, true), (true, false)] {
        let mut w = World::pair();
        w.create(0, "F/a.md");
        w.create(0, "F/S/b.md");
        quiesce(&mut w);
        w.delete(0, "F");
        let kept = if new_child {
            w.create(1, "F/S/new.md");
            "F/S/new.md"
        } else {
            w.edit(1, "F/S/b.md", Edit::Append);
            "F/S/b.md"
        };
        let order = if deleter_first { [0, 1] } else { [1, 0] };
        for d in order {
            sync(&mut w, d);
        }
        finish(&mut w);
        assert!(w.devices[0].file(kept).is_some(), "{kept} kept");
        assert!(
            w.devices[0].file("F/a.md").is_none(),
            "the untouched note is deleted"
        );
    }
}

/// A rename that only changes case is reported as the spec's case error on both paths and nothing is synced
/// for them; renaming through another name works.
#[test]
fn story_case_only_rename() {
    let mut w = World::pair();
    let original = shared_note(&mut w, 0, "Note.md");
    w.rename(0, "Note.md", "note.md");
    let out = sync(&mut w, 0);
    let errs = errors(&out);
    for p in ["Note.md", "note.md"] {
        assert!(errs.contains(&format!("{p}: {CASE_MESSAGE}")), "{errs:?}");
    }
    assert!(
        kinds(&out).iter().all(|k| *k == "error"),
        "nothing else is planned"
    );
    assert_eq!(drive_text(&w, 0, "Note.md").unwrap(), original);
    assert_eq!(w.drives[0].entry("Note.md").unwrap().path, "Note.md");
    w.assert_no_violations();

    w.rename(0, "note.md", "renaming.md");
    sync(&mut w, 0);
    sync(&mut w, 1);
    w.rename(0, "renaming.md", "note.md");
    sync(&mut w, 0);
    finish(&mut w);
    assert!(w.devices[1].file("note.md").is_some() && w.devices[1].file("Note.md").is_none());
    assert_eq!(w.drives[0].entry("note.md").unwrap().path, "note.md");
}

/// GAP: the spec's two-step case-only rename only works for a device that syncs between the two steps. A device
/// that sees only the end state (its `Note.md` against the drive's `note.md`) reports the case error on both paths
/// for ever: nothing is lost, but it never converges until its user renames the note too.
#[test]
fn gap_case_rename_missed_by_another_device() {
    let mut w = World::pair();
    let original = shared_note(&mut w, 0, "Note.md");
    w.rename(0, "Note.md", "renaming.md");
    sync(&mut w, 0);
    w.rename(0, "renaming.md", "note.md");
    sync(&mut w, 0);
    for _ in 0..2 {
        let out = sync(&mut w, 1);
        let errs = errors(&out);
        for p in ["Note.md", "note.md"] {
            assert!(errs.contains(&format!("{p}: {CASE_MESSAGE}")), "{errs:?}");
        }
        assert!(kinds(&out).iter().all(|k| *k == "error"));
    }
    assert_eq!(text(&w, 1, "Note.md"), original);
    assert_eq!(drive_text(&w, 0, "note.md").unwrap(), original);
    w.assert_no_violations();
    // The user renames it on dev1 too, through another name.
    w.rename(1, "Note.md", "renaming.md");
    sync(&mut w, 1);
    w.rename(1, "renaming.md", "note.md");
    finish(&mut w);
    assert!(w.devices[1].file("note.md").is_some());
}

/// Attachments are uploaded, downloaded and deleted, never merged; a corrupt download writes and records
/// nothing; the same attachment added on both sides is only recorded.
#[test]
fn story_attachments() {
    let mut w = World::pair();
    let att = w.add_attachment(0);
    sync(&mut w, 0);
    w.drives[0].corrupt_next_read.insert(paths::fold_case(&att));
    let out = sync(&mut w, 1);
    assert!(
        errors(&out)
            .iter()
            .any(|e| e.contains("does not match the hash")),
        "{out:?}"
    );
    assert!(w.devices[1].file(&att).is_none() && w.devices[1].record("drive0", &att).is_none());
    sync(&mut w, 1);
    assert_eq!(
        w.devices[1].file(&att).unwrap().0,
        w.devices[0].file(&att).unwrap().0
    );
    assert!(
        w.devices[1].base("drive0", &att).is_none(),
        "no base copy for attachments"
    );

    let both = w.add_attachment(0);
    let bytes = w.devices[0].file(&both).unwrap().0.to_vec();
    let t = w.time;
    w.devices[1].write(&both, bytes, t);
    sync(&mut w, 0);
    let out = sync(&mut w, 1);
    assert!(out[0]
        .actions
        .iter()
        .any(|a| matches!(a, Action::Record { path, .. } if *path == both)));

    w.delete(1, &att);
    finish(&mut w);
    assert!(w.devices[0].file(&att).is_none() && w.drives[0].entry(&att).is_none());
}

/// A device offline through many changes catches up in one sync, keeping its own offline edits.
#[test]
fn story_offline_device_catches_up() {
    let mut w = World::pair();
    let a = shared_note(&mut w, 0, "a.md");
    shared_note(&mut w, 0, "F/b.md");
    shared_note(&mut w, 0, "c.md");
    shared_note(&mut w, 0, "G/d.md");
    for _ in 0..3 {
        w.edit(0, "c.md", Edit::Append);
        sync(&mut w, 0);
    }
    w.replace_line(0, "a.md", &line(&a, 0));
    w.delete(0, "F");
    w.delete(0, "G");
    w.create(0, "H/e.md");
    sync(&mut w, 0);
    w.replace_line(1, "a.md", &line(&a, 0));
    w.edit(1, "F/b.md", Edit::Append);
    w.create(1, "offline.md");
    w.time += 86_400_000;
    finish(&mut w);
    assert!(
        w.devices[0].file("F/b.md").is_some(),
        "the offline edit beat the folder deletion"
    );
    assert!(w.devices[0].node("G").is_none());
    assert_eq!(copies(&w, 0).len(), 1);
}

/// Conflicts are decided by the devices' clocks: a device whose clock is behind loses although it edited later
/// (no loss: its version is the copy), and equal times go to the drive.
#[test]
fn story_clock_skew_and_ties() {
    let mut w = World::pair();
    w.devices[1].skew = -3_600_000;
    let t = shared_note(&mut w, 0, "p.md");
    let early = w.replace_line(0, "p.md", &line(&t, 0));
    let late = w.replace_line(1, "p.md", &line(&t, 0));
    sync(&mut w, 1);
    sync(&mut w, 0);
    finish(&mut w);
    assert!(
        text(&w, 0, "p.md").contains(&early),
        "dev0's clock says it is newer"
    );
    assert!(text(&w, 0, "p (conflicted copy).md").contains(&late));

    let mut w = World::pair();
    w.devices[1].skew = -1_000; // dev1 edits 1000 ms later, at the same clock time
    let t = shared_note(&mut w, 0, "p.md");
    let local = w.replace_line(0, "p.md", &line(&t, 0));
    let drive = w.replace_line(1, "p.md", &line(&t, 0));
    assert_eq!(
        w.devices[0].file("p.md").unwrap().1,
        w.devices[1].file("p.md").unwrap().1
    );
    sync(&mut w, 1);
    sync(&mut w, 0);
    finish(&mut w);
    assert!(
        text(&w, 0, "p.md").contains(&drive),
        "a tie goes to the drive"
    );
    assert!(text(&w, 0, "p (conflicted copy).md").contains(&local));
}

/// A drive commit whose response is lost (or followed by a crash) is not recorded; the next sync finds the same
/// state on both sides and only records or forgets it: no new upload, no conflicted copy.
#[test]
fn story_lost_response_after_drive_commit() {
    for kind in [FaultKind::LostResponse, FaultKind::Crash] {
        for label in ["drive upload revision", "drive upload new", "drive delete"] {
            let mut w = World::pair();
            shared_note(&mut w, 0, "p.md");
            match label {
                "drive upload new" => {
                    w.create(0, "q.md");
                }
                "drive delete" => w.delete(0, "p.md"),
                _ => {
                    w.edit(0, "p.md", Edit::Append);
                }
            }
            w.fail_at(0, label, kind);
            sync(&mut w, 0);
            assert_eq!(w.faults_hit.len(), 1, "{label}");
            let out = sync(&mut w, 0);
            let k = kinds(&out);
            assert!(
                !k.contains(&"uploadNew") && !k.contains(&"uploadRevision"),
                "{label}: {k:?}"
            );
            finish(&mut w);
            assert!(copies(&w, 0).is_empty(), "{label}");
        }
    }
}

/// Two devices lose the same conflict and name their copies the same: the second copy upload is refused
/// (uploads of new files are create-only) and both copies survive under different names.
#[test]
fn story_conflict_name_race() {
    let mut w = World::new(1, &[&[0], &[0], &[0]], 13);
    let t = shared_note(&mut w, 0, "p.md");
    let first = line(&t, 0);
    let l0 = w.replace_line(0, "p.md", &first);
    let l1 = w.replace_line(1, "p.md", &first);
    w.replace_line(2, "p.md", &first);
    sync(&mut w, 2);
    w.hook(before_action(1, "reconcile"), |w| {
        sync(w, 0);
    });
    let out = sync(&mut w, 1);
    assert!(
        errors(&out).iter().any(|e| e.contains("already exists")),
        "{out:?}"
    );
    finish(&mut w);
    let all: String = copies(&w, 0).iter().map(|c| text(&w, 0, c)).collect();
    assert!(all.contains(&l0) && all.contains(&l1), "both copies kept");
}

/// Three devices and two drives (dev0 relays between them), with conflicts, deletions and failures.
#[test]
fn story_three_devices_two_drives() {
    let mut w = World::new(2, &[&[0, 1], &[0], &[1]], 17);
    let a = w.create(1, "a.md");
    w.create(2, "F/b.md");
    w.add_attachment(2);
    quiesce(&mut w);
    w.replace_line(1, "a.md", &line(&a, 0));
    w.replace_line(2, "a.md", &line(&a, 0));
    w.edit(0, "F/b.md", Edit::Append);
    w.delete(1, "F");
    w.create(2, "c.md");
    w.drives[1].next_listing = Some(ListingMode::Fails);
    sync(&mut w, 0);
    sync(&mut w, 1);
    w.fail_at(0, "write local", FaultKind::Crash);
    sync(&mut w, 0);
    sync(&mut w, 2);
    finish(&mut w);
    assert_eq!(copies(&w, 0).len(), 1);
    assert!(
        w.devices[2].file("F/b.md").is_some(),
        "the edit beat the folder deletion"
    );
}

// ----- characterisation -----

/// GAP: drive folder deletions are not conditional (the spec's adapter contract only makes file deletions
/// conditional, and Dropbox and OneDrive delete folders recursively). A file another device uploads into the
/// folder between the listing and the folder deletion is deleted from the drive unseen, and that device then
/// deletes its own copy as "deleted on the drive": the note is lost. This test documents the loss.
#[test]
fn gap_folder_delete_races_concurrent_upload() {
    let mut w = World::pair();
    shared_note(&mut w, 0, "F/a.md");
    w.delete(0, "F");
    w.hook(before_action(0, "deleteRemoteFolder"), |w| {
        w.create(1, "F/new.md");
        sync(w, 1);
    });
    sync(&mut w, 0);
    quiesce(&mut w);
    assert!(
        w.violations()
            .iter()
            .any(|v| v.contains("F/new.md: deleted although it was never agreed")),
        "{:?}",
        w.violations()
    );
    assert!(
        w.violations().iter().any(|v| v.contains("LOST")),
        "{:?}",
        w.violations()
    );
    assert!(w.devices[1].file("F/new.md").is_none());
}

/// AMBIGUITY: the spec does not say which modification time the conflicted copy gets. The reference executor
/// gives it the losing version's time (when that content was last edited), so that syncing never makes it newer.
#[test]
fn gap_conflicted_copy_mtime() {
    let mut w = World::pair();
    let t = shared_note(&mut w, 0, "p.md");
    w.replace_line(0, "p.md", &line(&t, 0));
    let loser_at = w.devices[0].file("p.md").unwrap().1;
    w.replace_line(1, "p.md", &line(&t, 0));
    sync(&mut w, 1);
    sync(&mut w, 0);
    let copy = "p (conflicted copy).md";
    assert_eq!(w.devices[0].file(copy).unwrap().1, loser_at);
    assert_eq!(w.drives[0].entry(copy).unwrap().modified_millis, loser_at);
    finish(&mut w);
}

/// OBSERVATION: a crash after the conflicted copy is written but before the note is overwritten leaves the conflict
/// in place; the next sync reconciles again and writes a second copy (`… (conflicted copy) 2.md`) with the same
/// content. Nothing is lost, but the user sees a duplicate.
#[test]
fn gap_crash_after_copy_duplicates_it() {
    let mut w = World::pair();
    let t = shared_note(&mut w, 0, "p.md");
    w.replace_line(0, "p.md", &line(&t, 0));
    w.replace_line(1, "p.md", &line(&t, 0));
    sync(&mut w, 1);
    w.fail_at(0, "save pending conflict", FaultKind::Crash);
    sync(&mut w, 0);
    finish(&mut w);
    let c = copies(&w, 0);
    assert_eq!(c.len(), 2, "{c:?}");
    let body = |p: &str| body_lines(&text(&w, 0, p)).join("\n");
    assert_eq!(body(&c[0]), body(&c[1]), "the same content twice");
}

// ----- randomized -----

/// GAP (several drives): the plan compares the local note with the record of each drive separately, so a note put
/// back with exactly its recorded bytes looks unchanged to a drive whose record is stale. Here dev0 moves `F/d.md`
/// away, syncs only drive0 (the deletion reaches it, the upload under the new name fails), and the user moves the
/// note back. dev1 relays the deletion to drive1. dev0's next sync uploads the note to drive0 again, then deletes
/// it locally because drive1 deleted it and the local copy "is unchanged" since drive1's record, then deletes it
/// from drive0 for the same reason: the note the user moved back is gone from every device and drive. The core
/// follows the spec here; the randomized runs avoid this pattern (see `user_op`).
#[test]
fn gap_restored_note_deleted_through_stale_record() {
    let mut w = World::new(2, &[&[0, 1], &[0, 1]], 19);
    let x = shared_note(&mut w, 0, "F/d.md");
    w.rename(0, "F/d.md", "b.md");
    w.fail_at(0, "drive upload new", FaultKind::Fail);
    sync_session(&mut w, 0, 0);
    assert!(w.drives[0].entry("F/d.md").is_none() && w.drives[0].entry("b.md").is_none());
    w.rename(0, "b.md", "F/d.md");
    sync(&mut w, 1);
    assert!(w.drives[1].entry("F/d.md").is_none());
    let outs = sync(&mut w, 0);
    assert!(kinds(&outs).contains(&"deleteLocal"), "{outs:?}");
    assert!(
        w.violations().iter().any(|v| v.contains("LOST")),
        "{:?}",
        w.violations()
    );
    let anywhere = (0..2).any(|d| w.devices[d].text("F/d.md").as_deref() == Some(&x))
        || (0..2).any(|r| drive_text(&w, r, "F/d.md").as_deref() == Some(&x));
    assert!(!anywhere, "the note is gone everywhere");
}

// ----- randomized -----

const NOTES: [&str; 8] = [
    "a.md", "b.md", "F/c.md", "F/d.md", "F/S/e.md", "G/f.md", "G/T/g.md", "h.md",
];
const FOLDERS: [&str; 5] = ["F", "F/S", "G", "G/T", "E"];

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn notes_on(w: &World, dev: usize) -> Vec<String> {
    w.devices[dev]
        .durable
        .files
        .iter()
        .filter(|(p, n)| is_note_path(p) && matches!(n, Node::File { .. }))
        .map(|(p, _)| p.clone())
        .collect()
}

/// A random user operation on `dev`.
fn user_op(w: &mut World, rng: &mut Rng, dev: usize) {
    let notes = notes_on(w, dev);
    let free: Vec<&str> = NOTES
        .iter()
        .copied()
        .filter(|p| w.devices[dev].node(p).is_none())
        .collect();
    match rng.below(100) {
        0..=24 => {
            if let Some(p) = rng.pick(&free) {
                if rng.chance(15) {
                    w.create_plain(dev, p);
                } else {
                    w.create(dev, p);
                }
            }
        }
        25..=59 => {
            if let Some(p) = rng.pick(&notes).cloned() {
                let how = EDITS[rng.below(EDITS.len())];
                w.edit(dev, &p, how);
            }
        }
        60..=69 => {
            if let Some(p) = rng.pick(&notes).cloned() {
                w.delete(dev, &p);
            }
        }
        70..=74 => {
            let present: Vec<&str> = FOLDERS
                .iter()
                .copied()
                .filter(|p| w.devices[dev].node(p).is_some())
                .collect();
            if let Some(p) = rng.pick(&present) {
                w.delete(dev, p);
            }
        }
        75..=79 => {
            let f = FOLDERS[rng.below(FOLDERS.len())];
            w.mkdir(dev, f);
        }
        80..=85 => {
            if let (Some(from), Some(to)) = (rng.pick(&notes).cloned(), rng.pick(&free)) {
                // Moving a note to a path where this device holds a record of the same bytes is the pattern of
                // `gap_restored_note_deleted_through_stale_record`.
                let hash = w.devices[dev].hash(&from);
                let restores = w.devices[dev].drives.iter().any(|r| {
                    w.devices[dev]
                        .record(&format!("drive{r}"), to)
                        .is_some_and(|rec| Some(&rec.local_hash) == hash.as_ref())
                });
                if !restores {
                    w.rename(dev, &from, to);
                }
            }
        }
        86..=91 => {
            w.add_attachment(dev);
        }
        92..=95 => {
            let atts: Vec<String> = w.devices[dev]
                .durable
                .files
                .keys()
                .filter(|p| attachment_hash(p).is_some())
                .cloned()
                .collect();
            if let Some(p) = rng.pick(&atts).cloned() {
                w.delete(dev, &p);
            }
        }
        _ => {
            let f = FOLDERS[rng.below(FOLDERS.len())];
            w.add_ignored(dev, &format!("{f}/notes.txt"));
        }
    }
}

/// Does the rest of `dev`'s plan in flight delete a drive folder? (see `gap_folder_delete_races_concurrent_upload`)
fn deletes_remote_folder_later(w: &World, dev: usize) -> bool {
    w.devices[dev].volatile.as_ref().is_some_and(|v| {
        v.plan[v.next_action..]
            .iter()
            .any(|a| matches!(a, Action::DeleteRemoteFolder { .. }))
    })
}

/// A random history of `ops` operations, without the final quiescence.
fn random_run(seed: u64, ops: usize) -> World {
    let mut rng = Rng::new(seed);
    let n_drives = 1 + rng.below(2);
    let n_devices = 2 + rng.below(2);
    let mut links: Vec<Vec<usize>> = vec![(0..n_drives).collect()];
    for _ in 1..n_devices {
        if n_drives == 2 && rng.chance(50) {
            links.push(vec![rng.below(2)]);
        } else {
            links.push((0..n_drives).collect());
        }
    }
    let link_refs: Vec<&[usize]> = links.iter().map(Vec::as_slice).collect();
    let mut w = World::new(n_drives, &link_refs, seed ^ 0x5EED);
    for d in 1..n_devices {
        w.devices[d].skew = rng.below(4001) as i64 - 2000;
    }
    w.log(format!(
        "seed {seed}: {n_devices} devices, {n_drives} drives, links {links:?}"
    ));
    for _ in 0..ops {
        let dev = rng.below(n_devices);
        match rng.below(100) {
            0..=49 => user_op(&mut w, &mut rng, dev),
            50..=69 => {
                sync(&mut w, dev);
            }
            70..=79 => {
                let kind =
                    [FaultKind::Fail, FaultKind::LostResponse, FaultKind::Crash][rng.below(3)];
                let k = 1 + rng.below(30) as u64;
                w.fault = Some(Fault {
                    at: FaultAt::Step(w.steps + k),
                    kind,
                });
                sync(&mut w, dev);
                w.fault = None;
            }
            80..=87 => {
                // A user operation on the same device in the middle of its sync.
                let mut op_rng = Rng::new(rng.next_u64());
                let at = rng.below(6);
                let mut seen = 0;
                w.hook_repeat(
                    When::BeforeAction {
                        dev,
                        kind: None,
                        path: None,
                    },
                    move |w| {
                        if seen == at {
                            w.log("  (mid-sync user operation)");
                            user_op(w, &mut op_rng, dev);
                        }
                        seen += 1;
                    },
                );
                sync(&mut w, dev);
                w.hooks.clear();
            }
            88..=93 => {
                // Another device syncs in the middle of this device's sync.
                let other = (dev + 1 + rng.below(n_devices - 1)) % n_devices;
                let at = rng.below(6);
                let mut seen = 0;
                w.hook_repeat(
                    When::BeforeAction {
                        dev,
                        kind: None,
                        path: None,
                    },
                    move |w| {
                        if seen == at && !deletes_remote_folder_later(w, dev) {
                            w.log(format!("  (dev{other} syncs concurrently)"));
                            sync(w, other);
                        }
                        seen += 1;
                    },
                );
                sync(&mut w, dev);
                w.hooks.clear();
            }
            94..=96 => {
                let r = rng.below(n_drives);
                w.drives[r].next_listing = Some(if rng.chance(50) {
                    ListingMode::Fails
                } else {
                    ListingMode::Incomplete
                });
                sync(&mut w, dev);
                w.drives[r].next_listing = None;
            }
            _ => {
                let r = rng.below(n_drives);
                let atts: Vec<String> = w.drives[r]
                    .entries()
                    .filter(|e| attachment_hash(&e.path).is_some())
                    .map(|e| paths::fold_case(&e.path))
                    .collect();
                if let Some(p) = rng.pick(&atts) {
                    w.drives[r].corrupt_next_read.insert(p.clone());
                }
                sync(&mut w, dev);
                w.drives[r].corrupt_next_read.clear();
            }
        }
        if !w.violations().is_empty() {
            break;
        }
    }
    w
}

/// SIM-NOLOSS, SIM-CONVERGE and SIM-IDEMP over seeded random histories: 2–3 devices, 1–2 drives, edits, creates,
/// deletions, folder operations, renames, attachments, ignored files, syncs, failures, lost responses, crashes,
/// mid-sync user operations, concurrent devices, failed and incomplete listings and corrupt downloads. The oracle
/// runs after every durable transition; then the world is quiesced (no faults, fair syncs) and must converge.
/// `MARKNOTES_SIM_SEED` sets the first seed and `MARKNOTES_SIM_RUNS` the number of runs.
#[test]
fn randomized() {
    let first = env_u64("MARKNOTES_SIM_SEED").unwrap_or(0x4D41_524B);
    let runs = env_u64("MARKNOTES_SIM_RUNS").unwrap_or(200);
    for i in 0..runs {
        let seed = first.wrapping_add(i);
        let failure = panics(|| {
            let mut w = random_run(seed, 60);
            if w.violations().is_empty() {
                quiesce(&mut w);
            }
            w.assert_no_violations();
            w.assert_converged();
        });
        if let Some(msg) = failure {
            panic!(
                "randomized run failed with seed {seed}; reproduce with MARKNOTES_SIM_SEED={seed} MARKNOTES_SIM_RUNS=1\n{msg}"
            );
        }
    }
}

/// The same seed gives the same history.
#[test]
fn randomized_is_deterministic() {
    let a = random_run(42, 40);
    let b = random_run(42, 40);
    assert_eq!(a.log, b.log);
    assert_eq!(a.fingerprint(), b.fingerprint());
}
