//! Observed execution-level fixture results, shared by the Rust and binding fixture runners.

use serde_json::{json, Value};

use super::exec::{self, sync_session};
use super::world::{Fault, FaultAt};
use super::{quiesce, FaultKind, When, World};

pub fn write_observations(target: &std::path::Path) {
    let spec = std::env::var_os("MARKNOTES_SPEC_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec"));
    let fixtures: Value = serde_json::from_str(
        &std::fs::read_to_string(spec.join("fixtures/sync/executor-safety.json")).unwrap(),
    )
    .unwrap();
    let observations: Vec<_> = fixtures["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let actual = run(&case["input"]);
            assert_eq!(actual, case["expected"], "{}", case["name"]);
            json!({ "input": case["input"], "actual": actual })
        })
        .collect();
    std::fs::write(
        target.join("executor-safety.json"),
        serde_json::to_vec(&observations).unwrap(),
    )
    .unwrap();
}

fn before(kind: &'static str) -> When {
    When::BeforeAction {
        dev: 0,
        kind: Some(kind),
        path: None,
    }
}

fn session(w: &mut World) -> usize {
    sync_session(w, 0, 0).unwrap().errors.len()
}

fn folder_world() -> World {
    let mut w = World::pair();
    w.create(0, "F/a.md");
    quiesce(&mut w);
    w.delete(0, "F");
    w
}

fn conflict_world() -> (World, String) {
    let mut w = World::pair();
    w.time = 0;
    let loser = w.create_plain(0, "p.md");
    w.create_plain(1, "p.md");
    exec::sync(&mut w, 1);
    (w, loser)
}

