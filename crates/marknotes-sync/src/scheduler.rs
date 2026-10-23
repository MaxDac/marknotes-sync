//! Order in which the drives are synced so that every drive ends up with the changes brought by the others.

use std::collections::{HashMap, VecDeque};

pub const DEFAULT_MAX_PASSES: u32 = 3;

/// Drives are synced in turn. Whenever a drive brings changes into the local tree, every other drive not already
/// waiting is queued again so it receives them, at most `max_passes` times per drive.
#[derive(Debug, Clone)]
pub struct Scheduler {
    drive_ids: Vec<String>,
    max_passes: u32,
    pending: VecDeque<String>,
    passes: HashMap<String, u32>,
}

impl Scheduler {
    pub fn new(drive_ids: Vec<String>, max_passes: u32) -> Self {
        Scheduler {
            pending: drive_ids.iter().cloned().collect(),
            drive_ids,
            max_passes,
            passes: HashMap::new(),
        }
    }

    /// The next drive to sync, or `None` when done.
    // Named `next` in every binding; this isn't an iterator because `report` feeds back between calls.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<String> {
        while let Some(id) = self.pending.pop_front() {
            let count = self.passes.entry(id.clone()).or_insert(0);
            if *count >= self.max_passes {
                continue;
            }
            *count += 1;
            return Some(id);
        }
        None
    }

    /// Reports the outcome of syncing `drive_id`: `local_changes` is the number of changes it made to the local
    /// tree (downloads, local deletions, merges, conflicted copies); a failed sync brings none.
    pub fn report(&mut self, drive_id: &str, local_changes: u32, failed: bool) {
        if failed || local_changes == 0 {
            return;
        }
        for id in &self.drive_ids {
            if id != drive_id && !self.pending.contains(id) {
                self.pending.push_back(id.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ids: &[&str], max: u32, changes: &[(u32, bool)]) -> Vec<String> {
        let mut s = Scheduler::new(ids.iter().map(|s| s.to_string()).collect(), max);
        let mut order = Vec::new();
        let mut i = 0;
        while let Some(id) = s.next() {
            let (c, f) = changes.get(i).copied().unwrap_or((0, false));
            s.report(&id, c, f);
            order.push(id);
            i += 1;
        }
        order
    }

    #[test]
    fn requeues_other_drives() {
        assert_eq!(
            run(&["a", "b"], 3, &[(0, false), (1, false)]),
            ["a", "b", "a"]
        );
        assert_eq!(run(&["a", "b", "c"], 3, &[(1, false)]), ["a", "b", "c"]);
        assert_eq!(run(&["a", "b"], 3, &[(0, false), (1, true)]), ["a", "b"]);
        assert_eq!(
            run(
                &["a", "b"],
                2,
                &[(1, false), (1, false), (1, false), (1, false)]
            ),
            ["a", "b", "a", "b"]
        );
    }
}
