//! Unit tests for `manifest.rs` (`parse_manifest`, `serialize_manifest`, `build_manifest`) and `scheduler.rs`. Each
//! test is named after the rule it checks and cites the section of `spec/note-format.md` it comes from. Tests marked
//! "Characterisation" pin behaviour the spec leaves open, so that any change to it is deliberate.

use std::collections::HashMap;

use marknotes_sync::manifest::MANIFEST_FORMAT;
use marknotes_sync::scheduler::DEFAULT_MAX_PASSES;
use marknotes_sync::*;

const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn manifest_with(fields: &str) -> String {
    format!("{{{fields}}}")
}

/// A valid manifest root with `files` set to the given JSON.
fn with_files(files: &str) -> String {
    format!(r#"{{"format":1,"deviceId":"d","updatedAt":1,"files":{files}}}"#)
}

fn assert_accepts(texts: &[&str]) {
    for text in texts {
        assert!(parse_manifest(text).is_some(), "should accept {text}");
    }
}

fn assert_rejects(texts: &[&str]) {
    for text in texts {
        assert!(parse_manifest(text).is_none(), "should reject {text}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// parse_manifest: what readers accept and reject
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync manifest": the format written today is 1.
#[test]
fn the_format_written_is_1() {
    assert_eq!(MANIFEST_FORMAT, 1);
}

/// note-format.md, "Sync manifest": readers reject a manifest whose `format` is not a number up to 1.
#[test]
fn format_must_be_a_number_up_to_1() {
    let with_format =
        |f: &str| manifest_with(&format!(r#""format":{f},"deviceId":"d","updatedAt":1"#));
    for ok in ["1", "1.0", "0.5", "0", "-1", "-1e308", "1e0"] {
        assert!(parse_manifest(&with_format(ok)).is_some(), "format {ok}");
    }
    for bad in ["1.5", "2", "1e308", "\"1\"", "null", "true", "[1]", "{}"] {
        assert!(parse_manifest(&with_format(bad)).is_none(), "format {bad}");
    }
    assert_rejects(&[r#"{"deviceId":"d","updatedAt":1}"#]);
}

/// note-format.md, "Sync manifest": readers reject a manifest that lacks `deviceId` (a string).
#[test]
fn device_id_must_be_a_string() {
    let with_device =
        |d: &str| manifest_with(&format!(r#""format":1,"deviceId":{d},"updatedAt":1"#));
    for ok in [r#""d""#, r#""""#, r#""é \"x\"""#] {
        assert!(parse_manifest(&with_device(ok)).is_some(), "deviceId {ok}");
    }
    for bad in ["1", "null", "true", "[\"d\"]", "{}"] {
        assert!(
            parse_manifest(&with_device(bad)).is_none(),
            "deviceId {bad}"
        );
    }
    assert_rejects(&[r#"{"format":1,"updatedAt":1}"#]);
}

/// note-format.md, "Sync manifest": readers reject a manifest that lacks `updatedAt`.
#[test]
fn updated_at_is_required() {
    assert_rejects(&[r#"{"format":1,"deviceId":"d"}"#]);
    assert_accepts(&[
        r#"{"format":1,"deviceId":"d","updatedAt":0}"#,
        r#"{"format":1,"deviceId":"d","updatedAt":-5}"#,
        r#"{"format":1,"deviceId":"d","updatedAt":9223372036854775807}"#,
    ]);
}

/// Characterisation (the spec only says "lacks `updatedAt`", without a type): an `updatedAt` that is not a number
/// is rejected like a missing one.
#[test]
fn updated_at_that_is_not_a_number_is_rejected() {
    for bad in [r#""1""#, "null", "true", "[1]", "{}"] {
        let text = manifest_with(&format!(r#""format":1,"deviceId":"d","updatedAt":{bad}"#));
        assert!(parse_manifest(&text).is_none(), "updatedAt {bad}");
    }
}

/// note-format.md, "Sync manifest": a missing `files` means no files; `files` that is not an object is rejected.
#[test]
fn files_must_be_an_object_and_missing_means_none() {
    let none = parse_manifest(r#"{"format":1,"deviceId":"d","updatedAt":1}"#).unwrap();
    assert!(none.files.is_empty());
    assert!(parse_manifest(&with_files("{}")).unwrap().files.is_empty());
    for bad in ["null", "[]", r#""x""#, "1", "true"] {
        assert!(parse_manifest(&with_files(bad)).is_none(), "files {bad}");
    }
}

/// note-format.md, "Sync manifest": readers reject an entry lacking a string `sha256` or `version` or a numeric
/// `syncedAt`.
#[test]
fn entries_need_string_hash_and_version_and_numeric_synced_at() {
    let entry = |e: &str| with_files(&format!(r#"{{"a.md":{e}}}"#));
    assert!(parse_manifest(&entry(r#"{"sha256":"h","version":"v","syncedAt":1}"#)).is_some());
    assert!(parse_manifest(&entry(r#"{"sha256":"","version":"","syncedAt":0}"#)).is_some());
    for bad in [
        "null",
        "1",
        r#""h""#,
        "[]",
        r#"{"version":"v","syncedAt":1}"#,
        r#"{"sha256":"h","syncedAt":1}"#,
        r#"{"sha256":"h","version":"v"}"#,
        r#"{"sha256":1,"version":"v","syncedAt":1}"#,
        r#"{"sha256":null,"version":"v","syncedAt":1}"#,
        r#"{"sha256":"h","version":1,"syncedAt":1}"#,
        r#"{"sha256":"h","version":null,"syncedAt":1}"#,
        r#"{"sha256":"h","version":"v","syncedAt":"1"}"#,
        r#"{"sha256":"h","version":"v","syncedAt":null}"#,
    ] {
        assert!(parse_manifest(&entry(bad)).is_none(), "entry {bad}");
    }
}

/// note-format.md, "Sync manifest": one bad entry rejects the whole manifest, whatever its position.
#[test]
fn one_bad_entry_rejects_the_whole_manifest() {
    let good = r#"{"sha256":"h","version":"v","syncedAt":1}"#;
    let bad = r#"{"sha256":"h","version":"v"}"#;
    assert_rejects(&[
        &with_files(&format!(r#"{{"a.md":{bad},"b.md":{good}}}"#)),
        &with_files(&format!(r#"{{"a.md":{good},"b.md":{bad}}}"#)),
    ]);
}

/// note-format.md, "Sync manifest": unknown keys are ignored, at the root and in entries; `app` is not checked.
#[test]
fn unknown_keys_are_ignored() {
    let m = parse_manifest(
        r#"{"format":1,"app":"Other","x":[1,{"y":null}],"deviceId":"d","updatedAt":7,
            "files":{"a.md":{"sha256":"h","version":"v","syncedAt":2,"size":10}}}"#,
    )
    .unwrap();
    assert_eq!(m.device_id, "d");
    assert_eq!(m.updated_at, 7);
    assert_eq!(
        m.files["a.md"],
        ManifestEntry {
            sha256: "h".into(),
            version: "v".into(),
            synced_at: 2
        }
    );
    assert_accepts(&[r#"{"updatedAt":1,"deviceId":"d","format":1}"#]);
}

/// note-format.md, "Sync manifest": a manifest that is not a JSON object is rejected (treated as absent).
#[test]
fn text_that_is_not_a_json_object_is_rejected() {
    assert_rejects(&[
        "",
        " ",
        "{",
        "null",
        "1",
        r#""manifest""#,
        "[]",
        r#"[{"format":1,"deviceId":"d","updatedAt":1}]"#,
        r#"{"format":1,"deviceId":"d","updatedAt":1,}"#,
        r#"{"format":1,"deviceId":"d","updatedAt":1} x"#,
        r#"{format:1,deviceId:"d",updatedAt:1}"#,
    ]);
}

/// note-format.md, "Sync manifest": the compact form is written, but any JSON layout is read.
#[test]
fn whitespace_between_tokens_is_accepted() {
    assert_accepts(&["\r\n{ \"format\" : 1 ,\n\t\"deviceId\" : \"d\" , \"updatedAt\" : 1 }\n"]);
}

/// Characterisation (the spec does not mention a byte order mark for the manifest): a leading BOM makes the JSON
/// invalid, so the manifest is treated as absent.
#[test]
fn a_leading_byte_order_mark_is_rejected() {
    assert_rejects(&["\u{feff}{\"format\":1,\"deviceId\":\"d\",\"updatedAt\":1}"]);
}

/// Characterisation (JSON leaves duplicate keys undefined; the spec is silent): the last value of a duplicate key
/// wins, at the root and in `files`.
#[test]
fn duplicate_keys_keep_the_last_value() {
    let m = parse_manifest(
        r#"{"format":2,"format":1,"deviceId":"a","deviceId":"b","updatedAt":1,
            "files":{"a.md":{"sha256":"h1","version":"v","syncedAt":1},"a.md":{"sha256":"h2","version":"v","syncedAt":1}}}"#,
    )
    .unwrap();
    assert_eq!(m.device_id, "b");
    assert_eq!(m.files.len(), 1);
    assert_eq!(m.files["a.md"].sha256, "h2");
    assert_rejects(&[r#"{"format":1,"format":2,"deviceId":"d","updatedAt":1}"#]);
}

/// Characterisation (the spec does not bound numbers): a number too large for a double is invalid JSON, so the
/// manifest is rejected; values just past `i64` saturate (see also D12).
#[test]
fn numbers_beyond_double_range_reject_the_manifest() {
    assert_rejects(&[
        r#"{"format":1,"deviceId":"d","updatedAt":1e400}"#,
        &with_files(r#"{"a.md":{"sha256":"h","version":"v","syncedAt":-1e400}}"#),
    ]);
    let m =
        parse_manifest(r#"{"format":1,"deviceId":"d","updatedAt":9223372036854775808}"#).unwrap();
    assert_eq!(m.updated_at, i64::MAX);
    let m =
        parse_manifest(r#"{"format":1,"deviceId":"d","updatedAt":-9223372036854775809}"#).unwrap();
    assert_eq!(m.updated_at, i64::MIN);
}

/// Characterisation (the spec does not say readers check paths): paths in `files` are kept as written, even ones
/// that are not syncable; `plan` only looks them up.
#[test]
fn file_paths_are_not_validated_when_reading() {
    let m = parse_manifest(&with_files(
        r#"{"../x.md":{"sha256":"h","version":"v","syncedAt":1},"":{"sha256":"h","version":"v","syncedAt":1}}"#,
    ))
    .unwrap();
    assert_eq!(m.files.len(), 2);
}

// ---------------------------------------------------------------------------------------------------------------
// serialize_manifest
// ---------------------------------------------------------------------------------------------------------------

fn entry(sha256: &str, version: &str, synced_at: i64) -> ManifestEntry {
    ManifestEntry {
        sha256: sha256.into(),
        version: version.into(),
        synced_at,
    }
}

fn manifest(files: &[(&str, ManifestEntry)]) -> Manifest {
    Manifest {
        device_id: "dev".into(),
        updated_at: 1_700_000_000_000,
        files: files
            .iter()
            .map(|(p, e)| (p.to_string(), e.clone()))
            .collect(),
    }
}

/// note-format.md, "Sync manifest": compact JSON, keys in the order `format, app, deviceId, updatedAt, files`, entry
/// keys `sha256, version, syncedAt`.
#[test]
fn serialized_manifest_is_compact_with_keys_in_spec_order() {
    assert_eq!(
        serialize_manifest(&manifest(&[("Work/Roadmap.md", entry("h", "v1", 5))])),
        r#"{"format":1,"app":"MarkNotes","deviceId":"dev","updatedAt":1700000000000,"files":{"Work/Roadmap.md":{"sha256":"h","version":"v1","syncedAt":5}}}"#
    );
    assert_eq!(
        serialize_manifest(&manifest(&[])),
        r#"{"format":1,"app":"MarkNotes","deviceId":"dev","updatedAt":1700000000000,"files":{}}"#
    );
}

/// note-format.md, "Sync manifest" and "Sync" ("Paths are ordered by UTF-16 code units"): `files` is sorted by
/// UTF-16 code units, which puts a supplementary-plane character (surrogates 0xD800..) before a BMP character above
/// U+E000, unlike code-point order, and is case-sensitive.
#[test]
fn files_are_sorted_by_utf16_code_units() {
    let paths = [
        "\u{FF5E}.md",
        "\u{1F600}.md",
        "\u{E000}.md",
        "é.md",
        "a/b.md",
        "a.md",
        "Z.md",
        "a b.md",
    ];
    let m = manifest(
        &paths
            .iter()
            .map(|p| (*p, entry("h", "v", 1)))
            .collect::<Vec<_>>(),
    );
    let text = serialize_manifest(&m);
    let position = |p: &str| text.find(&format!("\"{p}\":")).unwrap();
    let expected = [
        "Z.md",
        "a b.md",
        "a.md",
        "a/b.md",
        "é.md",
        "\u{1F600}.md",
        "\u{E000}.md",
        "\u{FF5E}.md",
    ];
    for pair in expected.windows(2) {
        assert!(position(pair[0]) < position(pair[1]), "{pair:?} in {text}");
    }
    // Code-point order would put U+1F600 last.
    assert!('\u{1F600}' > '\u{FF5E}');
}

/// note-format.md, "Sync manifest": strings are written as JSON strings, escaped where JSON requires it.
#[test]
fn strings_are_escaped_as_json() {
    let mut m = manifest(&[("a \"q\" \\ é.md", entry("h\n", "v\t", -1))]);
    m.device_id = "d\u{1}".into();
    let text = serialize_manifest(&m);
    assert_eq!(
        text,
        "{\"format\":1,\"app\":\"MarkNotes\",\"deviceId\":\"d\\u0001\",\"updatedAt\":1700000000000,\
         \"files\":{\"a \\\"q\\\" \\\\ é.md\":{\"sha256\":\"h\\n\",\"version\":\"v\\t\",\"syncedAt\":-1}}}"
    );
    assert_eq!(parse_manifest(&text), Some(m));
}

/// note-format.md, "Sync manifest": what is written reads back the same, extreme times included.
#[test]
fn serialized_manifest_reads_back_unchanged() {
    for m in [
        manifest(&[]),
        manifest(&[
            ("b.md", entry(H1, "2", i64::MAX)),
            ("Work/a.md", entry(H2, "1", i64::MIN)),
            ("\u{1F600}/x.md", entry("", "", 0)),
        ]),
    ] {
        let text = serialize_manifest(&m);
        assert_eq!(parse_manifest(&text), Some(m.clone()));
        assert_eq!(serialize_manifest(&parse_manifest(&text).unwrap()), text);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// build_manifest
// ---------------------------------------------------------------------------------------------------------------

fn record(path: &str, is_folder: bool, version: &str, hash: &str, synced_at: i64) -> SyncRecord {
    SyncRecord {
        path: path.into(),
        is_folder,
        remote_id: format!("id-{path}"),
        remote_version: version.into(),
        local_hash: hash.into(),
        synced_at,
    }
}

fn base_records() -> Vec<SyncRecord> {
    vec![
        record("a.md", false, "1", H1, 10),
        record("Work", true, "", "", 10),
        record("Work/b.md", false, "2", H2, 10),
    ]
}

/// note-format.md, "Sync manifest": it lists every synced file (not folders) with the drive version and the hash.
#[test]
fn build_lists_files_with_version_and_hash_and_no_folders() {
    let text = build_manifest(&base_records(), "dev", 99, None).unwrap();
    assert_eq!(
        text,
        format!(
            r#"{{"format":1,"app":"MarkNotes","deviceId":"dev","updatedAt":99,"files":{{"Work/b.md":{{"sha256":"{H2}","version":"2","syncedAt":10}},"a.md":{{"sha256":"{H1}","version":"1","syncedAt":10}}}}}}"#
        )
    );
}

/// note-format.md, "Sync manifest": it is rewritten only when a file's path, hash or version changed, so nothing
/// is written when the previous manifest lists the same files; `syncedAt`, `deviceId`, `updatedAt`, key order and
/// layout don't count.
#[test]
fn build_skips_the_write_when_paths_hashes_and_versions_are_unchanged() {
    let previous = build_manifest(&base_records(), "other", 1, None).unwrap();
    assert_eq!(
        build_manifest(&base_records(), "dev", 99, Some(&previous)),
        None
    );
    let mut later = base_records();
    for r in &mut later {
        r.synced_at = 12345;
    }
    assert_eq!(build_manifest(&later, "dev", 99, Some(&previous)), None);
    let reordered = format!(
        r#"{{ "files": {{ "a.md": {{"syncedAt": 0, "version": "1", "sha256": "{H1}"}},
             "Work/b.md": {{"version": "2", "sha256": "{H2}", "syncedAt": 0}} }},
           "updatedAt": 5, "deviceId": "x", "format": 0.5, "extra": true }}"#
    );
    assert_eq!(
        build_manifest(&base_records(), "dev", 99, Some(&reordered)),
        None
    );
    assert_eq!(
        build_manifest(
            &[],
            "dev",
            1,
            Some(&build_manifest(&[], "d", 1, None).unwrap())
        ),
        None
    );
    let only_folders = [record("Work", true, "", "", 1)];
    assert_eq!(
        build_manifest(
            &only_folders,
            "dev",
            1,
            Some(r#"{"format":1,"deviceId":"d","updatedAt":1}"#)
        ),
        None
    );
}

/// note-format.md, "Sync manifest": a changed path, hash or version, or a file added or removed, rewrites it.
#[test]
fn build_writes_when_a_path_hash_or_version_changed() {
    let previous = build_manifest(&base_records(), "dev", 1, None).unwrap();
    let changes: [fn(&mut Vec<SyncRecord>); 7] = [
        |r| r[0].local_hash = H2.into(),
        |r| r[0].remote_version = "1b".into(),
        |r| r[0].path = "A.md".into(),
        |r| r[2].path = "Work/B.md".into(),
        |r| {
            r.remove(0);
        },
        |r| r.push(record("c.md", false, "3", H1, 1)),
        |r| r[0].is_folder = true,
    ];
    for (i, change) in changes.iter().enumerate() {
        let mut records = base_records();
        change(&mut records);
        let text = build_manifest(&records, "dev", 2, Some(&previous));
        assert!(text.is_some(), "change {i}");
        assert_eq!(parse_manifest(&text.unwrap()).unwrap().updated_at, 2);
    }
}

/// note-format.md, "Sync manifest": "A rejected manifest is treated as absent", so with no readable previous
/// manifest it is always written, even with no files.
#[test]
fn build_writes_when_the_previous_manifest_is_absent_or_rejected() {
    for previous in [
        None,
        Some(""),
        Some("not json"),
        Some(r#"{"format":2,"deviceId":"d","updatedAt":1}"#),
        Some(r#"{"format":1,"deviceId":"d","updatedAt":1,"files":null}"#),
    ] {
        assert!(
            build_manifest(&[], "dev", 1, previous).is_some(),
            "{previous:?}"
        );
        assert!(
            build_manifest(&base_records(), "dev", 1, previous).is_some(),
            "{previous:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------------------------------------------

/// Runs a scheduler to the end; `changes` gives each drive's local changes and failure, by drive id.
fn run(ids: &[&str], max: u32, mut outcome: impl FnMut(&str, usize) -> (u32, bool)) -> Vec<String> {
    let mut s = Scheduler::new(ids.iter().map(|s| s.to_string()).collect(), max);
    let mut order = Vec::new();
    while let Some(id) = s.next() {
        let (changes, failed) = outcome(&id, order.len());
        s.report(&id, changes, failed);
        order.push(id);
        assert!(order.len() <= 10_000, "scheduler does not terminate");
    }
    order
}

fn ids(order: &[String]) -> Vec<&str> {
    order.iter().map(String::as_str).collect()
}

/// note-format.md, "Several drives": at most 3 passes per drive by default.
#[test]
fn default_cap_is_three_passes() {
    assert_eq!(DEFAULT_MAX_PASSES, 3);
}

/// note-format.md, "Several drives": the app syncs the drives in order; without local changes each drive runs once.
#[test]
fn drives_run_once_in_order_without_changes() {
    assert_eq!(
        ids(&run(&["a", "b", "c"], 3, |_, _| (0, false))),
        ["a", "b", "c"]
    );
}

/// note-format.md, "Several drives": no drives means nothing to run.
#[test]
fn no_drives_run_nothing() {
    assert!(run(&[], 3, |_, _| (1, false)).is_empty());
}

/// Characterisation (the spec says "at most 3 passes" without a zero case): a cap of 0 runs nothing.
#[test]
fn zero_passes_run_nothing() {
    assert!(run(&["a", "b"], 0, |_, _| (1, false)).is_empty());
}

/// note-format.md, "Several drives": a single drive is never queued again, even when it brings changes.
#[test]
fn a_single_drive_runs_once() {
    assert_eq!(ids(&run(&["a"], 3, |_, _| (5, false))), ["a"]);
}

/// note-format.md, "Several drives": a drive bringing local changes queues every other drive not already waiting.
#[test]
fn local_changes_queue_the_other_drives_not_waiting() {
    // a brings changes while b and c are still waiting: nothing is added.
    assert_eq!(
        ids(&run(&["a", "b", "c"], 3, |_, n| (u32::from(n == 0), false))),
        ["a", "b", "c"]
    );
    // c brings changes after a and b ran: both run again, in drive order.
    assert_eq!(
        ids(&run(&["a", "b", "c"], 3, |_, n| (u32::from(n == 2), false))),
        ["a", "b", "c", "a", "b"]
    );
    // b brings changes while c is waiting: only a is added, after c.
    assert_eq!(
        ids(&run(&["a", "b", "c"], 3, |_, n| (u32::from(n == 1), false))),
        ["a", "b", "c", "a"]
    );
}

/// note-format.md, "Several drives": "A drive whose sync failed brings no changes", whatever it reports.
#[test]
fn a_failed_drive_brings_no_changes() {
    assert_eq!(
        ids(&run(&["a", "b"], 3, |id, _| (9, id == "b"))),
        ["a", "b"]
    );
    assert_eq!(ids(&run(&["a", "b"], 3, |_, _| (9, true))), ["a", "b"]);
}

/// note-format.md, "Several drives": drives that keep bringing changes stop at the cap of passes per drive.
#[test]
fn re_passes_stop_at_the_cap() {
    assert_eq!(
        ids(&run(&["a", "b"], 3, |_, _| (1, false))),
        ["a", "b", "a", "b", "a", "b"]
    );
    assert_eq!(
        ids(&run(&["a", "b", "c"], 3, |_, _| (1, false))),
        ["a", "b", "c", "a", "b", "c", "a", "b", "c"]
    );
    assert_eq!(ids(&run(&["a", "b"], 1, |_, _| (1, false))), ["a", "b"]);
}

/// note-format.md, "Several drives": whatever the drives report, the scheduler ends, every drive runs at least once
/// (when the cap is at least 1) and none runs more than the cap.
#[test]
fn every_drive_runs_between_once_and_the_cap() {
    let drive_lists: [&[&str]; 4] = [
        &["a"],
        &["a", "b"],
        &["a", "b", "c"],
        &["a", "b", "c", "d", "e"],
    ];
    for drives in drive_lists {
        for max in 1..=4 {
            for seed in 0u32..64 {
                // A small deterministic generator: bit k of the seed decides whether run k brings changes or fails.
                let order = run(drives, max, |_, n| {
                    let bit = (seed >> (n % 6)) & 1;
                    (bit * (n as u32 % 3), n % 5 == 4)
                });
                let mut passes: HashMap<&str, u32> = HashMap::new();
                for id in &order {
                    *passes.entry(id.as_str()).or_default() += 1;
                }
                for d in drives {
                    let p = passes.get(d).copied().unwrap_or(0);
                    assert!(
                        (1..=max).contains(&p),
                        "{drives:?} max {max} seed {seed}: {order:?}"
                    );
                }
                assert_eq!(&ids(&order)[..drives.len()], drives);
            }
        }
    }
}

/// Characterisation (the spec assumes distinct drives): a drive id listed twice runs twice, both runs counting
/// toward the same cap, and its own changes never queue it again.
#[test]
fn duplicate_drive_ids_share_one_cap() {
    assert_eq!(ids(&run(&["a", "a"], 3, |_, _| (0, false))), ["a", "a"]);
    assert_eq!(ids(&run(&["a", "a"], 1, |_, _| (0, false))), ["a"]);
    assert_eq!(ids(&run(&["a", "a"], 3, |_, _| (1, false))), ["a", "a"]);
    assert_eq!(
        ids(&run(&["a", "b", "a"], 2, |_, _| (1, false))),
        ["a", "b", "a", "b"]
    );
}

/// Characterisation (the spec only covers reports for the drive that just ran): a report for an unknown drive, or
/// one made before any `next`, queues every listed drive not already waiting.
#[test]
fn reports_for_unknown_drives_queue_every_drive_not_waiting() {
    let mut s = Scheduler::new(vec!["a".into(), "b".into()], 3);
    s.report("a", 1, false);
    assert_eq!(s.next().as_deref(), Some("a"));
    assert_eq!(s.next().as_deref(), Some("b"));
    s.report("zz", 1, false);
    assert_eq!(s.next().as_deref(), Some("a"));
    assert_eq!(s.next().as_deref(), Some("b"));
    s.report("zz", 0, false);
    s.report("zz", 1, true);
    assert_eq!(s.next(), None);
    assert_eq!(s.next(), None);
}
