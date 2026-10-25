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
| 2 | **Re-check hashes before every local write or deletion.** This covers a download, a merge, either conflict write and `deleteLocal`. If a hash differs from the listing, skip the step: it is not an error and records nothing. Folder removal is empty-only and atomic; a new child makes it fail without deleting anything. | Applying the plan | `sim_midsync`, `bug_no_recheck_is_caught` |
| 3 | **Upload what is read at upload time.** Upload the bytes read when the upload runs, and record their hash, not the hash in the listing. | Applying the plan | `sim_midsync`, `bug_record_listing_hash_is_caught` |
| 4 | **Make drive changes conditional.** Revision uploads and file deletions apply only to the listed version. A failed precondition is a step error that keeps the record. | Drive adapter contract | `sim_conditional`, `bug_unconditional_is_caught` |
| 5 | **Make creations create-only.** `uploadNew`, conflicted-copy uploads and `createRemoteFolder` fail when the path now exists. Never overwrite, adopt an existing folder, or fall back to autorename. | Drive adapter contract | `story_conflict_name_race`, `bug_upload_new_overwrites_is_caught`, `sync_executor_safety` |
| 6 | **Keep modification times.** Uploads carry the note's modification time. Downloads set the local time to the drive's. A merge result takes the current time. A conflicted copy uses `copyModifiedMillis`, the losing edit's time, on creation, replay and upload. Syncing never sets "now" otherwise. | Modification time; Drive adapter contract | `sim_mtime`, `gap_conflicted_copy_mtime`, `bug_mtime_now_on_download_is_caught` |
| 7 | **Run conflict substeps in order.** Persist the copy intent, then write the copy locally; if either fails, stop and report. Only after the verified copy is durable overwrite the losing side and record the note. Then upload the copy create-only. | Conflicts | `sim_order`, `bug_copy_after_overwrite_is_caught`, `sync_executor_safety` |
| 8 | **Save state only after success.** Save a record (with its base copy for notes, in one transaction) only after the step succeeded. Drop records (`forget`, folder deletions) only after the deletion succeeded. `record` and `recordFolder` change nothing on disk. | Records and base copies; Sync plan, step 3 | `sim_fail`, `sim_manifest` |
| 9 | **Publish the manifest from records.** Build it with `buildManifest` from the records saved by steps that succeeded. Upload it only when it returns a value. | Sync manifest | `sim_manifest`, `sim_idemp` (also checked after every session) |
| 10 | **Keep pending conflicts and replay intent durable.** Pending conflict metadata (note, copy, drive, base) is distinct from `conflictCopyIntent`. Scope intent to its drive and persist it before copy creation. Read the exact saved copy and pass `existingCopy` back to `reconcile`; use `reuseCopy` only after exact text and time verification. Clear intent only after winner and copy are durably recorded; preserve pending conflict metadata for the resolution UI. | Conflicts | `story_edit_edit_conflict`, `gap_crash_after_copy_duplicates_it`, `sync_executor_safety` |
| 11 | **Use a new scheduler for each sync session.** It queues drives again, at most 3 passes per drive. A failed drive brings no changes. | Several drives | `sim_multidrive` |
| 12 | **Run only record-proven case renames.** `renameLocal` and `renameRemote` carry old/new paths and the original remote entry; file local guards carry `sha256`, folders require the complete original subtree listing. Preserve bytes, modification times and ids; never overwrite another entry or use unannounced temporary remote names. Conditional remote rename must guard every stale revision. If the provider cannot guarantee it, report an explicit step error. Ambiguous/edited stale aliases remain case errors. | What is synced | `story_case_only_rename`, `gap_case_rename_missed_by_another_device`, `sync_plan` |
| 13 | **Verify attachments.** A download whose bytes don't match the hash in the name is a step error: write nothing and record nothing. Attachments get no base copy and are never merged. | Attachment sync | `story_attachments` |
| 14 | **Write durably in a crash-safe order.** For each step: drive change or local write, then record. For a conflict: durable intent, verified copy, overwrite, winner record, create-only copy upload, copy record, intent cleanup. Replay the original path/id/time only for the same conflict; stale intent or an edited/occupied copy must never block resolution or overwrite that copy. | Applying the plan; Conflicts | `sim_fail` (every substep × fail, lost response, crash), `sync_executor_safety`, `randomized` |
| 15 | **Delete known files individually, then handle empty folders deepest-first.** `deleteLocalFolder` and `deleteRemoteFolder` forget only the folder's own record. Atomic empty-only removal must not delete unseen/ignored descendants. Re-listing followed by recursive deletion is not safe. Explicitly declare `remoteFolderDeletion: "unsupported"` when the adapter lacks that guarantee; execute `preserveRemoteFolder` as durable state, never a delete request. An omitted capability remains strict `atomicEmptyOnly`; occupied folders, unsupported operations on that strict path and runtime failures remain step errors. | Sync plan; Drive adapter contract | `gap_folder_delete_races_concurrent_upload`, `sync_executor_safety`, `sim_fail` |
| 16 | **Track deletion epochs on every drive.** Before planning, run `localDeletions(local, records)` for all linked drives and durably flag the returned file records. Every successfully written record clears `localDeleted`. A restored file's unchanged bytes must not make its previous deletion valid again. | Records and base copies | `gap_restored_note_deleted_through_stale_record`, `sync_local_deletions`, `sync_plan` |
| 17 | **Migrate case-rename state per drive.** After success migrate that drive's record/base subtree and pending path metadata, preserving remote identity/version and `localDeleted`. Keep other drives' records/base paths unchanged until their own rename succeeds. If filesystem/provider rename and state migration cannot be atomic, persist a rename intent first and recover it before deletion-epoch detection or planning. | What is synced | `sync_plan`, `story_case_only_rename` |
| 18 | **Preserve unsupported folders with a durable inbox lifecycle.** Recheck local absence under the local-mutation/state transaction guard, retain the folder record bound to the action's `remote.id`, set `remoteFolderPreserved = true` and insert a unique pending manual-removal notice atomically. Scope notices to device/drive/path/remote id/deletion generation. Never suppress failed child steps, issue a remote delete or count a local change. Save failure is an error and retains the baseline for retry; lost responses/restarts reuse the committed lifecycle. Acknowledgement must be durably saved; failure leaves the same item pending, not a new toast/notice. | Records and base copies; Applying the plan | `sync_executor_safety` |
| 19 | **Retire preserved history on recreation across all drives before planning.** From a complete local listing call `recreatedFolders(local, records)` for every drive. Atomically remove only those preserved folder records and retire active notices; keep ordinary folder records, descendant file records and bases. Recover this transaction before planning after a crash. Removing just the marker would let a missing remote counterpart delete the recreated local folder. Normal folder recording clears preservation and retires its active notice; `forget` on complete remote absence or successful atomic deletion does likewise. Migrate preservation and notice paths with per-drive case-rename state. Missing markers migrate as false. | Records and base copies; Several drives | `sync_executor_safety`, `sync_plan` |
| 20 | **Do not transfer ancestor deletion authority across identities.** When recording a folder whose previous folder record has a different `remoteId`, atomically retire that drive's old descendant records, bases and active preservation notices with the new folder baseline. Do not remove local/remote bytes or acknowledged history. Otherwise a failed child download followed by restart could use stale descendant history to delete a child under the replacement parent. This is distinct from local recreation, which retains descendants. | Records and base copies | `sync_executor_safety` |

