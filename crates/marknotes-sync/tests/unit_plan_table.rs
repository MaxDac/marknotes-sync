//! Unit tests for `plan.rs`: the branches of the sync plan in note-format.md, "Sync" > "Sync plan" that
//! `spec/fixtures/sync/plan.json` does not cover, and the properties of the plan as a whole (order, determinism).
//! Each test is named after the rule it checks and cites the spec. Tests marked "Characterisation" pin behaviour
//! the spec leaves open, so that any change to it is deliberate.

use marknotes_sync::plan::CLASH_MESSAGE;
use marknotes_sync::*;
use serde_json::{json, Value};

const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

// ---------------------------------------------------------------------------------------------------------------
// Helpers: entries, records and actions in the JSON form of the fixtures
// ---------------------------------------------------------------------------------------------------------------

fn l_file(path: &str, hash: &str) -> Value {
    json!({ "path": path, "isFolder": false, "modifiedMillis": 0, "sha256": hash })
}

fn l_dir(path: &str) -> Value {
    json!({ "path": path, "isFolder": true, "modifiedMillis": 0, "sha256": null })
}

fn r_file(path: &str, id: &str, version: &str) -> Value {
    json!({ "path": path, "id": id, "isFolder": false, "version": version, "modifiedMillis": 0 })
}

fn r_dir(path: &str, id: &str) -> Value {
    json!({ "path": path, "id": id, "isFolder": true, "version": "", "modifiedMillis": 0 })
}

fn rec_file(path: &str, id: &str, version: &str, hash: &str) -> Value {
    json!({ "path": path, "isFolder": false, "remoteId": id, "remoteVersion": version, "localHash": hash, "syncedAt": 1 })
}

fn rec_dir(path: &str, id: &str) -> Value {
    json!({ "path": path, "isFolder": true, "remoteId": id, "remoteVersion": "", "localHash": "", "syncedAt": 1 })
}

fn manifest(files: &[(&str, &str, &str)]) -> String {
    let files: serde_json::Map<String, Value> = files
        .iter()
        .map(|(p, sha, v)| {
            (
                p.to_string(),
                json!({ "sha256": sha, "version": v, "syncedAt": 1 }),
            )
        })
        .collect();
    json!({ "format": 1, "app": "MarkNotes", "deviceId": "other", "updatedAt": 1, "files": files })
        .to_string()
}

fn input(
    local: Vec<Value>,
    remote: Vec<Value>,
    records: Vec<Value>,
    manifest: Option<String>,
) -> SyncInput {
    serde_json::from_value(
        json!({ "local": local, "remote": remote, "records": records, "manifest": manifest }),
    )
    .unwrap()
}

fn plan_of(input: &SyncInput) -> Value {
    serde_json::to_value(plan(input)).unwrap()["actions"].clone()
}

fn plan_json(local: Vec<Value>, remote: Vec<Value>, records: Vec<Value>) -> Value {
    plan_of(&input(local, remote, records, None))
}

fn error(path: &str, message: &str) -> Value {
    json!({ "kind": "error", "path": path, "message": message })
}
fn record_folder(path: &str, id: &str) -> Value {
    json!({ "kind": "recordFolder", "path": path, "remoteId": id })
}
fn create_remote_folder(path: &str) -> Value {
    json!({ "kind": "createRemoteFolder", "path": path })
}
fn create_local_folder(path: &str, id: &str) -> Value {
    json!({ "kind": "createLocalFolder", "path": path, "remoteId": id })
}
fn forget(path: &str) -> Value {
    json!({ "kind": "forget", "path": path })
}
fn upload_new(path: &str) -> Value {
    json!({ "kind": "uploadNew", "path": path })
}
fn upload_revision(path: &str, remote: Value) -> Value {
    json!({ "kind": "uploadRevision", "path": path, "remote": remote })
}
fn download(path: &str, remote: Value) -> Value {
    json!({ "kind": "download", "path": path, "remote": remote })
}
fn record(path: &str, remote: Value, sha256: &str, update_base: bool) -> Value {
    json!({ "kind": "record", "path": path, "remote": remote, "sha256": sha256, "updateBase": update_base })
}
fn delete_local(path: &str) -> Value {
    json!({ "kind": "deleteLocal", "path": path })
}
fn delete_remote(path: &str, remote: Value) -> Value {
    json!({ "kind": "deleteRemote", "path": path, "remote": remote })
}
fn delete_local_folder(path: &str, forget: &[&str]) -> Value {
    json!({ "kind": "deleteLocalFolder", "path": path, "forget": forget })
}
fn delete_remote_folder(path: &str, remote: Value, forget: &[&str]) -> Value {
    json!({ "kind": "deleteRemoteFolder", "path": path, "remote": remote, "forget": forget })
}
fn reconcile_action(path: &str, remote: Value, has_record: bool) -> Value {
    json!({ "kind": "reconcile", "path": path, "remote": remote, "hasRecord": has_record })
}

