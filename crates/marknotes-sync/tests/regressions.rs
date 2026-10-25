//! Regression and characterisation tests for the defects found while hardening the tests for the F-Droid release
//! (see TESTING.md, "Defect register"). A regression test pins a fix; a characterisation test pins behaviour that
//! the spec allows or leaves open, so that any change to it is deliberate.

use marknotes_sync::manifest::{build_manifest, parse_manifest};
use marknotes_sync::merge::MergeChunk;
use marknotes_sync::plan::{CASE_MESSAGE, CLASH_MESSAGE, DUPLICATE_MESSAGE};
use marknotes_sync::*;
use serde_json::{json, Value};

/// Plans `input`, filling in the fields these tests don't care about (the bindings require them all).
fn plan_json(mut input: Value) -> Value {
    let fill = |entries: Option<&mut Value>, defaults: Value| {
        for entry in entries.and_then(Value::as_array_mut).into_iter().flatten() {
            for (key, value) in defaults.as_object().unwrap() {
                entry
                    .as_object_mut()
                    .unwrap()
                    .entry(key.clone())
                    .or_insert(value.clone());
            }
        }
    };
    fill(
        input.get_mut("remote"),
        json!({ "version": "", "modifiedMillis": 0 }),
    );
    fill(
        input.get_mut("records"),
        json!({ "remoteVersion": "", "localHash": "", "syncedAt": 0 }),
    );
    let input: SyncInput = serde_json::from_value(input).unwrap();
    serde_json::to_value(plan(&input)).unwrap()["actions"].clone()
}

fn error(path: &str, message: &str) -> Value {
    json!({ "kind": "error", "path": path, "message": message })
}

// D1: conflicted-copy names are compared by simple case folding, like sync paths.

#[test]
fn d1_conflict_copy_name_folds_long_s() {
    assert_eq!(
        paths::conflict_copy_name("ſ.md", &["s (conflicted copy).md".into()]),
        "ſ (conflicted copy) 2.md"
    );
}

#[test]
fn d1_conflict_copy_name_folds_final_sigma() {
    assert_eq!(
        paths::conflict_copy_name("Σ.md", &["ς (conflicted copy).md".into()]),
        "Σ (conflicted copy) 2.md"
    );
}

#[test]
fn d1_conflict_copy_name_folds_in_the_folder_too() {
    assert_eq!(
        paths::conflict_copy_name("Work/a.md", &["WORK/A (Conflicted Copy).md".into()]),
        "Work/a (conflicted copy) 2.md"
    );
}

// D2: a folder named like a note that is a file on the drive. Only the clashing path is an error; what is inside the
// local folder is handled on its own (here the drive no longer has it, so it is deleted locally, as the spec says
// for an unchanged file the drive deleted).

#[test]
fn d2_file_folder_clash_errors_only_the_clashing_path() {
    let actions = plan_json(json!({
        "local": [
            { "path": "A.md", "isFolder": true },
            { "path": "A.md/x.md", "isFolder": false, "sha256": "h1" }
        ],
        "remote": [{ "path": "A.md", "id": "f", "isFolder": false, "version": "v1" }],
        "records": [
            { "path": "A.md", "isFolder": true, "remoteId": "fA" },
            { "path": "A.md/x.md", "isFolder": false, "remoteId": "r1", "remoteVersion": "v1", "localHash": "h1" }
        ]
    }));
    assert_eq!(actions[0], error("A.md", CLASH_MESSAGE));
    assert_eq!(
        actions.as_array().unwrap()[1..],
        [json!({ "kind": "deleteLocal", "path": "A.md/x.md" })]
    );
}

// D3: a path listed or recorded more than once is an error, whatever the order of the entries.

#[test]
fn d3_duplicate_local_entry_is_an_error_in_any_order() {
    for (first, second) in [("h1", "h2"), ("h2", "h1")] {
        let actions = plan_json(json!({
            "local": [
                { "path": "a.md", "isFolder": false, "sha256": first },
                { "path": "a.md", "isFolder": false, "sha256": second }
            ],
            "remote": [{ "path": "a.md", "id": "r", "isFolder": false, "version": "v1" }],
            "records": [{ "path": "a.md", "isFolder": false, "remoteId": "r", "remoteVersion": "v1", "localHash": "h1" }]
        }));
        assert_eq!(actions, json!([error("a.md", DUPLICATE_MESSAGE)]));
    }
}