Recreation preflight must retain a durable, specifically associated old-root identity witness until root adoption
succeeds. Before planning from a complete drive listing, use that witness to retire stale descendant authority if
the root identity changed. Otherwise simultaneous recreation/replacement, or a failed folder-record save after
local creation, could erase the old identity before replacement detection. Do not derive this witness from
arbitrary archived notices or discard descendant history merely because the remote root is absent.

Use the shared `foldCase` helper (Rust: `paths::fold_case`) for path-alias and atomic no-clobber checks. Ordinary
lowercasing is not Unicode simple case folding and does not match the planner, for example for long s or final sigma.
If `renameLocal` names an absent exact old spelling but the desired target already exists, verify that target against
the action's hash or complete subtree listing, then complete only the current drive's record/base migration. This
history-only action repairs another drive's stale spelling after a previous local rename; never overwrite the target
or replace its remote identity or recorded version.

`sim_noloss` and `randomized` check all the rules together over many devices and drives.

## Execution-level fixture coverage

`sync/executor-safety.json` is executed against real reference stores, including races after listing and crashes
after intent, copy, winner, winner record, copy upload and copy record. The binding fixture runners consume the
observed reference-executor results generated with the parity corpus; they do not pretend a sans-IO binding can
exercise a provider. Each consuming app must also run its own adapter and persistence tests.

The historical `gap_*` names remain stable regression identifiers, but now assert the fixed behavior from
MaxDac/marknotes-spec#37 through MaxDac/marknotes-spec#42.