pub fn run(input: &Value) -> Value {
    match input["scenario"].as_str().expect("scenario") {
        "folder-delete-concurrent-upload" => {
            let mut w = folder_world();
            w.hook(before("deleteRemoteFolder"), |w| {
                w.drives[0]
                    .create_file("F/new.md", b"unseen\n".to_vec(), 9, false)
                    .unwrap();
            });
            let errors = session(&mut w);
            json!({ "errors": errors, "unseenPreserved": w.drives[0].entry("F/new.md").is_some_and(|e| e.bytes == b"unseen\n"),
                "folderRecordRetained": w.devices[0].record("drive0", "F").is_some() })
        }
        "folder-delete-unsupported"
        | "folder-delete-default-unsupported"
        | "folder-delete-runtime-failure" => {
            let mut w = folder_world();
            if input["scenario"] == "folder-delete-runtime-failure" {
                w.fail_at(0, "drive delete folder", FaultKind::Fail);
            } else {
                w.drives[0].supports_empty_delete = false;
                if input["scenario"] == "folder-delete-default-unsupported" {
                    w.drives[0].declared_folder_deletion = Some(Default::default());
                }
            }
            let errors = session(&mut w);
            json!({ "errors": errors, "recursiveDeleteRequests": w.drives[0].recursive_delete_requests,
                "folderRecordRetained": w.devices[0].record("drive0", "F").is_some(),
                "notices": w.devices[0].durable.folder_notices.len() })
        }
        "folder-delete-deferred" => deferred(input),
        "folder-delete-local-recreation-race" => {
            let mut w = folder_world();
            w.drives[0].supports_empty_delete = false;
            w.hook(before("preserveRemoteFolder"), |w| w.mkdir(0, "F"));
            let errors = session(&mut w);
            json!({ "errors": errors, "notices": w.devices[0].durable.folder_notices.len(),
                "localFolderExists": w.devices[0].node("F").is_some(),
                "remoteFolderExists": w.drives[0].entry("F").is_some(),
                "preserved": preserved(&w), "recursiveDeleteRequests": w.drives[0].recursive_delete_requests })
        }
        "folder-delete-child-revision-race" | "folder-delete-unsupported-child-revision-race" => {
            let mut w = folder_world();
            let unsupported = input["scenario"] == "folder-delete-unsupported-child-revision-race";
            w.drives[0].supports_empty_delete = !unsupported;
            w.hook(before("deleteRemote"), |w| {
                let entry = w.drives[0].entry("F/a.md").unwrap().clone();
                w.drives[0]
                    .update_file(
                        "F/a.md",
                        &entry.id,
                        Some(&entry.version),
                        b"changed\n".to_vec(),
                        10,
                    )
                    .unwrap();
            });
            let errors = session(&mut w);
            let mut result = json!({ "errors": errors, "changedChildPreserved": w.drives[0].entry("F/a.md").is_some_and(|e| e.bytes == b"changed\n"),
                "childRecordRetained": w.devices[0].record("drive0", "F/a.md").is_some(),
                "folderRecordRetained": w.devices[0].record("drive0", "F").is_some() });
            if unsupported {
                result["notices"] = json!(w.devices[0].durable.folder_notices.len());
                result["recursiveDeleteRequests"] = json!(w.drives[0].recursive_delete_requests);
            }
            result
        }

        "new-file-create-race" => {
            let mut w = World::pair();
            w.create_plain(0, "p.md");
            w.hook(before("uploadNew"), |w| {
                w.drives[0]
                    .create_file("p.md", b"existing\n".to_vec(), 8, false)
                    .unwrap();
            });
            let errors = session(&mut w);
            json!({ "errors": errors, "existingTextPreserved": w.drives[0].entry("p.md").is_some_and(|e| e.bytes == b"existing\n"),
                "creationRecorded": w.devices[0].record("drive0", "p.md").is_some() })
        }
        "new-folder-create-race" => {
            let mut w = World::pair();
            w.mkdir(0, "F");
            w.hook(before("createRemoteFolder"), |w| {
                let folder = w.drives[0].create_folder("F").unwrap();
                // The drive counter is deterministic, but compare the observed raced identity.
                w.log(format!("raced folder identity: {}", folder.id));
            });
            let errors = session(&mut w);
            let identity = w.drives[0].entry("F").map(|e| e.id.as_str());
            let preserved = identity.is_some_and(|id| {
                w.log
                    .iter()
                    .any(|l| l == &format!("raced folder identity: {id}"))
            });
            json!({ "errors": errors, "existingIdentityPreserved": preserved,
                "creationRecorded": w.devices[0].record("drive0", "F").is_some() })
        }
        "conflict-copy-restart" => {
            let (mut probe, _) = conflict_world();
            let start = probe.trace.len();
            session(&mut probe);
            let (label, occurrence) = match input["crashAfter"].as_str().expect("crashAfter") {
                "intent" => ("save pending conflict", 0),
                "copy" => ("write copy", 0),
                "winner" => ("write local", 0),
                "winner-record" => ("save record", 0),
                "copy-upload" => ("upload copy", 0),
                "copy-record" => ("save record", 1),
                other => panic!("unknown crash point {other}"),
            };
            let position = probe
                .trace
                .iter()
                .enumerate()
                .skip(start)
                .filter(|(_, s)| *s == &format!("dev0:{label}"))
                .nth(occurrence)
                .expect("crash point reached")
                .0;
            let (mut w, loser) = conflict_world();
            w.fault = Some(Fault {
                at: FaultAt::Step(position as u64 + 1),
                kind: FaultKind::Crash,
            });
            assert!(sync_session(&mut w, 0, 0).is_err(), "crash must occur");
            w.restart(0);
            let mut errors = 0;
            for _ in 0..4 {
                errors += session(&mut w);
            }
            let copies: Vec<_> = w.devices[0]
                .durable
                .files
                .keys()
                .filter(|p| p.contains("(conflicted copy)"))
                .collect();
            let copy = copies.first().and_then(|p| w.devices[0].file(p));
            let preserved = copy.is_some_and(|(bytes, _)| {
                marknotes_sync::front_matter::split(std::str::from_utf8(bytes).unwrap()).1 == loser
            });
            json!({ "errors": errors, "copyCount": copies.len(), "copyModifiedMillis": copy.map(|(_, m)| m),
                "losingTextPreserved": preserved, "intentCleared": w.devices[0].durable.copy_intents.is_empty() })
        }
        other => panic!("unknown execution scenario {other}"),
    }
}

fn preserved(w: &World) -> bool {
    w.devices[0]
        .record("drive0", "F")
        .is_some_and(|r| r.remote_folder_preserved)
}