#[test]
fn d3_duplicate_remote_folder_skips_everything_inside() {
    let actions = plan_json(json!({
        "local": [],
        "remote": [
            { "path": "A", "id": "f1", "isFolder": true },
            { "path": "A", "id": "f2", "isFolder": true },
            { "path": "A/x.md", "id": "r1", "isFolder": false, "version": "v1" }
        ],
        "records": []
    }));
    assert_eq!(
        actions,
        json!([
            error("A", DUPLICATE_MESSAGE),
            error("A/x.md", DUPLICATE_MESSAGE)
        ])
    );
}

#[test]
fn d3_paths_equal_by_case_folding_are_a_case_error_not_a_duplicate() {
    let actions = plan_json(json!({
        "local": [
            { "path": "a.md", "isFolder": false, "sha256": "h1" },
            { "path": "A.md", "isFolder": false, "sha256": "h2" }
        ],
        "remote": [],
        "records": []
    }));
    assert_eq!(
        actions,
        json!([error("A.md", CASE_MESSAGE), error("a.md", CASE_MESSAGE)])
    );
}

#[test]
fn d3_build_manifest_keeps_the_last_of_duplicate_records() {
    let record = |hash: &str| SyncRecord {
        path: "a.md".into(),
        is_folder: false,
        remote_id: "r".into(),
        remote_version: "v".into(),
        local_hash: hash.into(),
        synced_at: 1,

        local_deleted: false,

        remote_folder_preserved: false,
    };
    let text = build_manifest(&[record("h1"), record("h2")], "d", 5, None).unwrap();
    assert_eq!(parse_manifest(&text).unwrap().files["a.md"].sha256, "h2");
}

// D4: a folder a listing omits although it lists entries inside it is not deleted.

#[test]
fn d4_folder_missing_from_the_drive_listing_is_not_deleted_locally() {
    let actions = plan_json(json!({
        "local": [
            { "path": "A", "isFolder": true },
            { "path": "A/x.md", "isFolder": false, "sha256": "h1" }
        ],
        "remote": [{ "path": "A/x.md", "id": "r1", "isFolder": false, "version": "v1" }],
        "records": [
            { "path": "A", "isFolder": true, "remoteId": "fA" },
            { "path": "A/x.md", "isFolder": false, "remoteId": "r1", "remoteVersion": "v1", "localHash": "h1" }
        ]
    }));
    assert_eq!(actions, json!([]));
}

#[test]
fn d4_folder_missing_from_the_local_listing_is_not_deleted_on_the_drive() {
    let actions = plan_json(json!({
        "local": [{ "path": "A/x.md", "isFolder": false, "sha256": "h1" }],
        "remote": [
            { "path": "A", "id": "fA", "isFolder": true },
            { "path": "A/x.md", "id": "r1", "isFolder": false, "version": "v1" }
        ],
        "records": [
            { "path": "A", "isFolder": true, "remoteId": "fA" },
            { "path": "A/x.md", "isFolder": false, "remoteId": "r1", "remoteVersion": "v1", "localHash": "h1" }
        ]
    }));
    assert_eq!(actions, json!([]));
}

#[test]
fn d4_implied_folder_with_no_record_is_neither_created_nor_recorded() {
    let actions = plan_json(json!({
        "local": [],
        "remote": [{ "path": "A/x.md", "id": "r1", "isFolder": false, "version": "v1" }],
        "records": []
    }));
    let kinds: Vec<&str> = actions
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["download"]);
}

#[test]
fn d4_ignored_entries_also_imply_their_folder() {
    let actions = plan_json(json!({
        "local": [
            { "path": "A", "isFolder": true },
            { "path": "A/x.md", "isFolder": false, "sha256": "h1" }
        ],
        "remote": [
            { "path": "A/x.md", "id": "r1", "isFolder": false, "version": "v1" },
            { "path": "A/photo.png", "id": "p", "isFolder": false, "version": "v1" }
        ],
        "records": [
            { "path": "A", "isFolder": true, "remoteId": "fA" },
            { "path": "A/x.md", "isFolder": false, "remoteId": "r1", "remoteVersion": "v1", "localHash": "h1" }
        ]
    }));
    assert_eq!(actions, json!([]));
}

