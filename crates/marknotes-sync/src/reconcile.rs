//! What to do with a note that changed on both sides: keep it if identical, merge it if the edits don't overlap,
//! otherwise keep the newest version and save the other one as a conflicted copy.

use crate::front_matter;
use crate::merge::merge3;
use crate::model::{ReconcileInput, ReconcileOutcome, Side};
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
    let copy_text = if front_matter::read_id(loser).is_some() {
        front_matter::with_id(loser, &input.new_id)
    } else {
        loser.clone()
    };
    ReconcileOutcome::NewestWins {
        winner,
        copy_path: conflict_copy_name(&input.path, &input.existing_paths),
        copy_text,
    }
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
        assert_eq!(
            out,
            ReconcileOutcome::NewestWins {
                winner: Side::Local,
                copy_path: "Work/Plan (conflicted copy).md".into(),
                copy_text: "---\nid: m4n5p6q7\n---\nremote\n".into(),
            }
        );
        // Ties go to the drive.
        let out = reconcile(&input("x\n", "y\n", None, 100));
        assert!(
            matches!(out, ReconcileOutcome::NewestWins { winner: Side::Remote, ref copy_text, .. } if copy_text == "x\n")
        );
    }
}