fn attachment(ext: &str) -> String {
    format!("attachments/{}.{ext}", "a".repeat(64))
}

// ---------------------------------------------------------------------------------------------------------------
// Step 2: notes
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "Sync plan", step 2: only on the drive and changed (the drive id differs from the
/// record) is downloaded, not deleted on the drive.
#[test]
fn drive_only_note_with_a_new_drive_id_is_downloaded() {
    let r = r_file("a.md", "r2", "v1");
    assert_eq!(
        plan_json(
            vec![],
            vec![r.clone()],
            vec![rec_file("a.md", "r1", "v1", H1)]
        ),
        json!([download("a.md", r)])
    );
}

/// note-format.md, "Sync" > "Sync plan", step 2: a changed drive id counts as a drive change, so with a local edit
/// the note changed on both sides and is reconciled with its base.
#[test]
fn local_edit_and_new_drive_id_is_reconciled_with_the_base() {
    let r = r_file("a.md", "r2", "v1");
    assert_eq!(
        plan_json(
            vec![l_file("a.md", H2)],
            vec![r.clone()],
            vec![rec_file("a.md", "r1", "v1", H1)]
        ),
        json!([reconcile_action("a.md", r, true)])
    );
}

/// note-format.md, "Sync" > "Sync plan", step 2: changed on both sides, the manifest listing this exact drive
/// version with the local hash means it is just recorded (keeping the local bytes as the base).
#[test]
fn changed_on_both_with_the_local_hash_in_the_manifest_is_recorded() {
    let r = r_file("a.md", "r1", "v2");
    let plan = plan_of(&input(
        vec![l_file("a.md", H2)],
        vec![r.clone()],
        vec![rec_file("a.md", "r1", "v1", H1)],
        Some(manifest(&[("a.md", H2, "v2")])),
    ));
    assert_eq!(plan, json!([record("a.md", r, H2, true)]));
}

/// note-format.md, "Sync" > "Sync plan", step 2: the manifest only counts for this exact drive version; a hash
/// listed for another version leaves a drive change a download.
#[test]
fn drive_change_with_the_recorded_hash_under_another_version_is_downloaded() {
    let r = r_file("a.md", "r1", "v3");
    let plan = plan_of(&input(
        vec![l_file("a.md", H1)],
        vec![r.clone()],
        vec![rec_file("a.md", "r1", "v1", H1)],
        Some(manifest(&[("a.md", H1, "v2")])),
    ));
    assert_eq!(plan, json!([download("a.md", r)]));
}

/// note-format.md, "Sync" > "Sync plan", step 2: the manifest is keyed by path, so a matching version and hash
/// under another path does not count.
#[test]
fn manifest_entry_for_another_path_is_not_used() {
    let r = r_file("a.md", "r1", "v2");
    let plan = plan_of(&input(
        vec![l_file("a.md", H1)],
        vec![r.clone()],
        vec![rec_file("a.md", "r1", "v1", H1)],
        Some(manifest(&[("A.md", H1, "v2"), ("b.md", H1, "v2")])),
    ));
    assert_eq!(plan, json!([download("a.md", r)]));
}

/// note-format.md, "Sync" > "Sync plan", step 2: deleted locally while the drive has a new version (even one the
/// manifest says holds the recorded content) is a change that wins over the deletion: downloaded.
#[test]
fn deleted_locally_while_the_drive_re_uploaded_the_same_content_is_downloaded() {
    let r = r_file("a.md", "r1", "v2");
    let plan = plan_of(&input(
        vec![],
        vec![r.clone()],
        vec![rec_file("a.md", "r1", "v1", H1)],
        Some(manifest(&[("a.md", H1, "v2")])),
    ));
    assert_eq!(plan, json!([download("a.md", r)]));
}

/// note-format.md, "Sync" > "Sync plan", step 2: a drive file replaced with a new id whose manifest hash equals the
/// recorded hash is only recorded, and a local edit is then uploaded as a revision of the new drive file.
#[test]
fn replaced_drive_file_with_our_content_is_recorded_then_the_edit_uploaded() {
    let r = r_file("a.md", "r2", "v1");
    let plan = plan_of(&input(
        vec![l_file("a.md", H2)],
        vec![r.clone()],
        vec![rec_file("a.md", "r1", "v1", H1)],
        Some(manifest(&[("a.md", H1, "v1")])),
    ));
    assert_eq!(
        plan,
        json!([
            record("a.md", r.clone(), H1, false),
            upload_revision("a.md", r)
        ])
    );
}

/// note-format.md, "Sync" > "Sync plan", step 2: a drive revision with our content and no local edit is only
/// recorded, without updating the base (whatever the manifest says about other files).
#[test]
fn unchanged_local_and_drive_revision_with_our_content_is_only_recorded() {
    let r = r_file("a.md", "r1", "v2");
    let plan = plan_of(&input(
        vec![l_file("a.md", H1)],
        vec![r.clone()],
        vec![rec_file("a.md", "r1", "v1", H1)],
        Some(manifest(&[("a.md", H1, "v2"), ("b.md", H2, "v9")])),
    ));
    assert_eq!(plan, json!([record("a.md", r, H1, false)]));
}

/// note-format.md, "What is synced": notes end in `.md` in any case, so `.MD` notes are synced like the others.
#[test]
fn notes_with_an_upper_case_extension_are_synced() {
    let r = r_file("B.MD", "rb", "v1");
    assert_eq!(
        plan_json(
            vec![l_file("A.Md", H1), l_file("B.MD", H1)],
            vec![r.clone()],
            vec![]
        ),
        json!([upload_new("A.Md"), reconcile_action("B.MD", r, false)])
    );
}

/// note-format.md, "Sync" > "Sync plan", step 2: a record left for a path on neither side is dropped, for notes
/// and attachments alike.
#[test]
fn file_records_on_neither_side_are_dropped() {
    let att = attachment("png");
    assert_eq!(
        plan_json(
            vec![],
            vec![],
            vec![
                rec_file("x.md", "r1", "v1", H1),
                rec_file(&att, "p", "v1", H1)
            ]
        ),
        json!([forget(&att), forget("x.md")])
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Step 2: attachments
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Attachment sync": attachments sync like notes; a new one on the drive is downloaded.
#[test]
fn new_drive_attachment_is_downloaded() {
    let att = attachment("png");
    let r = r_file(&att, "p", "v1");
    assert_eq!(
        plan_json(vec![], vec![r.clone()], vec![]),
        json!([download(&att, r)])
    );
}

/// note-format.md, "Attachment sync": "deletions propagate in both directions, like notes": deleted on the drive
/// is deleted locally.
#[test]
fn attachment_deleted_on_the_drive_is_deleted_locally() {
    let att = attachment("jpg");
    assert_eq!(
        plan_json(
            vec![l_file(&att, H1)],
            vec![],
            vec![rec_file(&att, "p", "v1", H1)]
        ),
        json!([delete_local(&att)])
    );
}

/// note-format.md, "Attachment sync": deleted locally while the drive has a new revision of it is downloaded (a
/// change wins over a deletion).
#[test]
fn attachment_deleted_locally_with_a_new_drive_revision_is_downloaded() {
    let att = attachment("gif");
    let r = r_file(&att, "p", "v2");
    assert_eq!(
        plan_json(vec![], vec![r.clone()], vec![rec_file(&att, "p", "v1", H1)]),
        json!([download(&att, r)])
    );
}

/// note-format.md, "Attachment sync": an attachment unchanged on both sides gets no action.
#[test]
fn unchanged_attachment_gets_no_action() {
    let att = attachment("webp");
    assert_eq!(
        plan_json(
            vec![l_file(&att, H1)],
            vec![r_file(&att, "p", "v1")],
            vec![rec_file(&att, "p", "v1", H1)]
        ),
        json!([])
    );
}

/// Characterisation (the spec says attachments are "only uploaded, downloaded or deleted", and that on both sides
/// they are recorded "(their name is their content)", but not what happens when only the local hash moved): the
/// record is refreshed with the new local hash, nothing is uploaded.
#[test]
fn attachment_with_only_a_new_local_hash_is_recorded() {
    let att = attachment("png");
    let r = r_file(&att, "p", "v1");
    assert_eq!(
        plan_json(
            vec![l_file(&att, H2)],
            vec![r.clone()],
            vec![rec_file(&att, "p", "v1", H1)]
        ),
        json!([record(&att, r, H2, false)])
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Step 1: folders
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "Sync plan", step 1: a folder on both sides is recorded if not yet recorded with that
/// drive id, so a recorded folder whose drive id changed is recorded again.
#[test]
fn folder_recorded_with_another_drive_id_is_recorded_again() {
    assert_eq!(
        plan_json(
            vec![l_dir("A")],
            vec![r_dir("A", "f2")],
            vec![rec_dir("A", "f1")]
        ),
        json!([record_folder("A", "f2")])
    );
}

/// note-format.md, "Sync" > "Sync plan", step 1: a recorded folder only local is created on the drive when it still
/// holds something to sync, such as a new subfolder; the subfolder is then created too.
#[test]
fn local_folder_deleted_on_the_drive_is_kept_for_a_new_subfolder() {
    assert_eq!(
        plan_json(
            vec![l_dir("A"), l_dir("A/N")],
            vec![],
            vec![rec_dir("A", "fA")]
        ),
        json!([create_remote_folder("A"), create_remote_folder("A/N")])
    );
}

/// note-format.md, "Sync" > "Sync plan", step 1: a recorded folder only on the drive is created locally when it
/// still holds something to sync, such as a new drive note, which is then downloaded.
#[test]
fn drive_folder_deleted_locally_is_kept_for_a_new_drive_note() {
    let r = r_file("A/new.md", "rn", "v1");
    assert_eq!(
        plan_json(
            vec![],
            vec![r_dir("A", "fA"), r.clone()],
            vec![rec_dir("A", "fA")]
        ),
        json!([create_local_folder("A", "fA"), download("A/new.md", r)])
    );
}

/// note-format.md, "Sync" > "Sync plan", step 1: ignored entries keep every folder above them, however deep, on
/// either side; the synced contents are handled one by one (an unchanged note deleted on the other side is deleted).
#[test]
fn deeply_nested_ignored_entries_keep_the_whole_folder_chain() {
    let local = plan_json(
        vec![
            l_dir("A"),
            l_dir("A/B"),
            l_dir("A/B/C"),
            l_dir("A/B/C/.obsidian"),
            l_file("A/B/x.md", H1),
        ],
        vec![],
        vec![
            rec_dir("A", "fA"),
            rec_dir("A/B", "fB"),
            rec_dir("A/B/C", "fC"),
            rec_file("A/B/x.md", "r1", "v1", H1),
        ],
    );
    assert_eq!(
        local,
        json!([
            create_remote_folder("A"),
            create_remote_folder("A/B"),
            create_remote_folder("A/B/C"),
            delete_local("A/B/x.md"),
        ])
    );
    let x = r_file("A/B/x.md", "r1", "v1");
    let drive = plan_json(
        vec![],
        vec![
            r_dir("A", "fA"),
            r_dir("A/B", "fB"),
            r_dir("A/B/C", "fC"),
            r_file("A/B/C/report.pdf", "pdf", "v1"),
            x.clone(),
        ],
        vec![
            rec_dir("A", "fA"),
            rec_dir("A/B", "fB"),
            rec_dir("A/B/C", "fC"),
            rec_file("A/B/x.md", "r1", "v1", H1),
        ],
    );
    assert_eq!(
        drive,
        json!([
            create_local_folder("A", "fA"),
            create_local_folder("A/B", "fB"),
            create_local_folder("A/B/C", "fC"),
            delete_remote("A/B/x.md", x),
        ])
    );
}

/// note-format.md, "What is synced" (the synced folder itself is the root, never an entry): an entry or record with
/// the empty path is not synced; a root entry in a listing gets no action.
#[test]
fn root_entries_get_no_action() {
    assert_eq!(
        plan_json(vec![l_dir("")], vec![r_dir("", "root")], vec![]),
        json!([])
    );
}

/// Characterisation (the spec does not say what happens to records of paths that are not synced): records are not
/// checked against the path rules, so a record for an invalid or ignored path, the root included, is on neither
/// side and is dropped, in path order.
#[test]
fn records_of_paths_that_are_not_synced_are_dropped() {
    let plan = plan_json(
        vec![l_file("notes.txt", H1), l_dir(".hidden")],
        vec![],
        vec![
            rec_file("notes.txt", "t", "v1", H1),
            rec_file("a//b.md", "e", "v1", H1),
            rec_file(".hidden.md", "h", "v1", H1),
            rec_file("../x.md", "u", "v1", H1),
            rec_file(" a.md", "s", "v1", H1),
            rec_file("a\\b.md", "b", "v1", H1),
            rec_file("attachments/readme.md", "n", "v1", H1),
            rec_dir("", "root"),
            rec_dir(".hidden", "hd"),
        ],
    );
    assert_eq!(
        plan,
        json!([
            forget(""),
            forget(".hidden"),
            forget(" a.md"),
            forget("../x.md"),
            forget(".hidden.md"),
            forget("a//b.md"),
            forget("a\\b.md"),
            forget("attachments/readme.md"),
            forget("notes.txt"),
        ])
    );
}

// ---------------------------------------------------------------------------------------------------------------
// File and folder types disagreeing between the listings and the records
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "What is synced": a path that is a file on one side and a folder on the other is an error and
/// skipped, also when one side is new (no record) and whichever side holds the folder.
#[test]
fn file_folder_clash_without_records_is_an_error_either_way() {
    assert_eq!(
        plan_json(vec![l_file("a.md", H1)], vec![r_dir("a.md", "f")], vec![]),
        json!([error("a.md", CLASH_MESSAGE)])
    );
    assert_eq!(
        plan_json(vec![l_dir("a.md")], vec![r_file("a.md", "r", "v1")], vec![]),
        json!([error("a.md", CLASH_MESSAGE)])
    );
}

/// Characterisation (the spec's clash rule is about the two sides, not the records): a folder record for a path
/// that is now a note on both sides is dropped, and the note, having no file record, is reconciled without a base.
#[test]
fn folder_record_for_a_note_on_both_sides_is_dropped_and_the_note_reconciled() {
    let r = r_file("A.md", "r", "v1");
    assert_eq!(
        plan_json(
            vec![l_file("A.md", H1)],
            vec![r.clone()],
            vec![rec_dir("A.md", "f")]
        ),
        json!([forget("A.md"), reconcile_action("A.md", r, false)])
    );
}

/// Characterisation (as above): a folder record for a path that is now a local note is dropped and the note,
/// having no file record, is uploaded as new.
#[test]
fn folder_record_for_a_local_note_is_dropped_and_the_note_uploaded() {
    assert_eq!(
        plan_json(vec![l_file("x.md", H1)], vec![], vec![rec_dir("x.md", "f")]),
        json!([forget("x.md"), upload_new("x.md")])
    );
}

/// Characterisation (as above): a file record for a path that is now a folder on both sides gives a folder record,
/// then the file record is dropped. As the executor applies them in order, the `forget` drops the folder record
/// just saved; the next sync records the folder again.
#[test]
fn file_record_for_a_folder_on_both_sides_records_it_then_forgets() {
    assert_eq!(
        plan_json(
            vec![l_dir("A")],
            vec![r_dir("A", "fA")],
            vec![rec_file("A", "r", "v1", H1)]
        ),
        json!([record_folder("A", "fA"), forget("A")])
    );
}

/// Characterisation (as above): a file record for a path that is now a local-only folder: the folder has no folder
/// record, so it is created on the drive, then the file record is dropped.
#[test]
fn file_record_for_a_local_folder_creates_it_then_forgets() {
    assert_eq!(
        plan_json(vec![l_dir("F")], vec![], vec![rec_file("F", "r", "v1", H1)]),
        json!([create_remote_folder("F"), forget("F")])
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Order of the plan
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "What is synced": errors come first, one per path, in UTF-16 code-unit order (a supplementary
/// character, with surrogates from 0xD800, before U+FF5E), before every other action.
#[test]
fn errors_come_first_in_utf16_order() {
    let plan = plan_json(
        vec![
            l_file("0.md", H1),
            l_dir("\u{FF5E}.md"),
            l_dir("\u{1F600}.md"),
            l_dir("b.md"),
        ],
        vec![
            r_file("\u{FF5E}.md", "x", "v1"),
            r_file("\u{1F600}.md", "y", "v1"),
            r_file("b.md", "z", "v1"),
        ],
        vec![],
    );
    assert_eq!(
        plan,
        json!([
            error("b.md", CLASH_MESSAGE),
            error("\u{1F600}.md", CLASH_MESSAGE),
            error("\u{FF5E}.md", CLASH_MESSAGE),
            upload_new("0.md"),
        ])
    );
}

/// note-format.md, "Sync" > "Sync plan", step 1: folders parents first, by depth then path, across creations on
/// both sides; files come after all folders.
#[test]
fn folders_are_planned_by_depth_then_path_before_files() {
    let plan = plan_json(
        vec![
            l_file("A/B/C/n.md", H1),
            l_dir("A/B/C"),
            l_dir("Z/Y"),
            l_dir("A/B"),
            l_dir("Z"),
            l_dir("A"),
        ],
        vec![r_dir("R/S/T", "t"), r_dir("R", "r"), r_dir("R/S", "s")],
        vec![],
    );
    assert_eq!(
        plan,
        json!([
            create_remote_folder("A"),
            create_local_folder("R", "r"),
            create_remote_folder("Z"),
            create_remote_folder("A/B"),
            create_local_folder("R/S", "s"),
            create_remote_folder("Z/Y"),
            create_remote_folder("A/B/C"),
            create_local_folder("R/S/T", "t"),
            upload_new("A/B/C/n.md"),
        ])
    );
}

/// Known files are individually guarded, then empty folders are deleted deepest-first, forgetting only themselves.
#[test]
fn folder_deletions_are_last_and_deepest_first() {
    let q = r_dir("Q", "fQ");
    let plan = plan_json(
        vec![
            l_dir("A"),
            l_dir("A/B"),
            l_dir("A/B/C"),
            l_file("A/x.md", H1),
            l_file("A/B/C/y.md", H1),
            l_file("z.md", H1),
        ],
        vec![
            q.clone(),
            r_dir("Q/R", "fR"),
            r_file("Q/R/q.md", "rq", "v1"),
        ],
        vec![
            rec_dir("A", "fA"),
            rec_dir("A/B", "fB"),
            rec_dir("A/B/C", "fC"),
            rec_file("A/x.md", "rx", "v1", H1),
            rec_file("A/B/C/y.md", "ry", "v1", H1),
            rec_dir("Q", "fQ"),
            rec_dir("Q/R", "fR"),
            rec_file("Q/R/q.md", "rq", "v1", H1),
        ],
    );
    assert_eq!(
        plan,
        json!([
            delete_local("A/B/C/y.md"),
            delete_local("A/x.md"),
            delete_remote("Q/R/q.md", r_file("Q/R/q.md", "rq", "v1")),
            upload_new("z.md"),
            delete_local_folder("A/B/C", &["A/B/C"]),
            delete_local_folder("A/B", &["A/B"]),
            delete_local_folder("A", &["A"]),
            delete_remote_folder("Q/R", r_dir("Q/R", "fR"), &["Q/R"]),
            delete_remote_folder("Q", q, &["Q"]),
        ])
    );
}

/// Local empty-folder deletions precede remote ones; each side is deepest-first, then path.
#[test]
fn local_folder_deletions_come_before_drive_ones() {
    let a = r_dir("A", "fA");
    let plan = plan_json(
        vec![l_dir("K"), l_dir("K/Z"), l_dir("M")],
        vec![a.clone(), r_dir("K", "fK")],
        vec![
            rec_dir("A", "fA"),
            rec_dir("K", "fK"),
            rec_dir("K/Z", "fZ"),
            rec_dir("M", "fM"),
        ],
    );
    assert_eq!(
        plan,
        json!([
            delete_local_folder("K/Z", &["K/Z"]),
            delete_local_folder("M", &["M"]),
            delete_remote_folder("A", a, &["A"]),
        ])
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------------------------------------------

fn rich_lists() -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let local = vec![
        l_dir("A"),
        l_dir("A/B"),
        l_file("A/B/n.md", H1),
        l_file("A/edited.md", H2),
        l_dir("Old"),
        l_file("Old/o.md", H1),
        l_file("new.md", H1),
        l_file("both.md", H2),
        l_dir("clash.md"),
        l_file("notes.txt", H1),
        l_file(&attachment("png"), H1),
        l_file("Case.md", H1),
        l_file("case.md", H2),
    ];
    let remote = vec![
        r_dir("A", "fA"),
        r_file("A/edited.md", "re", "v1"),
        r_dir("D", "fD"),
        r_file("D/d.md", "rd", "v1"),
        r_file("both.md", "rb", "v2"),
        r_file("clash.md", "rc", "v1"),
        r_file("gone-local.md", "rg", "v1"),
        r_file(&attachment("png"), "p", "v1"),
        r_file("\u{1F600}.md", "e1", "v1"),
        r_file("\u{FF5E}.md", "e2", "v1"),
    ];
    let records = vec![
        rec_dir("A", "fA"),
        rec_file("A/edited.md", "re", "v1", H1),
        rec_dir("Old", "fO"),
        rec_file("Old/o.md", "ro", "v1", H1),
        rec_file("both.md", "rb", "v1", H1),
        rec_file("gone-local.md", "rg", "v1", H1),
        rec_file("forgotten.md", "rf", "v1", H1),
    ];
    (local, remote, records)
}

/// note-format.md, "Sync" > "Sync plan" (an ordered plan, by depth and path): the plan does not depend on the order
/// of the listings or the records.
#[test]
fn plan_is_the_same_for_any_order_of_the_inputs() {
    let (local, remote, records) = rich_lists();
    let expected = plan_json(local.clone(), remote.clone(), records.clone());
    assert!(expected.as_array().unwrap().len() > 10);
    let permutations: Vec<fn(&mut Vec<Value>)> = vec![
        |v| v.reverse(),
        |v| v.rotate_left(1),
        |v| {
            let half = v.len() / 2;
            v.rotate_left(half)
        },
        |v| {
            let (even, odd): (Vec<_>, Vec<_>) =
                v.drain(..).enumerate().partition(|(i, _)| i % 2 == 0);
            v.extend(odd.into_iter().chain(even).map(|(_, x)| x));
        },
        |v| {
            v.sort_by_key(|e| {
                e["path"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .rev()
                    .collect::<String>()
            })
        },
    ];
    for (i, p) in permutations.iter().enumerate() {
        for (j, q) in permutations.iter().enumerate() {
            let (mut l, mut r, mut rec) = rich_lists();
            p(&mut l);
            q(&mut r);
            p(&mut rec);
            assert_eq!(plan_json(l, r, rec), expected, "permutations {i} and {j}");
        }
    }
}
