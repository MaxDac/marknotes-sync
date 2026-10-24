//! Runs the marknotes-spec fixtures that cover the sync core. The spec is read from `MARKNOTES_SPEC_DIR` or from the
//! `spec/` submodule at the repository root. Every case must pass: the core is the reference implementation, so
//! `knownDivergence` entries (which describe the apps) are ignored.

use std::collections::HashMap;
use std::path::PathBuf;

use marknotes_sync::*;
use serde_json::{json, Value};

fn spec_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MARKNOTES_SPEC_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec")
}

fn cases(file: &str) -> Vec<Value> {
    let path = spec_dir().join("fixtures").join(file);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "can't read {} ({e}); run `git submodule update --init` or set MARKNOTES_SPEC_DIR",
            path.display()
        )
    });
    let body: Value = serde_json::from_str(&text).unwrap();
    body["cases"].as_array().unwrap().clone()
}

fn run(file: &str, f: impl Fn(&Value) -> Value) {
    let mut failures = Vec::new();
    for case in cases(file) {
        let actual = f(&case["input"]);
        if actual != case["expected"] {
            failures.push(format!(
                "{file} / {}\n  expected {}\n  actual   {}",
                case["name"], case["expected"], actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failing case(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string {key}"))
}

fn from<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}

/// Fixture folders the core implements in full: every file in them must have a runner below.
const OWNED_FOLDERS: [&str; 3] = ["sync", "manifest", "merge"];

/// The fixture files that have a runner in this file.
const MAPPED: [&str; 8] = [
    "sync/plan.json",
    "sync/reconcile.json",
    "sync/multi-drive.json",
    "manifest/parse.json",
    "manifest/serialize.json",
    "manifest/build.json",
    "merge/merge3.json",
    "merge/conflict-copy-name.json",
];

#[test]
fn every_owned_fixture_file_is_mapped() {
    let mut unmapped = Vec::new();
    for folder in OWNED_FOLDERS {
        let dir = spec_dir().join("fixtures").join(folder);
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| {
            panic!(
                "can't list {} ({e}); run `git submodule update --init` or set MARKNOTES_SPEC_DIR",
                dir.display()
            )
        });
        for entry in entries {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") {
                let file = format!("{folder}/{name}");
                if !MAPPED.contains(&file.as_str()) {
                    unmapped.push(file);
                }
            }
        }
    }
    unmapped.sort();
    assert!(
        unmapped.is_empty(),
        "fixture file(s) without a runner in tests/spec_fixtures.rs: {}",
        unmapped.join(", ")
    );
}

#[test]
fn sync_plan() {
    run("sync/plan.json", |i| {
        serde_json::to_value(plan(&from(i))).unwrap()
    });
}

#[test]
fn sync_reconcile() {
    run("sync/reconcile.json", |i| {
        serde_json::to_value(reconcile(&from(i))).unwrap()
    });
}

#[test]
fn sync_multi_drive() {
    run("sync/multi-drive.json", |i| {
        let ids: Vec<String> = from(&i["driveIds"]);
        let mut scheduler = Scheduler::new(ids, i["maxPasses"].as_u64().unwrap() as u32);
        let reports = i["reports"].as_array().unwrap();
        let mut order = Vec::new();
        while let Some(id) = scheduler.next() {
            let r = &reports[order.len()];
            scheduler.report(
                &id,
                r["localChanges"].as_u64().unwrap() as u32,
                r["failed"].as_bool().unwrap(),
            );
            order.push(id);
        }
        json!({ "order": order })
    });
}

#[test]
fn manifest_parse() {
    run(
        "manifest/parse.json",
        |i| json!({ "manifest": parse_manifest(str_of(i, "json")) }),
    );
}

#[test]
fn manifest_serialize() {
    run(
        "manifest/serialize.json",
        |i| json!({ "json": serialize_manifest(&from(i)) }),
    );
}

#[test]
fn manifest_build() {
    run("manifest/build.json", |i| {
        let records: Vec<SyncRecord> = from(&i["records"]);
        let json = build_manifest(
            &records,
            str_of(i, "deviceId"),
            i["nowMillis"].as_i64().unwrap(),
            i["previous"].as_str(),
        );
        json!({ "json": json })
    });
}

#[test]
fn merge_merge3() {
    run("merge/merge3.json", |i| {
        let result = merge3(i["base"].as_str(), str_of(i, "local"), str_of(i, "remote"));
        let text = match i.get("choices") {
            Some(c) if !c.is_null() => {
                let choices: HashMap<String, ConflictChoice> = from(c);
                let choices = choices
                    .into_iter()
                    .map(|(k, v)| (k.parse::<i32>().unwrap(), v))
                    .collect();
                merge_text(&result.chunks, &choices)
            }
            _ => result.text,
        };
        json!({ "text": text, "conflicts": result.conflicts })
    });
}

#[test]
fn merge_conflict_copy_name() {
    run("merge/conflict-copy-name.json", |i| {
        let existing: Vec<String> = i.get("existingPaths").map(from).unwrap_or_default();
        json!({ "path": paths::conflict_copy_name(str_of(i, "path"), &existing) })
    });
}

#[test]
fn front_matter_split() {
    run("front-matter/split.json", |i| {
        let (front_matter, body) = front_matter::split(str_of(i, "text"));
        json!({ "frontMatter": front_matter, "body": body })
    });
}

#[test]
fn front_matter_read_id() {
    run(
        "front-matter/read-id.json",
        |i| json!({ "id": front_matter::read_id(str_of(i, "text")) }),
    );
}

#[test]
fn front_matter_with_id() {
    run(
        "front-matter/with-id.json",
        |i| json!({ "text": front_matter::with_id(str_of(i, "text"), str_of(i, "id")) }),
    );
}

#[test]
fn note_ids_validation() {
    run(
        "note-ids/validation.json",
        |i| json!({ "valid": front_matter::is_valid_id(str_of(i, "value")) }),
    );
}

#[test]
fn attachments_is_attachment_path() {
    run(
        "attachments/is-attachment-path.json",
        |i| json!({ "isAttachment": paths::is_attachment_path(str_of(i, "path")) }),
    );
}