fn deferred(input: &Value) -> Value {
    use marknotes_sync::Action;
    use std::collections::BTreeSet;
    let event = input["event"].as_str().expect("event");
    let mut w = if event == "multi-drive" {
        let mut w = World::new(2, &[&[0, 1]], 1);
        w.create(0, "F/a.md");
        quiesce(&mut w);
        w.delete(0, "F");
        w
    } else if event == "unchanged-subfolders" {
        let mut w = World::pair();
        w.create(0, "F/Sub/a.md");
        quiesce(&mut w);
        w.delete(0, "F");
        w
    } else {
        folder_world()
    };
    let old_id = w.drives[0].entry("F").unwrap().id.clone();
    let baseline = w.devices[0].record("drive0", "F/a.md").cloned();
    for drive in &mut w.drives {
        drive.supports_empty_delete = false;
    }
    match event {
        "before-preserve-save" => w.fail_at(0, "check preserve remote folder", FaultKind::Crash),
        "after-preserve-save" => w.fail_at(0, "preserve remote folder", FaultKind::Crash),
        "preserve-save-failure" => w.fail_at(0, "preserve remote folder", FaultKind::Fail),
        "identity-replacement-restart" => w.fail_at(0, "drive delete", FaultKind::Fail),
        _ => {}
    }
    let first = sync_session(&mut w, 0, 0);
    let mut errors = first.as_ref().map_or(0, |out| out.errors.len());
    if matches!(event, "before-preserve-save" | "after-preserve-save") {
        assert!(first.is_err(), "crash must occur");
    }
    w.restart(0);
    if matches!(
        event,
        "before-preserve-save" | "after-preserve-save" | "preserve-save-failure"
    ) {
        errors += session(&mut w);
    }
    let mut old_descendant_history_retired = None;
    let child_before = w.drives[0].entry("F/a.md").cloned();
    match event {
        "restart-pending"
        | "unchanged-subfolders"
        | "before-preserve-save"
        | "after-preserve-save"
        | "preserve-save-failure" => {}
        "restart-acknowledged" => {
            let generation = w.devices[0].durable.folder_notices[0].generation;
            exec::acknowledge_folder_notice(&mut w, 0, generation).unwrap();
            w.restart(0);
        }
        "ack-save-failure" => {
            w.fail_at(0, "acknowledge folder notice", FaultKind::Fail);
            let generation = w.devices[0].durable.folder_notices[0].generation;
            assert!(exec::acknowledge_folder_notice(&mut w, 0, generation).is_err());
            errors += 1;
            w.restart(0);
        }
        "new-child" => {
            w.drives[0]
                .create_file("F/new.md", b"new\n".to_vec(), 9, false)
                .unwrap();
        }
        "changed-child" => {
            let mut record = baseline.unwrap();
            let entry = w.drives[0]
                .create_file("F/a.md", b"base\n".to_vec(), 9, false)
                .unwrap();
            record.remote_id = entry.id.clone();
            record.remote_version = entry.version.clone();
            record.local_hash = super::oracle::sha_hex(b"base\n");
            w.devices[0]
                .durable
                .records
                .get_mut("drive0")
                .unwrap()
                .insert(record.path.clone(), record);
            w.drives[0]
                .update_file(
                    "F/a.md",
                    &entry.id,
                    Some(&entry.version),
                    b"changed\n".to_vec(),
                    10,
                )
                .unwrap();
        }
        "ignored-child" => {
            w.drives[0]
                .create_file("F/keep.pdf", b"ignored\n".to_vec(), 9, false)
                .unwrap();
        }
        "new-empty-subfolder" => {
            w.drives[0].create_folder("F/New").unwrap();
        }
        "local-recreation" => {
            w.mkdir(0, "F");
            errors += session(&mut w);
            w.delete(0, "F");
        }
        "local-recreation-remote-missing" | "local-recreation-preflight-restart" => {
            w.drives[0].delete_empty_folder("F", &old_id).unwrap_err();
            // A user/provider removes the folder, independently of the unsupported sync adapter.
            w.drives[0].supports_empty_delete = true;
            w.drives[0].delete_empty_folder("F", &old_id).unwrap();
            w.drives[0].supports_empty_delete = false;
            w.mkdir(0, "F");
            if event == "local-recreation-preflight-restart" {
                w.fail_at(0, "flag local deletions", FaultKind::Crash);
                assert!(sync_session(&mut w, 0, 0).is_err());
                w.restart(0);
            }
        }
        "remote-disappearance" => {
            w.drives[0].supports_empty_delete = true;
            w.drives[0].delete_empty_folder("F", &old_id).unwrap();
        }
        "identity-replacement" => {
            w.drives[0].supports_empty_delete = true;
            w.drives[0].delete_empty_folder("F", &old_id).unwrap();
            w.drives[0].supports_empty_delete = false;
            w.drives[0].create_folder("F").unwrap();
            w.drives[0]
                .create_file("F/a.md", b"replacement\n".to_vec(), 10, false)
                .unwrap();
        }
        "identity-replacement-restart" => {
            w.drives[0].replace_folder_identity("F");
            w.fail_at(0, "drive read", FaultKind::Fail);
            errors += session(&mut w);
            old_descendant_history_retired = Some(
                w.devices[0].record("drive0", "F/a.md").is_none()
                    && w.devices[0].base("drive0", "F/a.md").is_none(),
            );
            w.restart(0);
        }
        "multi-drive" => {
            errors += sync_session(&mut w, 0, 1).unwrap().errors.len();
            w.mkdir(0, "F");
            w.drives[1].supports_empty_delete = true;
            let child = w.drives[1].entry("F/a.md");
            assert!(child.is_none());
            let id = w.drives[1].entry("F").unwrap().id.clone();
            w.drives[1].delete_empty_folder("F", &id).unwrap();
            w.drives[1].supports_empty_delete = false;
            errors += session(&mut w);
            errors += sync_session(&mut w, 0, 1).unwrap().errors.len();
        }
        "supported" => {
            w.drives[0].supports_empty_delete = true;
        }
        "failed-listing" => {
            w.drives[0].next_listing = Some(super::drive::ListingMode::Fails);
        }
        other => panic!("unknown deferred event {other}"),
    }
    errors += session(&mut w);
    let repeated = if matches!(
        event,
        "restart-pending"
            | "restart-acknowledged"
            | "unchanged-subfolders"
            | "before-preserve-save"
            | "after-preserve-save"
            | "preserve-save-failure"
            | "ack-save-failure"
    ) {
        let out = sync_session(&mut w, 0, 0).unwrap();
        errors += out.errors.len();
        out.actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    Action::PreserveRemoteFolder { .. }
                        | Action::DeleteRemoteFolder { .. }
                        | Action::CreateRemoteFolder { .. }
                        | Action::CreateLocalFolder { .. }
                )
            })
            .count()
    } else {
        0
    };
    let d = &w.devices[0];
    let notices = &d.durable.folder_notices;
    let all = json!({
        "errors": errors, "notices": notices.len(),
        "pendingNotices": notices.iter().filter(|n| n.active && !n.acknowledged).count(),
        "acknowledgedNotices": notices.iter().filter(|n| n.acknowledged).count(),
        "activeNotices": notices.iter().filter(|n| n.active).count(),
        "localFolderExists": d.node("F").is_some(), "remoteFolderExists": w.drives[0].entry("F").is_some(),
        "folderRecordRetained": d.record("drive0", "F").is_some(), "preserved": preserved(&w),
        "repeatActions": repeated, "recursiveDeleteRequests": w.drives[0].recursive_delete_requests,
        "newChildDownloaded": d.file("F/new.md").is_some_and(|(b, _)| b == b"new\n"),
        "changedChildDownloaded": d.file("F/a.md").is_some_and(|(b, _)| b == b"changed\n"),
        "ignoredChildPreserved": w.drives[0].entry("F/keep.pdf").is_some(),
        "ignoredChildDownloaded": d.file("F/keep.pdf").is_some(),
        "localSubfolderExists": d.node("F/New").is_some(),
        "sameRemoteIdentity": w.drives[0].entry("F").is_some_and(|e| e.id == old_id),
        "replacementChildDownloaded": d.file("F/a.md").is_some_and(|(b, _)| b == b"replacement\n"),
        "replacementIdentityRecorded": d.record("drive0", "F").is_some_and(|r| w.drives[0].entry("F").is_some_and(|e| r.remote_id == e.id && e.id != old_id)),
        "distinctNoticeDrives": notices.iter().map(|n| &n.drive).collect::<BTreeSet<_>>().len(),
        "bothRemoteFoldersExist": w.drives.iter().all(|r| r.entry("F").is_some()),
        "preservedRecords": d.durable.records.values().flat_map(|r| r.values()).filter(|r| r.remote_folder_preserved).count()
        ,"oldDescendantHistoryRetired": old_descendant_history_retired,
        "childDownloaded": child_before.as_ref().is_some_and(|e| d.file("F/a.md").is_some_and(|(b,_)| b == e.bytes)),
        "remoteChildPreserved": child_before.as_ref().is_some_and(|e| w.drives[0].entry("F/a.md").is_some_and(|r| r == e))
    });
    let fields: &[&str] = match event {
        "restart-pending" => &[
            "errors",
            "notices",
            "pendingNotices",
            "localFolderExists",
            "remoteFolderExists",
            "folderRecordRetained",
            "repeatActions",
        ],
        "restart-acknowledged" => &[
            "errors",
            "notices",
            "pendingNotices",
            "acknowledgedNotices",
            "localFolderExists",
            "repeatActions",
        ],
        "new-child" => &[
            "errors",
            "localFolderExists",
            "newChildDownloaded",
            "preserved",
            "activeNotices",
        ],
        "changed-child" => &[
            "errors",
            "localFolderExists",
            "changedChildDownloaded",
            "preserved",
            "activeNotices",
        ],
        "ignored-child" => &[
            "errors",
            "localFolderExists",
            "ignoredChildPreserved",
            "ignoredChildDownloaded",
            "preserved",
            "activeNotices",
        ],
        "new-empty-subfolder" => &[
            "errors",
            "localFolderExists",
            "localSubfolderExists",
            "preserved",
            "activeNotices",
        ],
        "local-recreation" => &[
            "errors",
            "notices",
            "pendingNotices",
            "sameRemoteIdentity",
            "localFolderExists",
            "preserved",
        ],
        "local-recreation-remote-missing" | "local-recreation-preflight-restart" => &[
            "errors",
            "localFolderExists",
            "remoteFolderExists",
            "preserved",
            "activeNotices",
        ],
        "remote-disappearance" => &[
            "errors",
            "localFolderExists",
            "remoteFolderExists",
            "folderRecordRetained",
            "activeNotices",
        ],
        "identity-replacement" => &[
            "errors",
            "localFolderExists",
            "replacementChildDownloaded",
            "replacementIdentityRecorded",
            "preserved",
            "activeNotices",
        ],
        "identity-replacement-restart" => &[
            "errors",
            "oldDescendantHistoryRetired",
            "childDownloaded",
            "remoteChildPreserved",
            "activeNotices",
            "recursiveDeleteRequests",
        ],
        "multi-drive" => &[
            "errors",
            "notices",
            "distinctNoticeDrives",
            "localFolderExists",
            "bothRemoteFoldersExist",
            "preservedRecords",
            "activeNotices",
        ],
        "supported" => &[
            "errors",
            "remoteFolderExists",
            "folderRecordRetained",
            "activeNotices",
            "recursiveDeleteRequests",
        ],
        "failed-listing" => &[
            "errors",
            "folderRecordRetained",
            "pendingNotices",
            "localFolderExists",
            "remoteFolderExists",
        ],
        "before-preserve-save" | "after-preserve-save" | "preserve-save-failure" => &[
            "errors",
            "notices",
            "pendingNotices",
            "folderRecordRetained",
            "localFolderExists",
            "repeatActions",
        ],
        "ack-save-failure" => &[
            "errors",
            "notices",
            "pendingNotices",
            "acknowledgedNotices",
            "repeatActions",
        ],
        "unchanged-subfolders" => &[
            "errors",
            "notices",
            "pendingNotices",
            "localFolderExists",
            "preservedRecords",
            "repeatActions",
        ],
        _ => unreachable!(),
    };
    Value::Object(
        fields
            .iter()
            .map(|key| ((*key).to_string(), all[*key].clone()))
            .collect(),
    )
}
