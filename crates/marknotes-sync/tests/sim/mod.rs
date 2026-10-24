//! End-to-end simulation harness for marknotes-sync.
//!
//! The harness runs the real core (`plan`, `reconcile`, `build_manifest`, `Scheduler`) through a reference
//! executor of the spec (`exec`), over several simulated devices (`device`) and drives (`drive`), with injected
//! failures and crashes (`world`), and checks after every durable transition that nothing was lost (`oracle`).
//!
//! # State model
//!
//! - A **drive** maps case-folded paths to entries `{id, version, bytes, modifiedMillis, isFolder}`. Every commit
//!   gets a new version (`v<n>`, never reused) and is kept in `history` for the oracle. New files are uploaded
//!   create-only, revisions and file deletions are conditional on the listed version (a mismatch is the step error
//!   `precondition failed`), folder creation is idempotent and folder deletion is recursive and unconditional (as
//!   on Dropbox and OneDrive). A listing can be made to fail or to come back incomplete, and a read can be corrupted
//!   in transit.
//! - A **device** has a durable part (files with bytes and modification time, sync records and base copies per
//!   drive, pending conflicts) and a volatile part (the plan in flight and the listing it came from). A crash
//!   discards the volatile part; a restart starts from the durable part alone.
//! - The **world** owns a clock (`time`, plus a per-device skew), a substep counter and the fault injector.
//!
//! # Fault points
//!
//! Every externally visible substep of the executor goes through `World::point` with a label: `list local`,
//! `list drive`, `drive read`, `write local`, `delete local`, `delete local folder`, `create local folder`,
//! `drive upload new`, `drive upload revision`, `drive delete`, `drive delete folder`, `drive create folder`,
//! `write copy`, `save pending conflict`, `upload copy`, `save record` (record and base copy in one transaction),
//! `forget`, `write manifest`, `prune conflicts`. A fault (by global step number or by device and label) makes the
//! substep **fail** (not applied, step error), **lose its response** (applied, but the executor sees an error) or
//! **crash** the device right after it (applied, then the sync in flight is lost). Hooks run user operations or
//! other devices' syncs at a session start or before any action (mid-sync edits and concurrent devices).
//!
//! # The no-loss oracle
//!
//! The oracle never trusts sync records to decide what must exist. It follows the users and compares bytes.
//!
//! - **Lines.** Users only ever type globally unique lines; the oracle tracks the non-empty body lines (after the
//!   front-matter block, parsed by the oracle itself) of every note. A line is **retired** when a user removes it
//!   by an edit or deletes a file holding it. (Retirement is global: once a user removed a line anywhere, the
//!   oracle stops requiring it everywhere. This can only hide losses, never invent them.)
//! - **Presence.** Every line a user wrote that is not retired is in some note on some replica (any device or
//!   drive, at any path, a conflicted copy included).
//! - **Cohesion.** The live lines each user write introduced are all together in at least one note: a version is
//!   not considered kept if its lines survive only scattered over several files.
//! - **Provenance and modification time.** Every note body on every replica is a body a user wrote or a merge
//!   produced, and its modification time is one at which that body was written (by a user edit or a merge).
//!   Syncing never makes a note newer.
//! - **Clean merges.** Each merge the executor writes keeps every line that either side added to the base and adds
//!   no line that is in neither side (checked by the oracle's own line-set code, not by `merge3`).
//! - **Attachments.** Every live attachment is on some replica with its bytes, and every attachment on a device
//!   matches the hash in its name.
//! - **Records.** A saved file record names a drive version that was really committed and whose bytes hash to the
//!   recorded local hash; those bytes become the *agreed blob* for (device, drive, path) until it is forgotten.
//!   `record`, `recordFolder` and `forget` actions change no bytes, modification times or ids anywhere.
//! - **Deletions.** A sync may delete a file (alone or inside a folder), locally or on a drive, only after a
//!   complete listing and only if the deleted bytes are the agreed blob. (That no *other* content is lost through a
//!   deletion is the presence and cohesion rules.)
//! - **Manifest.** Every manifest entry names a version the drive really committed, with that content's hash: only
//!   successful steps are published.
//! - **Idempotence.** After a session without errors, skips, hooks or faults, the next plan for that drive is empty
//!   (errors aside).
//!
//! Violations are collected in `World::oracle.violations`; tests assert there are none (or, for the deliberately
//! buggy executors in `Bugs`, that there are some).

