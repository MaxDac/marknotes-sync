//! Parity corpus: the Rust core generates many seeded, randomized inputs and records its own outputs; the Kotlin
//! (`bindings/kotlin/src/sharedTest/.../ParityTest.kt`, JVM and Android) and npm (`bindings/npm/test/parity.test.mjs`)
//! bindings replay every case and must produce exactly the same outputs. Parity means the binding layers
//! (serialization, numbers, strings with non-BMP characters, byte order marks and control characters, null and
//! `None`) add nothing to and lose nothing from what the core computes.
//!
//! Regenerate with `cargo test -p marknotes-sync --test parity_corpus --locked -- --ignored`. It writes
//! `target/parity/<op>.json` at the workspace root (one file per operation, in the shape of the spec fixtures:
//! `cases[].input` and `cases[].expected`, camelCase fields, actions tagged with `kind`) and `target/parity/index.json`
//! (the seed and the case count of every operation). The output is byte-identical for a given seed:
//! `MARKNOTES_PARITY_SEED` (decimal, or hex with `0x`) overrides the seed and `MARKNOTES_PARITY_CASES` the number of
//! cases per operation (200 by default).
//!
//! Only inputs every binding accepts are generated: times are JS safe integers and scheduler counts fit a Kotlin
//! `Int`. Inputs the bindings reject by design are tested in `bindings/npm/test/boundary.test.mjs` and the Kotlin
//! `BindingsTest`.

mod support;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::PathBuf;

use marknotes_sync::hashing::sha256_hex;
use marknotes_sync::manifest::{
    build_manifest, parse_manifest, serialize_manifest, Manifest, ManifestEntry,
};
use marknotes_sync::merge::{merge3, merge_text, ConflictChoice};
use marknotes_sync::model::{ReconcileInput, RemoteEntry, SyncInput, SyncRecord};
use marknotes_sync::scheduler::Scheduler;
use marknotes_sync::{front_matter, paths, plan, reconcile};
use proptest::collection::{hash_map, vec};
use proptest::prelude::*;
use proptest::sample::select;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use serde_json::{json, Value};
use support::gen;

/// `Number.MAX_SAFE_INTEGER`: the largest time every binding carries exactly.
const MAX_SAFE: i64 = 9_007_199_254_740_991;
const DEFAULT_SEED: u64 = 0x7061_7269_7479_0001;
const DEFAULT_CASES: usize = 200;

fn seed() -> u64 {
    std::env::var("MARKNOTES_PARITY_SEED")
        .ok()
        .map(|s| {
            let s = s.trim().to_string();
            match s.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16),
                None => s.parse(),
            }
            .unwrap_or_else(|e| panic!("invalid MARKNOTES_PARITY_SEED {s:?}: {e}"))
        })
        .unwrap_or(DEFAULT_SEED)
}

fn cases() -> usize {
    std::env::var("MARKNOTES_PARITY_CASES")
        .ok()
        .map(|s| {
            s.trim()
                .parse()
                .unwrap_or_else(|e| panic!("invalid MARKNOTES_PARITY_CASES {s:?}: {e}"))
        })
        .unwrap_or(DEFAULT_CASES)
}

/// A runner whose RNG depends only on the seed and the operation, so adding an operation leaves the others as they
/// are.
fn runner(seed: u64, op: &str) -> TestRunner {
    let hex = sha256_hex(format!("marknotes parity {seed} {op}").as_bytes());
    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
        .collect();
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes))
}

// ---------------------------------------------------------------------------------------------------------------
// Values every binding accepts
// ---------------------------------------------------------------------------------------------------------------

/// `v` itself when it is a safe integer, otherwise its remainder modulo 2^53 (keeping its sign).
fn safe(v: i64) -> i64 {
    if v.unsigned_abs() <= MAX_SAFE as u64 {
        v
    } else {
        v % (MAX_SAFE + 1)
    }
}

