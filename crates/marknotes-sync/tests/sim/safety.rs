//! Observed execution-level fixture results, shared by the Rust and binding fixture runners.

use serde_json::{json, Value};

use super::exec::{self, sync_session};
use super::world::{Fault, FaultAt};
use super::{quiesce, FaultKind, When, World};

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
        "folder-delete-unsupported" => {
            let mut w = folder_world();
            w.drives[0].supports_empty_delete = false;
            let errors = session(&mut w);
            json!({ "errors": errors, "recursiveDeleteRequests": w.drives[0].recursive_delete_requests,
                "folderRecordRetained": w.devices[0].record("drive0", "F").is_some() })
        }
        "folder-delete-child-revision-race" => {
            let mut w = folder_world();
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
            json!({ "errors": errors, "changedChildPreserved": w.drives[0].entry("F/a.md").is_some_and(|e| e.bytes == b"changed\n"),
                "childRecordRetained": w.devices[0].record("drive0", "F/a.md").is_some(),
                "folderRecordRetained": w.devices[0].record("drive0", "F").is_some() })
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