#![allow(dead_code)]

pub mod device;
pub mod drive;
pub mod exec;
pub mod oracle;
pub mod rng;
pub mod safety;
pub mod world;

pub use exec::{sync, sync_all};
pub use world::{Bugs, Edit, FaultKind, When, World};

use std::panic::{catch_unwind, AssertUnwindSafe};

/// Syncs every device in turn until a round changes nothing (no faults, no hooks), at most `max_rounds` rounds.
pub fn quiesce(w: &mut World) {
    w.fault = None;
    w.hooks.clear();
    for _ in 0..20 {
        let before = w.fingerprint() + &records_fingerprint(w);
        for d in 0..w.devices.len() {
            sync(w, d);
        }
        if w.fingerprint() + &records_fingerprint(w) == before {
            return;
        }
    }
    panic!("no quiescence after 20 rounds\n{}", w.describe());
}

fn records_fingerprint(w: &World) -> String {
    format!(
        "{:?}",
        w.devices
            .iter()
            .map(|d| &d.durable.records)
            .collect::<Vec<_>>()
    )
}

/// A note created on `dev` and synced to every device.
pub fn shared_note(w: &mut World, dev: usize, path: &str) -> String {
    let text = w.create(dev, path);
    quiesce(w);
    text
}

/// Runs `f` and returns the panic message, if any.
pub fn panics(f: impl FnOnce()) -> Option<String> {
    catch_unwind(AssertUnwindSafe(f)).err().map(|e| {
        e.downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::drive::Drive;
    use super::exec::sync_session;
    use super::*;

    #[test]
    fn drive_writes_are_conditional_and_create_only() {
        let mut d = Drive::new("t");
        let a = d.create_file("a.md", b"1".to_vec(), 5, false).unwrap();
        assert!(
            d.create_file("A.MD", b"2".to_vec(), 6, false).is_err(),
            "create-only, case-insensitive"
        );
        assert!(
            d.create_file("F/x.md", b"2".to_vec(), 6, false).is_err(),
            "parent must exist"
        );
        let err = d
            .update_file("a.md", &a.id, Some("v999"), b"3".to_vec(), 7)
            .unwrap_err();
        assert!(err.contains("precondition failed"));
        assert_eq!(
            d.entry("a.md").unwrap().bytes,
            b"1",
            "a failed precondition changes nothing"
        );
        let b = d
            .update_file("a.md", &a.id, Some(&a.version), b"3".to_vec(), 7)
            .unwrap();
        assert_ne!(a.version, b.version);
        assert!(d.delete_file("a.md", Some(&a.version)).is_err());
        assert_eq!(
            d.bytes_at("a.md", &a.version).unwrap(),
            b"1",
            "history keeps old versions"
        );
        let folder = d.create_folder("F").unwrap();
        assert!(d.create_folder("F").is_err());
        d.create_file("F/x.md", b"x".to_vec(), 1, false).unwrap();
        assert!(d.delete_empty_folder("F", &folder.id).is_err());
        d.delete_folder("f").unwrap();
        assert!(d.entry("F/x.md").is_none());
        d.delete_file("a.md", Some(&b.version)).unwrap();
    }

    #[test]
    fn drive_listing_modes() {
        let mut d = Drive::new("t");
        for p in ["a.md", "b.md", "c.md", "d.md"] {
            d.create_file(p, p.as_bytes().to_vec(), 1, false).unwrap();
        }
        d.next_listing = Some(drive::ListingMode::Fails);
        assert!(d.list().is_err());
        d.next_listing = Some(drive::ListingMode::Incomplete);
        let l = d.list().unwrap();
        assert!(!l.complete && l.entries.len() == 2);
        assert_eq!(d.list().unwrap().entries.len(), 4, "the mode is one-shot");
    }

    #[test]
    fn crash_discards_volatile_state_and_keeps_durable_state() {
        let mut w = World::pair();
        w.create(0, "a.md");
        w.create(0, "b.md");
        sync(&mut w, 0);
        w.hook(
            When::BeforeAction {
                dev: 1,
                kind: Some("download"),
                path: Some("b.md".into()),
            },
            |w| assert!(w.devices[1].volatile.is_some(), "a plan is in flight"),
        );
        w.fail_at(1, "write local", FaultKind::Crash);
        assert!(sync_all(&mut w, 1).is_err());
        let d = &w.devices[1];
        assert!(d.volatile.is_none(), "the crash discarded the plan");
        assert!(
            d.file("a.md").is_some(),
            "the write before the crash is durable"
        );
        assert!(
            d.record("drive0", "a.md").is_none(),
            "but it was not recorded"
        );
        w.restart(1);
        quiesce(&mut w);
        w.assert_converged();
        w.assert_no_violations();
    }

    #[test]
    fn oracle_checks_merges_itself() {
        let mut o = oracle::Oracle::default();
        o.merge_written(
            "ok",
            Some("a\nb\n"),
            "a\nb\nl\n",
            "r\na\nb\n",
            "r\na\nb\nl\n",
            1,
        );
        assert!(o.violations.is_empty());
        o.merge_written(
            "drop",
            Some("a\nb\n"),
            "a\nb\nl\n",
            "r\na\nb\n",
            "r\na\nb\n",
            1,
        );
        assert!(o.violations.iter().any(|v| v.contains("dropped")));
        o.merge_written("invent", Some("a\n"), "a\n", "a\nr\n", "a\nr\nx\n", 1);
        assert!(o.violations.iter().any(|v| v.contains("invented")));
    }

    #[test]
    fn oracle_flags_a_line_that_vanishes() {
        let mut w = World::pair();
        w.create(0, "a.md");
        w.assert_no_violations();
        w.devices[0].durable.files.remove("a.md"); // not a user deletion
        w.after("test");
        assert!(w.violations().iter().any(|v| v.contains("LOST")));
    }

    #[test]
    fn oracle_flags_split_versions() {
        let mut w = World::pair();
        let text = w.create(0, "a.md");
        let lines: Vec<&str> = oracle::body_lines(&text);
        let (l1, l2) = (lines[0].to_string(), lines[1].to_string());
        w.devices[0].write("a.md", format!("{l1}\n").into_bytes(), 0);
        w.devices[0].write("b.md", format!("{l2}\n").into_bytes(), 0);
        w.after("test");
        assert!(w
            .violations()
            .iter()
            .any(|v| v.contains("no longer together")));
    }

    // ----- each deliberate executor bug is caught -----

    /// Runs `story` with and without the bug: clean without, flagged with.
    fn caught(expect: &str, set: impl Fn(&mut Bugs), story: impl Fn(&mut World)) {
        let mut good = World::pair();
        story(&mut good);
        good.assert_no_violations();
        let mut bad = World::pair();
        set(&mut bad.bugs);
        let panicked = panics(AssertUnwindSafe(|| story(&mut bad)));
        assert!(
            bad.violations().iter().any(|v| v.contains(expect)),
            "the oracle did not flag the bug with {expect:?} (panic: {panicked:?}): {:?}\nlog:\n  {}",
            bad.violations(),
            bad.log.join("\n  ")
        );
    }

    /// dev0 edits the note while it is about to download dev1's version.
    fn edit_during_download(w: &mut World) {
        shared_note(w, 0, "p.md");
        w.edit(1, "p.md", Edit::Append);
        sync(w, 1);
        w.hook(
            When::BeforeAction {
                dev: 0,
                kind: Some("download"),
                path: None,
            },
            |w| {
                w.edit(0, "p.md", Edit::Prepend);
            },
        );
        sync(w, 0);
        quiesce(w);
    }

    #[test]
    fn bug_no_recheck_is_caught() {
        caught("LOST", |b| b.no_recheck = true, edit_during_download);
    }

    /// A conflict the drive wins; dev0 crashes right after overwriting its local version.
    fn conflict_crash_after_overwrite(w: &mut World) {
        let text = shared_note(w, 0, "p.md");
        let first = oracle::body_lines(&text)[0].to_string();
        w.replace_line(0, "p.md", &first);
        w.replace_line(1, "p.md", &first);
        sync(w, 1);
        w.fail_at(0, "write local", FaultKind::Crash);
        sync(w, 0);
        quiesce(w);
    }

    #[test]
    fn bug_copy_after_overwrite_is_caught() {
        caught(
            "LOST",
            |b| b.copy_after_overwrite = true,
            conflict_crash_after_overwrite,
        );
    }

    /// dev1 uploads a revision between dev0's listing and dev0's upload.
    fn concurrent_revision(w: &mut World) {
        shared_note(w, 0, "p.md");
        w.edit(0, "p.md", Edit::Append);
        w.edit(1, "p.md", Edit::Prepend);
        w.hook(
            When::BeforeAction {
                dev: 0,
                kind: Some("uploadRevision"),
                path: None,
            },
            |w| {
                sync(w, 1);
            },
        );
        sync(w, 0);
        quiesce(w);
    }

    #[test]
    fn bug_unconditional_is_caught() {
        caught("LOST", |b| b.unconditional = true, concurrent_revision);
    }

    /// Two devices lose the same conflict to a third version and both name their copy the same.
    fn conflict_name_race(w: &mut World) {
        w.devices.push(device::Device::new("dev2", vec![0]));
        let text = shared_note(w, 0, "p.md");
        let first = oracle::body_lines(&text)[0].to_string();
        w.replace_line(0, "p.md", &first);
        w.replace_line(1, "p.md", &first);
        w.replace_line(2, "p.md", &first);
        sync(w, 2);
        w.hook(
            When::BeforeAction {
                dev: 1,
                kind: Some("reconcile"),
                path: None,
            },
            |w| {
                sync(w, 0);
            },
        );
        sync(w, 1);
        quiesce(w);
    }

    #[test]
    fn bug_upload_new_overwrites_is_caught() {
        caught(
            "LOST",
            |b| b.upload_new_overwrites = true,
            conflict_name_race,
        );
    }

    /// The user edits the note between the listing and its upload.
    fn edit_before_upload(w: &mut World) {
        shared_note(w, 0, "p.md");
        w.edit(0, "p.md", Edit::Append);
        w.hook(
            When::BeforeAction {
                dev: 0,
                kind: Some("uploadRevision"),
                path: None,
            },
            |w| {
                w.edit(0, "p.md", Edit::Append);
            },
        );
        sync(w, 0);
        quiesce(w);
    }

    #[test]
    fn bug_record_listing_hash_is_caught() {
        caught(
            "recorded hash does not match",
            |b| b.record_listing_hash = true,
            edit_before_upload,
        );
    }

    fn incomplete_listing(w: &mut World) {
        for p in ["a.md", "b.md", "c.md", "d.md"] {
            w.create(0, p);
        }
        quiesce(w);
        w.drives[0].next_listing = Some(drive::ListingMode::Incomplete);
        sync(w, 1);
        quiesce(w);
    }

    #[test]
    fn bug_trust_incomplete_listing_is_caught() {
        caught(
            "deleted after an incomplete listing",
            |b| b.trust_incomplete_listing = true,
            incomplete_listing,
        );
    }

    fn download_once(w: &mut World) {
        w.create(0, "a.md");
        quiesce(w);
    }

    #[test]
    fn bug_mtime_now_on_download_is_caught() {
        caught(
            "modification time",
            |b| b.mtime_now_on_download = true,
            download_once,
        );
    }

    #[test]
    fn sessions_can_be_driven_one_drive_at_a_time() {
        let mut w = World::new(2, &[&[0, 1]], 3);
        w.create(0, "a.md");
        let out = sync_session(&mut w, 0, 1).unwrap();
        assert_eq!(out.actions.len(), 1);
        assert!(w.drives[0].entry("a.md").is_none());
        assert!(w.drives[1].entry("a.md").is_some());
    }
}
