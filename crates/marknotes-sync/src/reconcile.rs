//! What to do with a note that changed on both sides: keep it if identical, merge it if the edits don't overlap,
//! otherwise keep the newest version and save the other one as a conflicted copy.

use crate::front_matter;
use crate::hashing::sha256_hex;
use crate::merge::merge3;
use crate::model::{ConflictCopyIntent, ReconcileInput, ReconcileOutcome, Side};
use crate::paths::conflict_copy_name;

pub fn reconcile(input: &ReconcileInput) -> ReconcileOutcome {
    if input.local == input.remote {
        return ReconcileOutcome::Same;
    }
    if let Some(base) = &input.base {
        let merged = merge3(Some(base), &input.local, &input.remote);
        if !merged.has_conflicts {
            return ReconcileOutcome::Merged { text: merged.text };
        }
    }
    let winner = if input.local_modified_millis > input.remote_entry.modified_millis {
        Side::Local
    } else {
        Side::Remote
    };
    let loser = match winner {
        Side::Local => &input.remote,
        Side::Remote => &input.local,
    };
    let copy_modified_millis = match winner {
        Side::Local => input.remote_entry.modified_millis,
        Side::Remote => input.local_modified_millis,
    };
    let mut existing_paths = input.existing_paths.clone();
    if let Some(copy) = &input.existing_copy {
        existing_paths.push(copy.path.clone());
    }
    let mut intent = ConflictCopyIntent {
        path: input.path.clone(),
        local_hash: sha256_hex(input.local.as_bytes()),
        remote_hash: sha256_hex(input.remote.as_bytes()),
        remote_id: input.remote_entry.id.clone(),
        remote_version: input.remote_entry.version.clone(),
        local_modified_millis: input.local_modified_millis,
        remote_modified_millis: input.remote_entry.modified_millis,
        base_hash: input.base.as_ref().map(|b| sha256_hex(b.as_bytes())),
        new_id: input.new_id.clone(),
        copy_path: conflict_copy_name(&input.path, &existing_paths),
        copy_modified_millis,
    };
    let mut reuse_copy = false;
    if let Some(saved) = &input.conflict_copy_intent {
        let mut expected = intent.clone();
        expected.new_id = saved.new_id.clone();
        expected.copy_path = saved.copy_path.clone();
        let expected_text = front_matter::with_id(loser, &saved.new_id);
        let valid_path = valid_copy_path(&input.path, &saved.copy_path);
        let verified_copy = input.existing_copy.as_ref().is_some_and(|copy| {
            copy.path == saved.copy_path
                && copy.text == expected_text
                && copy.modified_millis == saved.copy_modified_millis
        });
        let absent = input.existing_copy.is_none()
            && !input
                .existing_paths
                .iter()
                .any(|p| crate::paths::fold_case(p) == crate::paths::fold_case(&saved.copy_path));
        if *saved == expected
            && front_matter::is_valid_id(&saved.new_id)
            && valid_path
            && (verified_copy || absent)
        {
            intent = saved.clone();
            reuse_copy = verified_copy;
        }
    }
    let copy_text = front_matter::with_id(loser, &intent.new_id);
    ReconcileOutcome::NewestWins {
        winner,
        copy_path: intent.copy_path.clone(),
        copy_text,
        copy_modified_millis,
        conflict_copy_intent: intent,
        reuse_copy,
    }
}

fn valid_copy_path(path: &str, copy: &str) -> bool {
    let first = conflict_copy_name(path, &[]);
    if copy == first {
        return true;
    }
    let file = crate::paths::name(path);
    let ext = file.rfind('.').map_or("", |i| &file[i..]);
    let prefix = first.strip_suffix(ext).unwrap_or(&first);
    copy.strip_prefix(&format!("{prefix} "))
        .and_then(|suffix| suffix.strip_suffix(ext))
        .and_then(|n| n.parse::<u32>().ok().map(|value| (n, value)))
        .is_some_and(|(n, value)| value >= 2 && n == value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RemoteEntry;

    fn input(local: &str, remote: &str, base: Option<&str>, local_millis: i64) -> ReconcileInput {
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
                modified_millis: 100,
            },
            base: base.map(str::to_string),
            existing_paths: vec!["Work/Plan.md".into()],
            new_id: "m4n5p6q7".into(),
            conflict_copy_intent: None,
            existing_copy: None,
        }
    }

    #[test]
    fn same_text() {
        assert_eq!(
            reconcile(&input("a\n", "a\n", None, 0)),
            ReconcileOutcome::Same
        );
    }

    #[test]
    fn merges_when_possible() {
        let out = reconcile(&input("A\nb\nc\n", "a\nb\nC\n", Some("a\nb\nc\n"), 0));
        assert_eq!(
            out,
            ReconcileOutcome::Merged {
                text: "A\nb\nC\n".into()
            }
        );
    }

    #[test]
    fn newest_wins_and_copy_gets_new_id() {
        let local = "---\nid: k7f3q2xa\n---\nlocal\n";
        let remote = "---\nid: k7f3q2xa\n---\nremote\n";
        let out = reconcile(&input(
            local,
            remote,
            Some("---\nid: k7f3q2xa\n---\nbase\n"),
            101,
        ));
        let ReconcileOutcome::NewestWins {
            winner,
            copy_path,
            copy_text,
            copy_modified_millis,
            ..
        } = out
        else {
            panic!("expected conflict");
        };
        assert_eq!(winner, Side::Local);
        assert_eq!(copy_path, "Work/Plan (conflicted copy).md");
        assert_eq!(copy_text, "---\nid: m4n5p6q7\n---\nremote\n");
        assert_eq!(copy_modified_millis, 100);
        // Ties go to the drive.
        let out = reconcile(&input("x\n", "y\n", None, 100));
        assert!(
            matches!(out, ReconcileOutcome::NewestWins { winner: Side::Remote, ref copy_text, .. } if copy_text == "---\nid: m4n5p6q7\n---\nx\n")
        );
    }
}