// D5: a UTF-8 BOM before the front matter (MaxDac/marknotes-spec#33). The block is recognised after the BOM, which
// stays first.

#[test]
fn d5_front_matter_is_recognised_after_a_bom() {
    let text = "\u{feff}---\nid: k7f3q2xa\n---\nBody";
    assert_eq!(front_matter::read_id(text).as_deref(), Some("k7f3q2xa"));
    assert_eq!(
        front_matter::split(text),
        ("\u{feff}---\nid: k7f3q2xa\n---\n", "Body")
    );
    assert_eq!(
        front_matter::with_id(text, "m4n5p6q7"),
        "\u{feff}---\nid: m4n5p6q7\n---\nBody"
    );
    assert_eq!(
        front_matter::with_id("\u{feff}Body", "m4n5p6q7"),
        "\u{feff}---\nid: m4n5p6q7\n---\nBody"
    );
    assert_eq!(front_matter::split("\u{feff}Body"), ("", "\u{feff}Body"));
}

// D6: names with control characters (MaxDac/marknotes-spec#34) are not valid, so they are ignored on both sides.

#[test]
fn d6_control_characters_make_a_name_invalid() {
    for path in ["a\0b.md", "a\nb.md", "a\rb.md", "A\tB/x.md", "a\u{7f}.md"] {
        assert!(!paths::is_syncable_path(path), "{path:?}");
    }
    let actions = plan_json(json!({
        "local": [{ "path": "a\nb.md", "isFolder": false, "sha256": "h1" }],
        "remote": [{ "path": "c\u{0}d.md", "id": "r", "isFolder": false, "version": "v1" }],
        "records": []
    }));
    assert_eq!(actions, json!([]));
}

// D10: above the maximum edit distance the inputs are treated as completely different. The merge stays bounded and
// keeps every line of both sides.

#[test]
fn d10_merge_beyond_the_edit_distance_bound_keeps_every_line() {
    let local: String = (0..1500).map(|i| format!("local {i}\n")).collect();
    let remote: String = (0..1500).map(|i| format!("remote {i}\n")).collect();
    let result = merge3(None, &local, &remote);
    assert_eq!(result.conflicts, 1);
    match &result.chunks[..] {
        [MergeChunk::Conflict {
            local: l,
            remote: r,
            ..
        }] => {
            assert_eq!(l.concat(), local);
            assert_eq!(r.concat(), remote);
        }
        other => panic!("unexpected chunks {other:?}"),
    }
}

#[test]
fn d10_merge_beyond_the_bound_with_one_side_unchanged_takes_the_other() {
    let base: String = (0..3000).map(|i| format!("base {i}\n")).collect();
    let local: String = (0..3000).map(|i| format!("local {i}\n")).collect();
    let result = merge3(Some(&base), &local, &base);
    assert_eq!(result.conflicts, 0);
    assert_eq!(result.text, local);
}

// D11: without a base the merge is not symmetric; the spec fixes the order: this device's text first, then the
// drive's (MaxDac/marknotes-spec#35). Both orders keep every line.

#[test]
fn d11_merge_without_base_is_not_symmetric() {
    let (a, b) = ("a\na\nb\n", "b\na\nb\na\na\n");
    let ab = merge3(None, a, b);
    let ba = merge3(None, b, a);
    assert_eq!((ab.conflicts, ba.conflicts), (3, 2));
    for result in [&ab, &ba] {
        for line in a.lines().chain(b.lines()) {
            assert!(result.text.contains(line));
        }
    }
}

// D12: manifest timestamps written as fractional or huge numbers are truncated toward zero and saturate.