/// Times: small, negative, realistic, around the 32-bit limits and at the safe-integer bounds.
fn millis() -> BoxedStrategy<i64> {
    prop_oneof![
        4 => 0i64..5,
        2 => -1000i64..1000,
        2 => 1_600_000_000_000i64..1_800_000_000_000,
        1 => select(vec![
            MAX_SAFE,
            -MAX_SAFE,
            MAX_SAFE - 1,
            i64::from(i32::MAX) + 1,
            i64::from(i32::MIN) - 1,
            i64::from(u32::MAX) + 1,
        ]),
        1 => any::<i64>().prop_map(safe),
    ]
    .boxed()
}

/// Strings that trip up JS objects, JSON, UTF-16 and C strings.
const ADVERSARIAL: &[&str] = &[
    "",
    "__proto__",
    "constructor",
    "prototype",
    "toString",
    "hasOwnProperty",
    "valueOf",
    "0",
    "-1",
    "1e3",
    "null",
    "undefined",
    "NaN",
    "\u{feff}",
    "\u{feff}a.md",
    "\u{0}",
    "a\u{0}b.md",
    "\u{1F600}.md",
    "\u{10FFFF}",
    "\u{FFFF}",
    "\u{FFFE}",
    "\u{2028}\u{2029}",
    "\u{D7FF}\u{E000}",
    "e\u{301}.md",
    "\u{e9}.md",
    "\\",
    "\"",
    "'",
    "\r\n",
    "\r",
    "attachments",
    "attachments/",
    ".marknotes/manifest.json",
    "a/../b.md",
    "Ω/ẞ/ſ.MD",
];

fn adversarial() -> BoxedStrategy<String> {
    select(ADVERSARIAL).prop_map(str::to_string).boxed()
}

/// Any string: path-like, short with controls and separators, adversarial, or arbitrary.
fn string() -> BoxedStrategy<String> {
    prop_oneof![
        3 => gen::any_path(),
        2 => gen::any_short_string(12),
        1 => adversarial(),
        1 => any::<String>(),
    ]
    .boxed()
}

fn safe_input(mut input: SyncInput) -> SyncInput {
    for e in &mut input.local {
        e.modified_millis = safe(e.modified_millis);
    }
    for e in &mut input.remote {
        e.modified_millis = safe(e.modified_millis);
    }
    for r in &mut input.records {
        r.synced_at = safe(r.synced_at);
    }
    input
}

fn safe_manifest(mut m: Manifest) -> Manifest {
    m.updated_at = safe(m.updated_at);
    for e in m.files.values_mut() {
        e.synced_at = safe(e.synced_at);
    }
    m
}

