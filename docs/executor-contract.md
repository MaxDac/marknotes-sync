# Executor contract

marknotes-sync only plans. Each app has an **executor** that lists the trees, calls `plan`, runs the actions with its
own stores, calls `reconcile` and `buildManifest`, and drives the `Scheduler`. The core's guarantees hold only if the
executor follows the rules below:

- no live version of a note is ever lost;
- devices converge;
- a successful sync is followed by an empty plan.

Each rule cites [`spec/note-format.md`](../spec/note-format.md), "Sync", and names the test in
`crates/marknotes-sync/tests/simulation.rs` that checks it. The tests run a reference executor (`tests/sim/exec.rs`)
under faults and crashes. They use an independent no-loss oracle, defined in `tests/sim/mod.rs`.

## Rules

| # | The executor MUST | Spec | Checked by |
| --- | --- | --- | --- |
| 1 | **Complete listings.** Pass only a complete listing of the drive to `plan`. If a listing fails, or an entry can't be read, fail that drive's sync: plan nothing and run nothing. A missing entry reads as a deletion. | Drive adapter contract | `sim_converge`, `bug_trust_incomplete_listing_is_caught`, `randomized` |
| 2 | **Re-check hashes before every local write or deletion.** This covers a download, a merge, either conflict write, `deleteLocal` and `deleteLocalFolder` (every file inside). If a hash differs from the listing, skip the step: it is not an error and records nothing. | Applying the plan | `sim_midsync`, `bug_no_recheck_is_caught` |
| 3 | **Upload what is read at upload time.** Upload the bytes read when the upload runs, and record their hash, not the hash in the listing. | Applying the plan | `sim_midsync`, `bug_record_listing_hash_is_caught` |
| 4 | **Make drive changes conditional.** Revision uploads and file deletions apply only to the listed version. A failed precondition is a step error that keeps the record. | Drive adapter contract | `sim_conditional`, `bug_unconditional_is_caught` |
| 5 | **Make new uploads create-only.** `uploadNew`, including the conflicted copy, must fail when the path now exists on the drive. The spec doesn't say this (see gap 4). | Conflicts (copy is "synced as a new note") | `story_conflict_name_race`, `bug_upload_new_overwrites_is_caught` |
| 6 | **Keep modification times.** Uploads carry the note's modification time. Downloads set the local time to the drive's. A merge result takes the current time. Syncing never sets "now" otherwise. | Modification time; Drive adapter contract | `sim_mtime`, `story_clock_skew_and_ties`, `bug_mtime_now_on_download_is_caught` |
| 7 | **Run conflict substeps in order.** First write the copy locally; if that fails, stop and report. Then overwrite the losing side and record the note. Then upload the copy. | Conflicts | `sim_order`, `bug_copy_after_overwrite_is_caught` |
| 8 | **Save state only after success.** Save a record (with its base copy for notes, in one transaction) only after the step succeeded. Drop records (`forget`, folder deletions) only after the deletion succeeded. `record` and `recordFolder` change nothing on disk. | Records and base copies; Sync plan, step 3 | `sim_fail`, `sim_manifest` |
| 9 | **Publish the manifest from records.** Build it with `buildManifest` from the records saved by steps that succeeded. Upload it only when it returns a value. | Sync manifest | `sim_manifest`, `sim_idemp` (also checked after every session) |
| 10 | **Keep pending conflicts durable.** Persist the pending conflict (note, copy, drive, base) right after the copy is written. Drop it only when it is resolved or a file is gone. | Conflicts | `story_edit_edit_conflict`, `sim_order` |
| 11 | **Use a new scheduler for each sync session.** It queues drives again, at most 3 passes per drive. A failed drive brings no changes. | Several drives | `sim_multidrive` |
| 12 | **Keep case-only renames in two steps.** Never "fix" case errors: report both paths and touch neither. To change only the case, rename to another name, sync, then rename again. | What is synced | `story_case_only_rename`, `gap_case_rename_missed_by_another_device` |
| 13 | **Verify attachments.** A download whose bytes don't match the hash in the name is a step error: write nothing and record nothing. Attachments get no base copy and are never merged. | Attachment sync | `story_attachments` |
| 14 | **Write durably in a crash-safe order.** For each step: drive change or local write, then record. For a conflict: copy, then pending conflict, then the overwrite, then the record, then the copy upload. A crash at any point is safe if this order holds, because the next sync replans from durable state. | Applying the plan; Conflicts | `sim_fail` (every substep × fail, lost response, crash), `story_lost_response_after_drive_commit`, `randomized` |

`sim_noloss` and `randomized` check all the rules together over many devices and drives.

## Gaps and ambiguities found

Each gap is kept as a test that documents today's behaviour.

1. **A drive folder deletion races a concurrent upload** (`gap_folder_delete_races_concurrent_upload`). The
   adapter contract makes only *file* deletions conditional, and Dropbox and OneDrive delete folders recursively.
   - If another device uploads into the folder between the listing and the folder deletion, the new note is
     deleted unseen.
   - That device then deletes its own copy as "deleted on the drive", so the note is lost.
   - Suggested fix: list the folder again just before deleting it, and fail the step if it holds anything not
     in the listing. Or delete the listed files conditionally and then the folder only if it is empty.
2. **A restored note is deleted through a stale record** (`gap_restored_note_deleted_through_stale_record`, several
   drives). The plan judges "unchanged" per drive, so a note put back with exactly its recorded bytes looks unchanged
   to a drive whose record predates the move.
   - In the test, a device moves `F/d.md` away and syncs only drive 0, then the user moves it back. Another device
     relays the deletion to drive 1.
   - The next sync deletes the note locally because of drive 1, and then deletes it from drive 0 as well.
   - Suggested fix: when a local deletion is synced to one drive, the other drives' records for that path should
     stop counting as "unchanged since the record". A later re-creation would then be a change.
3. **The modification time of a conflicted copy is not specified** (`gap_conflicted_copy_mtime`). The reference
   executor gives the copy the losing version's time, so syncing never makes it newer.
4. **The spec doesn't say new uploads are create-only** (rule 5). Without it, two devices that name their
   conflicted copies the same overwrite each other's copy (`bug_upload_new_overwrites_is_caught`).
5. **A crash after the copy is written duplicates the copy** (`gap_crash_after_copy_duplicates_it`). The next sync
   reconciles again and writes `… (conflicted copy) 2.md` with the same content. Nothing is lost, but the user sees
   a duplicate.
6. **A case-only rename seen only at its end state is stuck** (`gap_case_rename_missed_by_another_device`). A device
   that didn't sync between the two steps reports the case error until its user renames the note too.