#[test]
fn d12_manifest_millis_are_truncated_and_saturate() {
    let m = parse_manifest(
        r#"{"format":1,"deviceId":"d","updatedAt":1.9,"files":{"a.md":{"sha256":"x","version":"1","syncedAt":1e300}}}"#,
    )
    .unwrap();
    assert_eq!(m.updated_at, 1);
    assert_eq!(m.files["a.md"].synced_at, i64::MAX);
    let m = parse_manifest(r#"{"format":1,"deviceId":"d","updatedAt":-1e300,"files":{}}"#).unwrap();
    assert_eq!(m.updated_at, i64::MIN);
}

/// D14 (found by the FM-ID property): "Any other front-matter keys are kept verbatim". `with_id` rebuilt the whole
/// block with the opening fence's line ending, so a key line ending in a carriage return in an LF block lost it,
/// mixed line endings were normalised, and fence lines lost trailing spaces. It now only replaces or inserts the
/// `id` line.
#[test]
fn d14_with_id_keeps_the_rest_of_the_block_byte_for_byte() {
    assert_eq!(
        front_matter::with_id("---\ntrailing \r\r\n---\n", "00000000"),
        "---\nid: 00000000\ntrailing \r\r\n---\n"
    );
    assert_eq!(
        front_matter::with_id("---\r\ntitle: a\n---  \nBody", "00000000"),
        "---\r\nid: 00000000\r\ntitle: a\n---  \nBody"
    );
}

// D16: simple case folding includes the S mappings that lowercasing misses.

#[test]
fn d16_case_folding_uses_the_s_mappings() {
    assert_eq!(
        paths::fold_case("\u{FB05}.md"),
        paths::fold_case("\u{FB06}.md")
    );
    assert_eq!(
        paths::conflict_copy_name("\u{1FD3}.md", &["\u{0390} (conflicted copy).md".into()]),
        "\u{1FD3} (conflicted copy) 2.md"
    );
}

// D19 (#18): a defaulted input field given as null (or undefined from JS) gets its default, as if omitted; any other
// value must still have the right type.

#[test]
fn d19_null_defaulted_fields_mean_omitted() {
    let folder =
        json!({ "path": "F", "id": "f", "isFolder": true, "version": "", "modifiedMillis": 0 });
    let record = json!({ "path": "F", "isFolder": true, "remoteId": "f", "remoteVersion": "", "localHash": "", "syncedAt": 0 });
    let input = |extra: Value| {
        let mut input = json!({ "local": [], "remote": [folder], "records": [record] });
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value::<SyncInput>(input)
    };
    for absent in [json!({}), json!({ "remoteFolderDeletion": null })] {
        assert_eq!(
            input(absent).unwrap().remote_folder_deletion,
            RemoteFolderDeletion::AtomicEmptyOnly
        );
    }
    assert_eq!(
        input(json!({ "remoteFolderDeletion": "atomicEmptyOnly" }))
            .unwrap()
            .remote_folder_deletion,
        RemoteFolderDeletion::AtomicEmptyOnly
    );
    assert_eq!(
        input(json!({ "remoteFolderDeletion": "unsupported" }))
            .unwrap()
            .remote_folder_deletion,
        RemoteFolderDeletion::Unsupported
    );
    for bad in [
        json!("recursive"),
        json!(""),
        json!("Unsupported"),
        json!(true),
        json!(1),
    ] {
        assert!(input(json!({ "remoteFolderDeletion": bad })).is_err());
    }

    let record = serde_json::from_value::<SyncRecord>(json!({
        "path": "F", "isFolder": true, "remoteId": "f", "remoteVersion": "", "localHash": "", "syncedAt": 0,
        "localDeleted": null, "remoteFolderPreserved": null,
    }))
    .unwrap();
    assert!(!record.local_deleted && !record.remote_folder_preserved);
    let local = serde_json::from_value::<LocalEntry>(
        json!({ "path": "F", "isFolder": true, "modifiedMillis": null }),
    )
    .unwrap();
    assert_eq!(local.modified_millis, 0);
    let reconcile = serde_json::from_value::<ReconcileInput>(json!({
        "path": "a.md", "local": "", "localModifiedMillis": 0, "remote": "",
        "remoteEntry": { "path": "a.md", "id": "r", "isFolder": false, "version": "v", "modifiedMillis": 0 },
        "existingPaths": null, "newId": "00000000",
    }))
    .unwrap();
    assert!(reconcile.existing_paths.is_empty());
    for (key, bad) in [
        ("localDeleted", json!("true")),
        ("remoteFolderPreserved", json!(0)),
    ] {
        let mut value = json!({ "path": "F", "isFolder": true, "remoteId": "f", "remoteVersion": "", "localHash": "", "syncedAt": 0 });
        value[key] = bad;
        assert!(
            serde_json::from_value::<SyncRecord>(value).is_err(),
            "{key}"
        );
    }
}

// D20: the first nightly (seed 1) found `preserveRemoteFolder` after a local folder deletion. It replaces
// `deleteRemoteFolder` in the deepest-first folder deletions (spec, "Sync"), so this order is right and the property
// was stale; pinned here.

#[test]
fn d20_preserve_remote_folder_runs_with_the_folder_deletions() {
    let plan = plan_json(json!({
        "local": [{ "path": "😀µµK", "isFolder": true }],
        "remote": [{ "path": "attachments", "id": "d5-0", "isFolder": true }],
        "records": [
            { "path": "ś", "isFolder": true, "remoteId": "d0-0", "syncedAt": 1, "remoteFolderPreserved": true },
            { "path": "i😀.MD", "isFolder": false, "remoteId": "f1-0", "remoteVersion": "v0", "syncedAt": 1,
              "localHash": "75e0d458fc2da40b5b8b8b614d0192e9da7fffc2d6042f33300fdb0e8a83dfb4" },
            { "path": "ΜÉÉ", "isFolder": true, "remoteId": "d2-0", "syncedAt": 1, "remoteFolderPreserved": true },
            { "path": "😀µµK", "isFolder": true, "remoteId": "d4-0", "syncedAt": 1 },
            { "path": "attachments", "isFolder": true, "remoteId": "d5-0", "syncedAt": 1 },
        ],
        "remoteFolderDeletion": "unsupported",
    }));
    let kinds: Vec<_> = plan
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["kind"].as_str().unwrap(), a["path"].as_str().unwrap()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("forget", "ś"),
            ("forget", "ΜÉÉ"),
            ("forget", "i😀.MD"),
            ("deleteLocalFolder", "😀µµK"),
            ("preserveRemoteFolder", "attachments"),
        ]
    );
}