/// Whether every number in `v` is a safe integer.
fn all_safe(v: &Value) -> bool {
    match v {
        Value::Number(n) => n
            .as_i64()
            .is_some_and(|i| i.unsigned_abs() <= MAX_SAFE as u64),
        Value::Array(a) => a.iter().all(all_safe),
        Value::Object(o) => o.values().all(all_safe),
        _ => true,
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("serializable")
}

// ---------------------------------------------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------------------------------------------

/// One generated case: `None` when the input is outside what every binding accepts.
type Case = Option<(Value, Value)>;

struct Op {
    name: &'static str,
    description: &'static str,
    cases: BoxedStrategy<Case>,
}

fn plan_op() -> BoxedStrategy<Case> {
    prop_oneof![
        2 => gen::sync_input(12),
        1 => gen::synced_input(8),
        2 => gen::inconsistent_sync_input(12),
        2 => gen::arbitrary_sync_input(),
    ]
    .prop_map(|input| {
        let input = safe_input(input);
        let expected = to_json(&plan(&input));
        Some((to_json(&input), expected))
    })
    .boxed()
}

fn reconcile_op() -> BoxedStrategy<Case> {
    let texts = prop_oneof![
        4 => gen::text_triple(),
        1 => (gen::note_text(), gen::note_text(), gen::note_text()),
        1 => (string(), string(), string()),
    ];
    let path = prop_oneof![3 => gen::note_path(3), 1 => gen::any_path(), 1 => adversarial()];
    let remote = (string(), string(), any::<bool>(), string(), millis());
    let new_id = prop_oneof![4 => gen::valid_id(), 1 => gen::invalid_id(), 1 => string()];
    (
        path.prop_flat_map(|p| (Just(p.clone()), gen::existing_paths_near(p))),
        texts,
        prop::bool::weighted(0.75),
        millis(),
        remote,
        any::<bool>(),
        new_id,
    )
        .prop_map(
            |(
                (path, existing_paths),
                (base, local, remote_text),
                has_base,
                local_modified_millis,
                (other_path, id, is_folder, version, modified_millis),
                same_path,
                new_id,
            )| {
                let input = ReconcileInput {
                    path: path.clone(),
                    local,
                    local_modified_millis,
                    remote: remote_text,
                    remote_entry: RemoteEntry {
                        path: if same_path { path } else { other_path },
                        id,
                        is_folder,
                        version,
                        modified_millis,
                    },
                    base: has_base.then_some(base),
                    existing_paths,
                    new_id,
                    conflict_copy_intent: None,
                    existing_copy: None,
                };
                let expected = to_json(&reconcile(&input));
                Some((to_json(&input), expected))
            },
        )
        .boxed()
}

fn choice() -> BoxedStrategy<ConflictChoice> {
    select(vec![
        ConflictChoice::Local,
        ConflictChoice::Remote,
        ConflictChoice::Both,
        ConflictChoice::Neither,
    ])
    .boxed()
}

fn merge3_op() -> BoxedStrategy<Case> {
    let texts = prop_oneof![
        30 => gen::text_triple(),
        4 => (gen::text(), gen::text(), gen::text()),
        4 => (string(), string(), string()),
        1 => gen::large_text_triple(),
    ];
    let choices = prop::option::weighted(
        0.5,
        vec(
            (
                prop_oneof![8 => 0i32..6, 1 => Just(-1), 1 => Just(i32::MAX)],
                choice(),
            ),
            0..5,
        ),
    );
    (texts, prop::bool::weighted(0.85), choices)
        .prop_map(|((base, local, remote), has_base, choices)| {
            let base = has_base.then_some(base);
            let result = merge3(base.as_deref(), &local, &remote);
            let mut input = json!({ "base": base, "local": local, "remote": remote });
            let text = match choices {
                Some(list) => {
                    let map: HashMap<i32, ConflictChoice> = list.iter().copied().collect();
                    let keys: BTreeMap<String, ConflictChoice> =
                        map.iter().map(|(k, v)| (k.to_string(), *v)).collect();
                    input["choices"] = to_json(&keys);
                    merge_text(&result.chunks, &map)
                }
                None => result.text.clone(),
            };
            let expected = json!({
                "text": text,
                "conflicts": result.conflicts,
                "hasConflicts": result.has_conflicts,
                "chunks": result.chunks,
            });
            Some((input, expected))
        })
        .boxed()
}

/// Manifest paths: the generator's, plus keys that are special in JS objects.
fn manifest() -> BoxedStrategy<Manifest> {
    let extra = hash_map(
        adversarial(),
        (gen::json_string(), gen::json_string(), millis()),
        0..3,
    );
    (gen::manifest(), extra, millis(), prop::bool::weighted(0.5))
        .prop_map(|(mut m, extra, updated_at, realistic)| {
            for (path, (sha256, version, synced_at)) in extra {
                m.files.insert(
                    path,
                    ManifestEntry {
                        sha256,
                        version,
                        synced_at,
                    },
                );
            }
            if realistic {
                m.updated_at = updated_at;
            }
            safe_manifest(m)
        })
        .boxed()
}

fn parse_manifest_op() -> BoxedStrategy<Case> {
    prop_oneof![
        4 => gen::json_text(),
        2 => manifest().prop_map(|m| serialize_manifest(&m)),
        1 => string(),
    ]
    .prop_map(|text| {
        let expected = json!({ "manifest": parse_manifest(&text) });
        // A manifest with times beyond 2^53 can't cross the JS boundary (see index.d.ts).
        all_safe(&expected).then(|| (json!({ "json": text }), expected))
    })
    .boxed()
}

fn serialize_manifest_op() -> BoxedStrategy<Case> {
    manifest()
        .prop_map(|m| {
            let expected = json!({ "json": serialize_manifest(&m) });
            Some((to_json(&m), expected))
        })
        .boxed()
}

fn build_manifest_op() -> BoxedStrategy<Case> {
    let records = prop_oneof![
        3 => gen::records(8),
        1 => vec(gen::arbitrary_record(), 0..8),
    ];
    (
        records,
        string(),
        millis(),
        0u8..4,
        gen::json_text(),
        millis(),
    )
        .prop_map(
            |(records, device_id, now_millis, previous_kind, garbage, other_time)| {
                let records: Vec<SyncRecord> = records
                    .into_iter()
                    .map(|r| SyncRecord {
                        synced_at: safe(r.synced_at),
                        ..r
                    })
                    .collect();
                let previous = match previous_kind {
                    0 => None,
                    1 => Some(garbage),
                    // The same files written by another device: nothing to write.
                    2 => build_manifest(&records, "other", other_time, None),
                    // The same files but for the first one.
                    _ => build_manifest(
                        records.get(1..).unwrap_or_default(),
                        "other",
                        other_time,
                        None,
                    ),
                };
                let expected = json!({
                    "json": build_manifest(&records, &device_id, now_millis, previous.as_deref())
                });
                let input = json!({
                    "records": records,
                    "deviceId": device_id,
                    "nowMillis": now_millis,
                    "previous": previous,
                });
                Some((input, expected))
            },
        )
        .boxed()
}

fn conflict_copy_name_op() -> BoxedStrategy<Case> {
    prop_oneof![3 => gen::note_path(4), 2 => gen::any_path(), 1 => adversarial()]
        .prop_flat_map(|p| (Just(p.clone()), gen::existing_paths_near(p)))
        .prop_map(|(path, existing)| {
            let expected = json!({ "path": paths::conflict_copy_name(&path, &existing) });
            Some((json!({ "path": path, "existingPaths": existing }), expected))
        })
        .boxed()
}

/// Paths for the path predicates: anything, plus attachment-like names in other cases and with other extensions.
fn predicate_path() -> BoxedStrategy<String> {
    prop_oneof![
        4 => gen::any_path(),
        2 => gen::attachment_path(),
        1 => gen::attachment_path().prop_map(|p| p.to_uppercase()),
        1 => (gen::attachment_path(), select(vec!["/", "x/", "attachments/", ""]), select(vec![".md", ".PNG", "", ".png.md"]))
            .prop_map(|(p, pre, suf)| format!("{pre}{p}{suf}")),
        1 => adversarial(),
    ]
    .boxed()
}

fn is_syncable_path_op() -> BoxedStrategy<Case> {
    predicate_path()
        .prop_map(|path| {
            let expected = json!({ "syncable": paths::is_syncable_path(&path) });
            Some((json!({ "path": path }), expected))
        })
        .boxed()
}

fn is_synced_entry_op() -> BoxedStrategy<Case> {
    (predicate_path(), any::<bool>())
        .prop_map(|(path, is_folder)| {
            let expected = json!({ "synced": paths::is_synced_entry(&path, is_folder) });
            Some((json!({ "path": path, "isFolder": is_folder }), expected))
        })
        .boxed()
}

fn is_attachment_path_op() -> BoxedStrategy<Case> {
    predicate_path()
        .prop_map(|path| {
            let expected = json!({ "isAttachment": paths::is_attachment_path(&path) });
            Some((json!({ "path": path }), expected))
        })
        .boxed()
}

fn note() -> BoxedStrategy<String> {
    prop_oneof![
        4 => gen::note_text(),
        2 => (gen::valid_id(), select(vec!["\n", "\r\n"]), gen::body_text())
            .prop_map(|(id, nl, body)| format!("---{nl}id: {id}{nl}---{nl}{body}")),
        1 => string(),
    ]
    .boxed()
}

fn read_id_op() -> BoxedStrategy<Case> {
    note()
        .prop_map(|text| {
            let expected = json!({ "id": front_matter::read_id(&text) });
            Some((json!({ "text": text }), expected))
        })
        .boxed()
}

fn with_id_op() -> BoxedStrategy<Case> {
    (
        note(),
        prop_oneof![4 => gen::valid_id(), 1 => gen::invalid_id(), 1 => string()],
    )
        .prop_map(|(text, id)| {
            let expected = json!({ "text": front_matter::with_id(&text, &id) });
            Some((json!({ "text": text, "id": id }), expected))
        })
        .boxed()
}

fn sha256_hex_op() -> BoxedStrategy<Case> {
    prop_oneof![
        2 => vec(any::<u8>(), 0..80),
        1 => vec(any::<u8>(), 0..300),
        2 => string().prop_map(String::into_bytes),
    ]
    .prop_map(|bytes| {
        let expected = json!({ "sha256": sha256_hex(&bytes) });
        Some((json!({ "bytes": bytes }), expected))
    })
    .boxed()
}

fn sync_scheduler_op() -> BoxedStrategy<Case> {
    let ids = prop_oneof![
        3 => gen::drive_ids(),
        1 => vec(prop_oneof![2 => select(vec!["a", "b", "c"]).prop_map(str::to_string), 1 => adversarial()], 0..5),
    ];
    (
        ids,
        prop_oneof![1 => Just(0u32), 5 => 1u32..6],
        gen::drive_reports(),
    )
        .prop_map(|(ids, max_passes, reports)| {
            // Kotlin takes the counts as Int.
            let reports: Vec<(u32, bool)> = reports
                .into_iter()
                .map(|(n, failed)| (n.min(i32::MAX as u32), failed))
                .collect();
            let mut scheduler = Scheduler::new(ids.clone(), max_passes);
            let mut order = Vec::new();
            let mut used = Vec::new();
            while let Some(id) = scheduler.next() {
                let (n, failed) = reports[used.len() % reports.len()];
                scheduler.report(&id, n, failed);
                used.push(json!({ "localChanges": n, "failed": failed }));
                order.push(id);
            }
            let input = json!({ "driveIds": ids, "maxPasses": max_passes, "reports": used });
            Some((input, json!({ "order": order })))
        })
        .boxed()
}

fn ops() -> Vec<Op> {
    vec![
        Op {
            name: "plan",
            description: "plan(input) -> { actions }, as spec/fixtures/sync/plan.json.",
            cases: plan_op(),
        },
        Op {
            name: "reconcile",
            description: "reconcile(input) -> outcome tagged with kind, as spec/fixtures/sync/reconcile.json.",
            cases: reconcile_op(),
        },
        Op {
            name: "merge3",
            description: "merge3(base, local, remote), then mergeText(chunks, choices) when input.choices is \
                          present, as spec/fixtures/merge/merge3.json; expected also holds hasConflicts and chunks.",
            cases: merge3_op(),
        },
        Op {
            name: "parseManifest",
            description: "parseManifest(json) -> { manifest } (null when unreadable), as \
                          spec/fixtures/manifest/parse.json.",
            cases: parse_manifest_op(),
        },
        Op {
            name: "serializeManifest",
            description: "serializeManifest(input) -> { json }, as spec/fixtures/manifest/serialize.json.",
            cases: serialize_manifest_op(),
        },
        Op {
            name: "buildManifest",
            description: "buildManifest(records, deviceId, nowMillis, previous) -> { json } (null when unchanged), as \
                          spec/fixtures/manifest/build.json.",
            cases: build_manifest_op(),
        },
        Op {
            name: "conflictCopyName",
            description: "conflictCopyName(path, existingPaths) -> { path }, as \
                          spec/fixtures/merge/conflict-copy-name.json.",
            cases: conflict_copy_name_op(),
        },
        Op {
            name: "isSyncablePath",
            description: "isSyncablePath(path) -> { syncable }.",
            cases: is_syncable_path_op(),
        },
        Op {
            name: "isSyncedEntry",
            description: "isSyncedEntry(path, isFolder) -> { synced }.",
            cases: is_synced_entry_op(),
        },
        Op {
            name: "isAttachmentPath",
            description: "isAttachmentPath(path) -> { isAttachment }, as \
                          spec/fixtures/attachments/is-attachment-path.json.",
            cases: is_attachment_path_op(),
        },
        Op {
            name: "readId",
            description: "readId(text) -> { id } (null when none), as spec/fixtures/front-matter/read-id.json.",
            cases: read_id_op(),
        },
        Op {
            name: "withId",
            description: "withId(text, id) -> { text }, as spec/fixtures/front-matter/with-id.json.",
            cases: with_id_op(),
        },
        Op {
            name: "sha256Hex",
            description: "sha256Hex(bytes) -> { sha256 }; input.bytes lists the bytes (0..255).",
            cases: sha256_hex_op(),
        },
        Op {
            name: "syncScheduler",
            description: "new SyncScheduler(driveIds, maxPasses); call next() until it returns null, reporting \
                          reports[n] after the n-th call -> { order }, as spec/fixtures/sync/multi-drive.json.",
            cases: sync_scheduler_op(),
        },
    ]
}

fn file_text(op: &Op, seed: u64, cases: &[String]) -> String {
    let mut out = String::new();
    writeln!(out, "{{").unwrap();
    writeln!(
        out,
        "  \"description\": {},",
        Value::from(format!(
            "Parity corpus generated by crates/marknotes-sync/tests/parity_corpus.rs. {}",
            op.description
        ))
    )
    .unwrap();
    writeln!(out, "  \"op\": {},", Value::from(op.name)).unwrap();
    writeln!(out, "  \"seed\": {},", Value::from(format!("{seed:#x}"))).unwrap();
    writeln!(out, "  \"cases\": [").unwrap();
    for (i, c) in cases.iter().enumerate() {
        let sep = if i + 1 < cases.len() { "," } else { "" };
        writeln!(out, "    {c}{sep}").unwrap();
    }
    writeln!(out, "  ]").unwrap();
    writeln!(out, "}}").unwrap();
    out
}

#[test]
#[ignore = "writes the parity corpus to target/parity; run with --ignored"]
fn write_parity_corpus() {
    let seed = seed();
    let count = cases();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/parity");
    std::fs::create_dir_all(&dir).unwrap();
    let mut index = serde_json::Map::new();
    let mut total = 0;
    for op in ops() {
        let mut runner = runner(seed, op.name);
        let mut cases = Vec::with_capacity(count);
        let mut skipped = 0;
        while cases.len() < count {
            let tree = op
                .cases
                .new_tree(&mut runner)
                .unwrap_or_else(|e| panic!("{}: can't generate a case: {e}", op.name));
            match tree.current() {
                Some((input, expected)) => {
                    let case = json!({
                        "name": format!("{} #{}", op.name, cases.len()),
                        "input": input,
                        "expected": expected,
                    });
                    cases.push(serde_json::to_string(&case).unwrap());
                }
                None => {
                    skipped += 1;
                    assert!(
                        skipped <= 10 * count.max(1),
                        "{}: too many skipped inputs",
                        op.name
                    );
                }
            }
        }
        let text = file_text(&op, seed, &cases);
        let path = dir.join(format!("{}.json", op.name));
        std::fs::write(&path, &text).unwrap();
        println!(
            "{}: {} cases ({skipped} skipped), {} bytes",
            op.name,
            cases.len(),
            text.len()
        );
        total += text.len();
        index.insert(op.name.to_string(), Value::from(cases.len()));
    }
    let index = json!({ "seed": format!("{seed:#x}"), "ops": index });
    std::fs::write(
        dir.join("index.json"),
        format!("{}\n", serde_json::to_string_pretty(&index).unwrap()),
    )
    .unwrap();
    println!(
        "parity corpus: seed {seed:#x}, {} bytes in {}",
        total,
        dir.display()
    );
}