// D21: the first nightly (seed 1) found a folder case rename inferred while a descendant was recorded under the
// other spelling of the folder. That record is inconsistent (spec, "Case-only renames"), so no rename is inferred and
// every path in the case group, the descendant included, is a case error.

#[test]
fn d21_descendant_recorded_under_another_spelling_blocks_a_folder_case_rename() {
    let png = "attachments/cc88f6acf10ba9e488201201e9067fd63fc23aeb64fcf686578e6d900fceeef6.png";
    let plan = plan_json(json!({
        "local": [{ "path": "attachmentS", "isFolder": true }],
        "remote": [{ "path": "attachments", "id": "d4-0", "isFolder": true }],
        "records": [
            { "path": png, "isFolder": true, "remoteId": "f5-0", "remoteVersion": "v0", "syncedAt": 1,
              "localHash": "cc88f6acf10ba9e488201201e9067fd63fc23aeb64fcf686578e6d900fceeef6" },
            { "path": "attachmentS", "isFolder": true, "remoteId": "d6-0", "syncedAt": 1 },
        ],
    }));
    assert_eq!(
        plan,
        json!([
            error("attachmentS", CASE_MESSAGE),
            error("attachments", CASE_MESSAGE),
            error(png, CASE_MESSAGE),
        ])
    );
}

// D22: the first nightly (seed 1) found a case rename before the `recordFolder` of its parent. Renames come right
// after the errors (since 0.3.0); a rename only joins entries on both sides, so its ancestors exist on both sides
// and their step only records them. Characterised: the spec doesn't place renames in the plan order yet.

#[test]
fn d22_case_rename_runs_before_its_parents_record_folder() {
    let hash = "75e0d458fc2da40b5b8b8b614d0192e9da7fffc2d6042f33300fdb0e8a83dfb4";
    let plan = plan_json(json!({
        "local": [{ "path": "B/İς.md", "isFolder": false, "sha256": hash }, { "path": "B", "isFolder": true }],
        "remote": [
            { "path": "B/İσ.MD", "id": "f2-0", "isFolder": false, "version": "v1" },
            { "path": "B", "id": "d0-0", "isFolder": true },
        ],
        "records": [{ "path": "B/İσ.MD", "isFolder": false, "remoteId": "f2-0", "remoteVersion": "v1", "localHash": hash, "syncedAt": 1 }],
    }));
    let kinds: Vec<_> = plan
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["kind"].as_str().unwrap(), a["path"].as_str().unwrap()))
        .collect();
    assert_eq!(kinds, [("renameRemote", "B/İσ.MD"), ("recordFolder", "B")]);
}
