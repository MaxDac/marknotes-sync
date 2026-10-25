//! The reference executor: runs the core's plan the way the spec ("Applying the plan", "Conflicts", "Several
//! drives", "Attachment sync", "Drive adapter contract") says an app must, one externally visible substep at a
//! time, each a fault point.

use std::collections::BTreeMap;

use marknotes_sync::{
    build_manifest, paths, reconcile, scheduler::DEFAULT_MAX_PASSES, Action, ConflictCopyIntent,
    ExistingCopy, LocalEntry, ReconcileInput, ReconcileOutcome, RemoteEntry, Scheduler, Side,
    SyncInput, SyncRecord,
};

use super::device::{FolderNotice, InFlight, Node, PendingConflict};
use super::oracle::{self, attachment_hash, sha_hex};
use super::world::{Point, When, World};

/// Why a step stopped.
#[derive(Debug)]
pub enum Stop {
    /// A step error, reported as `<path>: <message>`; the sync goes on.
    Error(String),
    /// The local file changed since the listing: skipped, not an error, nothing recorded.
    Skip(String),
    /// The device crashed: the sync in flight is gone.
    Crash,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Crashed;

#[derive(Debug, Default, Clone)]
pub struct SessionOutcome {
    pub drive: usize,
    /// The listing failed (or was incomplete): the sync of this drive failed.
    pub failed: bool,
    pub errors: Vec<String>,
    pub skipped: Vec<String>,
    pub local_changes: u32,
    pub actions: Vec<Action>,
}

pub fn kind(a: &Action) -> &'static str {
    match a {
        Action::RenameLocal { .. } => "renameLocal",
        Action::RenameRemote { .. } => "renameRemote",
        Action::Error { .. } => "error",
        Action::RecordFolder { .. } => "recordFolder",
        Action::CreateRemoteFolder { .. } => "createRemoteFolder",
        Action::CreateLocalFolder { .. } => "createLocalFolder",
        Action::Forget { .. } => "forget",
        Action::UploadNew { .. } => "uploadNew",
        Action::UploadRevision { .. } => "uploadRevision",
        Action::Download { .. } => "download",
        Action::Record { .. } => "record",
        Action::DeleteLocal { .. } => "deleteLocal",
        Action::DeleteRemote { .. } => "deleteRemote",
        Action::DeleteLocalFolder { .. } => "deleteLocalFolder",
        Action::DeleteRemoteFolder { .. } => "deleteRemoteFolder",
        Action::PreserveRemoteFolder { .. } => "preserveRemoteFolder",
        Action::Reconcile { .. } => "reconcile",
    }
}

pub fn path_of(a: &Action) -> &str {
    match a {
        Action::Error { path, .. }
        | Action::RenameLocal { path, .. }
        | Action::RenameRemote { path, .. }
        | Action::RecordFolder { path, .. }
        | Action::CreateRemoteFolder { path }
        | Action::CreateLocalFolder { path, .. }
        | Action::Forget { path }
        | Action::UploadNew { path }
        | Action::UploadRevision { path, .. }
        | Action::Download { path, .. }
        | Action::Record { path, .. }
        | Action::DeleteLocal { path }
        | Action::DeleteRemote { path, .. }
        | Action::DeleteLocalFolder { path, .. }
        | Action::DeleteRemoteFolder { path, .. }
        | Action::PreserveRemoteFolder { path, .. }
        | Action::Reconcile { path, .. } => path,
    }
}

/// Runs one substep: `apply` is the externally visible operation; the fault injector may fail it, lose its
/// response, or crash the device after it.
fn step<T>(
    w: &mut World,
    dev: usize,
    label: &'static str,
    commit: bool,
    apply: impl FnOnce(&mut World) -> Result<T, String>,
) -> Result<T, Stop> {
    match w.point(dev, label) {
        Point::Proceed => {
            let r = apply(w);
            if commit {
                w.after(label);
            }
            r.map_err(Stop::Error)
        }
        Point::Fail => Err(Stop::Error(format!("{label}: injected failure"))),
        Point::LostResponse if commit => {
            let _ = apply(w);
            w.after(label);
            Err(Stop::Error(format!("{label}: response lost")))
        }
        Point::LostResponse => Err(Stop::Error(format!("{label}: injected failure"))),
        Point::Crash => {
            if commit {
                let _ = apply(w);
                w.after(label);
            }
            w.crash(dev);
            Err(Stop::Crash)
        }
    }
}

struct Ctx {
    dev: usize,
    drive: usize,
    drive_name: String,
    complete: bool,
    remote_listed: Vec<RemoteEntry>,
}

/// Syncs every drive of `dev` with a new scheduler, as at the start of each sync session.
pub fn sync_all(w: &mut World, dev: usize) -> Result<Vec<SessionOutcome>, Crashed> {
    let names: Vec<String> = w.devices[dev]
        .drives
        .iter()
        .map(|&r| w.drives[r].name.clone())
        .collect();
    let mut scheduler = Scheduler::new(names.clone(), DEFAULT_MAX_PASSES);
    let mut outcomes = Vec::new();
    while let Some(id) = scheduler.next() {
        let r = w.devices[dev].drives[names.iter().position(|n| *n == id).unwrap()];
        let out = sync_session(w, dev, r)?;
        scheduler.report(&id, out.local_changes, out.failed);
        outcomes.push(out);
    }
    Ok(outcomes)
}

/// `sync_all`, then a restart if the device crashed.
pub fn sync(w: &mut World, dev: usize) -> Vec<SessionOutcome> {
    match sync_all(w, dev) {
        Ok(o) => o,
        Err(Crashed) => {
            w.restart(dev);
            Vec::new()
        }
    }
}

/// One sync of `dev` with drive `r`.
pub fn sync_session(w: &mut World, dev: usize, r: usize) -> Result<SessionOutcome, Crashed> {
    let drive_name = w.drives[r].name.clone();
    w.log(format!("{} syncs {drive_name}", w.devices[dev].name));
    let fired_before = w.hooks_fired;
    let faults_before = w.faults_hit.len();
    w.fire(|h| matches!(h, When::SessionStart { dev: d, drive } if *d == dev && drive.is_none_or(|x| x == r)));
    let mut out = SessionOutcome {
        drive: r,
        ..Default::default()
    };

    // Apps flush pending editor saves first: user writes are immediate here.
    let local = match step(w, dev, "list local", false, |w| Ok(w.devices[dev].list())) {
        Ok(l) => l,
        Err(Stop::Crash) => return Err(Crashed),
        Err(Stop::Error(e)) | Err(Stop::Skip(e)) => {
            out.failed = true;
            out.errors.push(e);
            return Ok(out);
        }
    };
    let listing = match step(w, dev, "list drive", false, |w| w.drives[r].list()) {
        Ok(l) => l,
        Err(Stop::Crash) => return Err(Crashed),
        Err(Stop::Error(e)) | Err(Stop::Skip(e)) => {
            out.failed = true;
            out.errors.push(e);
            return Ok(out);
        }
    };
    match step(w, dev, "flag local deletions", true, |w| {
        let durable = &mut w.devices[dev].durable;
        let mut recreated = Vec::new();
        for (drive, records) in &mut durable.records {
            let values: Vec<_> = records.values().cloned().collect();
            for path in marknotes_sync::local_deletions(&local, &values) {
                records.get_mut(&path).unwrap().local_deleted = true;
            }
            for path in marknotes_sync::recreated_folders(&local, &values) {
                let record = records.remove(&path).unwrap();
                recreated.push((drive.clone(), path, record.remote_id));
            }
        }
        for (drive, path, old_id) in recreated {
            durable.retire_folder_notice(&drive, &path);
            durable
                .recreated_folder_identities
                .insert((drive, path), old_id);
        }
        Ok(())
    }) {
        Ok(()) => {}
        Err(Stop::Crash) => return Err(Crashed),
        Err(Stop::Error(e)) | Err(Stop::Skip(e)) => {
            out.failed = true;
            out.errors.push(e);
            return Ok(out);
        }
    }
    if !listing.complete && !w.bugs.trust_incomplete_listing {
        out.failed = true;
        out.errors.push("listing incomplete".into());
        return Ok(out);
    }
    let replacements: Vec<_> = w.devices[dev]
        .durable
        .recreated_folder_identities
        .iter()
        .filter(|((drive, path), old_id)| {
            drive == &drive_name
                && listing
                    .entries
                    .iter()
                    .any(|e| e.path == *path && e.is_folder && e.id != **old_id)
        })
        .map(|((_, path), _)| path.clone())
        .collect();
    if !replacements.is_empty() {
        match step(w, dev, "recover folder identities", true, |w| {
            for path in replacements {
                retire_folder_descendants(w, dev, r, &drive_name, &path);
            }
            Ok(())
        }) {
            Ok(()) => {}
            Err(Stop::Crash) => return Err(Crashed),
            Err(Stop::Error(e)) | Err(Stop::Skip(e)) => {
                out.failed = true;
                out.errors.push(e);
                return Ok(out);
            }
        }
    }
    let input = SyncInput {
        local: local.clone(),
        remote: listing.entries.clone(),
        records: w.devices[dev].records(&drive_name),
        manifest: listing.manifest.clone(),
        remote_folder_deletion: w.drives[r].folder_deletion(),
    };
    let plan = marknotes_sync::plan(&input).actions;
    w.log(format!("  plan: {}", describe_plan(&plan)));
    let listed: BTreeMap<String, LocalEntry> =
        local.into_iter().map(|e| (e.path.clone(), e)).collect();
    w.devices[dev].volatile = Some(InFlight {
        drive: r,
        plan: plan.clone(),
        next_action: 0,
        listed: listed.clone(),
    });
    let ctx = Ctx {
        dev,
        drive: r,
        drive_name: drive_name.clone(),
        complete: listing.complete,
        remote_listed: listing.entries.clone(),
    };
    for (i, action) in plan.iter().enumerate() {
        let k = kind(action);
        let p = path_of(action).to_string();
        w.fire(|h| match h {
            When::BeforeAction { dev: d, kind, path } => {
                *d == dev && kind.is_none_or(|x| x == k) && path.as_ref().is_none_or(|x| *x == p)
            }
            _ => false,
        });
        let before = matches!(
            action,
            Action::Record { .. } | Action::RecordFolder { .. } | Action::Forget { .. }
        )
        .then(|| w.fingerprint());
        match run(w, &ctx, action, &listed, &mut out) {
            Ok(()) => {}
            Err(Stop::Error(e)) => {
                w.log(format!("  error {p}: {e}"));
                out.errors.push(format!("{p}: {e}"));
            }
            Err(Stop::Skip(why)) => {
                w.log(format!("  skipped {p}: {why}"));
                out.skipped.push(p.clone());
            }
            Err(Stop::Crash) => return Err(Crashed),
        }
        if let Some(before) = before {
            if before != w.fingerprint() {
                w.oracle.violate(
                    "record",
                    format!("{k} {p} changed bytes, modification times or ids"),
                );
            }
        }
        if let Some(v) = w.devices[dev].volatile.as_mut() {
            v.next_action = i + 1;
        }
    }
    out.actions = plan;
    let complete_intents: Vec<_> = w.devices[dev]
        .durable
        .copy_intents
        .iter()
        .filter_map(|(key, intent)| {
            if key.0 != drive_name {
                return None;
            }
            let d = &w.devices[dev];
            let (bytes, _) = d.file(&intent.copy_path)?;
            let record = d.record(&drive_name, &intent.copy_path)?;
            let remote = w.drives[r].entry(&intent.copy_path)?;
            (record.local_hash == sha_hex(bytes)
                && record.remote_id == remote.id
                && record.remote_version == remote.version)
                .then(|| key.clone())
        })
        .collect();
    if !complete_intents.is_empty() {
        match step(w, dev, "clear copy intent", true, |w| {
            for key in complete_intents {
                w.devices[dev].durable.copy_intents.remove(&key);
            }
            Ok(())
        }) {
            Ok(()) => {}
            Err(Stop::Crash) => return Err(Crashed),
            Err(Stop::Error(e)) | Err(Stop::Skip(e)) => out.errors.push(e),
        }
    }

    // The manifest, from the records only (so only successful steps), uploaded only when it changed.
    let records = w.devices[dev].records(&drive_name);
    let device_id = w.devices[dev].name.clone();
    let now = w.now(dev);
    if let Some(text) = build_manifest(&records, &device_id, now, listing.manifest.as_deref()) {
        match step(w, dev, "write manifest", true, |w| {
            w.drives[r].write_manifest(text);
            Ok(())
        }) {
            Ok(()) => {}
            Err(Stop::Crash) => return Err(Crashed),
            Err(Stop::Error(e)) | Err(Stop::Skip(e)) => out.errors.push(format!("manifest: {e}")),
        }
        let name = format!("manifest of {drive_name}");
        w.oracle.check_manifest(&name, &w.drives[r]);
    }

    // Pending conflicts are kept until one of the two files is gone.
    let d = &w.devices[dev];
    let stale = d
        .durable
        .pending_conflicts
        .iter()
        .any(|c| d.file(&c.note_path).is_none());
    if stale {
        let pruned = step(w, dev, "prune conflicts", true, |w| {
            let d = &mut w.devices[dev];
            let files = d.durable.files.clone();
            let present = |p: &str| matches!(files.get(p), Some(Node::File { .. }));
            d.durable
                .pending_conflicts
                .retain(|c| present(&c.note_path));
            Ok(())
        });
        if let Err(Stop::Crash) = pruned {
            return Err(Crashed);
        }
    }
    w.devices[dev].volatile = None;

    let disturbed = w.hooks_fired != fired_before || w.faults_hit.len() != faults_before;
    if w.check_idempotence
        && !disturbed
        && out.errors.is_empty()
        && out.skipped.is_empty()
        && !out
            .actions
            .iter()
            .any(|a| matches!(a, Action::RenameLocal { .. } | Action::RenameRemote { .. }))
    {
        let left = w.pending_actions(dev, r);
        if !left.is_empty() {
            w.oracle.violate(
                "idempotence",
                format!(
                    "{} after a clean sync of {drive_name}, the next plan is not empty: {}",
                    w.devices[dev].name,
                    describe_plan(&left)
                ),
            );
        }
    }
    Ok(out)
}

pub fn describe_plan(plan: &[Action]) -> String {
    plan.iter()
        .map(|a| format!("{} {}", kind(a), path_of(a)))
        .collect::<Vec<_>>()
        .join(", ")
}

// ----- substeps -----

/// The local file at `path` must be as listed (absent if it was not listed): otherwise the user edited it during
/// the sync and the step is skipped.
fn recheck(
    w: &World,
    dev: usize,
    path: &str,
    listed: &BTreeMap<String, LocalEntry>,
) -> Result<(), Stop> {
    if w.bugs.no_recheck {
        return Ok(());
    }
    let now = w.devices[dev].durable.files.get(path);
    let same = match (listed.get(path), now) {
        (None, None) => true,
        (Some(e), Some(Node::File { bytes, .. })) => {
            !e.is_folder && e.sha256.as_deref() == Some(&sha_hex(bytes))
        }
        (Some(e), Some(Node::Folder)) => e.is_folder,
        _ => false,
    };
    if same {
        Ok(())
    } else {
        Err(Stop::Skip(format!("{path} changed since the listing")))
    }
}

fn save_record(
    w: &mut World,
    ctx: &Ctx,
    rec: SyncRecord,
    base: Option<String>,
) -> Result<(), Stop> {
    let (dev, r, name) = (ctx.dev, ctx.drive, ctx.drive_name.clone());
    step(w, dev, "save record", true, |w| {
        w.oracle
            .on_record("save record", dev, r, &rec, &w.drives[r]);
        let d = &mut w.devices[dev].durable;
        // The record and the base copy are saved in one transaction.
        match base {
            Some(b) => {
                d.bases
                    .entry(name.clone())
                    .or_default()
                    .insert(rec.path.clone(), b);
            }
            None => {
                if let Some(m) = d.bases.get_mut(&name) {
                    m.remove(&rec.path);
                }
            }
        }
        d.records
            .entry(name)
            .or_default()
            .insert(rec.path.clone(), rec);
        Ok(())
    })
}

fn forget(
    w: &mut World,
    ctx: &Ctx,
    paths_: Vec<String>,
    under: Option<String>,
) -> Result<(), Stop> {
    let (dev, r, name) = (ctx.dev, ctx.drive, ctx.drive_name.clone());
    step(w, dev, "forget", true, |w| {
        let d = &mut w.devices[dev].durable;
        let records = d.records.entry(name.clone()).or_default();
        let bases = d.bases.entry(name.clone()).or_default();
        let mut gone = paths_;
        if let Some(root) = &under {
            gone.extend(
                records
                    .keys()
                    .chain(bases.keys())
                    .filter(|p| *p == root || paths::is_ancestor(root, p))
                    .cloned()
                    .collect::<Vec<_>>(),
            );
        }
        for p in &gone {
            records.remove(p);
            bases.remove(p);
            w.oracle.on_forget(dev, r, p);
        }
        for p in gone {
            d.retire_folder_notice(&name, &p);
            d.recreated_folder_identities.remove(&(name.clone(), p));
        }
        Ok(())
    })
}

fn file_record(path: &str, remote: &RemoteEntry, hash: String, now: i64) -> SyncRecord {
    SyncRecord {
        path: path.to_string(),
        is_folder: false,
        remote_id: remote.id.clone(),
        remote_version: remote.version.clone(),
        local_hash: hash,
        synced_at: now,
        local_deleted: false,
        remote_folder_preserved: false,
    }
}

fn note_base(path: &str, bytes: &[u8]) -> Option<String> {
    oracle::is_note_path(path).then(|| String::from_utf8_lossy(bytes).into_owned())
}

fn local_bytes(w: &World, dev: usize, path: &str) -> Option<(Vec<u8>, i64)> {
    w.devices[dev].file(path).map(|(b, m)| (b.to_vec(), m))
}

fn run(
    w: &mut World,
    ctx: &Ctx,
    action: &Action,
    listed: &BTreeMap<String, LocalEntry>,
    out: &mut SessionOutcome,
) -> Result<(), Stop> {
    let (dev, r) = (ctx.dev, ctx.drive);
    let now = w.now(dev);
    match action {
        Action::RenameLocal { .. } | Action::RenameRemote { .. } => {
            run_rename(w, ctx, action, listed)?;
            if matches!(action, Action::RenameLocal { .. }) {
                out.local_changes += 1;
            }
            Ok(())
        }
        Action::Error { message, .. } => Err(Stop::Error(message.clone())),
        Action::RecordFolder { path, remote_id } => save_folder_record(w, ctx, path, remote_id),
        Action::CreateRemoteFolder { path } => {
            let p = path.clone();
            let entry = step(w, dev, "drive create folder", true, |w| {
                w.drives[r].create_folder(&p)
            })?;
            save_folder_record(w, ctx, path, &entry.id)
        }
        Action::CreateLocalFolder { path, remote_id } => {
            let p = path.clone();
            step(w, dev, "create local folder", true, |w| {
                w.devices[dev].mkdir_p(&p);
                Ok(())
            })?;
            out.local_changes += 1;
            save_folder_record(w, ctx, path, remote_id)
        }
        Action::Forget { path } => forget(w, ctx, vec![path.clone()], None),
        Action::UploadNew { path } => {
            let Some((bytes, mtime)) = local_bytes(w, dev, path) else {
                return Err(Stop::Skip(format!("{path} is gone")));
            };
            let (p, b) = (path.clone(), bytes.clone());
            let overwrite = w.bugs.upload_new_overwrites;
            let entry = step(w, dev, "drive upload new", true, |w| {
                w.drives[r].create_file(&p, b, mtime, overwrite)
            })?;
            let hash = uploaded_hash(w, path, &bytes, listed);
            save_record(
                w,
                ctx,
                file_record(path, &entry, hash, now),
                note_base(path, &bytes),
            )
        }
        Action::UploadRevision { path, remote } => {
            let Some((bytes, mtime)) = local_bytes(w, dev, path) else {
                return Err(Stop::Skip(format!("{path} is gone")));
            };
            let (p, b, rm) = (path.clone(), bytes.clone(), remote.clone());
            let cond = (!w.bugs.unconditional).then_some(rm.version.clone());
            let entry = step(w, dev, "drive upload revision", true, |w| {
                w.drives[r].update_file(&p, &rm.id, cond.as_deref(), b, mtime)
            })?;
            let hash = uploaded_hash(w, path, &bytes, listed);
            save_record(
                w,
                ctx,
                file_record(path, &entry, hash, now),
                note_base(path, &bytes),
            )
        }
        Action::Download { path, remote } => {
            let (p, v) = (path.clone(), remote.version.clone());
            let (bytes, entry) = step(w, dev, "drive read", false, |w| w.drives[r].read(&p, &v))?;
            if paths::is_attachment_path(path) {
                let name_hash = attachment_hash(path).unwrap_or("");
                if sha_hex(&bytes) != name_hash {
                    return Err(Stop::Error(
                        "content does not match the hash in its name".into(),
                    ));
                }
            }
            recheck(w, dev, path, listed)?;
            let mtime = if w.bugs.mtime_now_on_download {
                now
            } else {
                entry.modified_millis
            };
            let (p, b) = (path.clone(), bytes.clone());
            step(w, dev, "write local", true, |w| {
                w.devices[dev].write(&p, b, mtime);
                Ok(())
            })?;
            out.local_changes += 1;
            save_record(
                w,
                ctx,
                file_record(path, &entry, sha_hex(&bytes), now),
                note_base(path, &bytes),
            )
        }
        Action::Record {
            path,
            remote,
            sha256,
            update_base,
        } => {
            let base = if *update_base {
                let Some((bytes, _)) = local_bytes(w, dev, path) else {
                    return Err(Stop::Skip(format!("{path} is gone")));
                };
                if sha_hex(&bytes) != *sha256 {
                    return Err(Stop::Skip(format!("{path} changed since the listing")));
                }
                note_base(path, &bytes)
            } else {
                w.devices[dev].base(&ctx.drive_name, path).cloned()
            };
            save_record(w, ctx, file_record(path, remote, sha256.clone(), now), base)
        }
        Action::DeleteLocal { path } => {
            recheck(w, dev, path, listed)?;
            let p = path.clone();
            let complete = ctx.complete;
            step(w, dev, "delete local", true, |w| {
                if let Some(Node::File { bytes, .. }) = w.devices[dev].durable.files.remove(&p) {
                    w.oracle
                        .check_deletion("delete local", dev, r, &p, &bytes, complete);
                }
                Ok(())
            })?;
            out.local_changes += 1;
            forget(w, ctx, vec![path.clone()], None)
        }
        Action::DeleteRemote { path, remote } => {
            let p = path.clone();
            let cond = (!w.bugs.unconditional).then_some(remote.version.clone());
            let complete = ctx.complete;
            step(w, dev, "drive delete", true, |w| {
                let bytes = w.drives[r].entry(&p).map(|e| e.bytes.clone());
                w.drives[r].delete_file(&p, cond.as_deref())?;
                if let Some(bytes) = bytes {
                    w.oracle
                        .check_deletion("drive delete", dev, r, &p, &bytes, complete);
                }
                Ok(())
            })?;
            forget(w, ctx, vec![path.clone()], None)
        }
        Action::DeleteLocalFolder { path, forget: f } => {
            let p = path.clone();
            step(w, dev, "delete local folder", true, |w| {
                if w.devices[dev]
                    .durable
                    .files
                    .keys()
                    .any(|q| paths::is_ancestor(&p, q))
                {
                    return Err("local folder is not empty".into());
                }
                w.devices[dev].durable.files.remove(&p);
                Ok(())
            })?;
            out.local_changes += 1;
            forget(w, ctx, f.clone(), None)
        }
        Action::DeleteRemoteFolder {
            path,
            remote,
            forget: f,
        } => {
            let p = path.clone();
            let id = remote.id.clone();
            step(w, dev, "drive delete folder", true, |w| {
                w.drives[r].delete_empty_folder(&p, &id)
            })?;
            forget(w, ctx, f.clone(), None)
        }
        Action::PreserveRemoteFolder { path, remote } => {
            step(w, dev, "check preserve remote folder", false, |_| Ok(()))?;
            recheck(w, dev, path, listed)?;
            step(w, dev, "preserve remote folder", true, |w| {
                let d = &mut w.devices[dev].durable;
                if d.files
                    .keys()
                    .any(|p| p == path || paths::is_ancestor(path, p))
                {
                    return Err("local folder was recreated".into());
                }
                let record = d
                    .records
                    .get_mut(&ctx.drive_name)
                    .and_then(|records| records.get_mut(path))
                    .filter(|record| record.is_folder && record.remote_id == remote.id)
                    .ok_or("folder record identity changed")?;
                if !record.remote_folder_preserved {
                    record.remote_folder_preserved = true;
                    d.next_folder_generation += 1;
                    d.folder_notices.push(FolderNotice {
                        drive: ctx.drive_name.clone(),
                        path: path.clone(),
                        remote_id: remote.id.clone(),
                        generation: d.next_folder_generation,
                        acknowledged: false,
                        active: true,
                    });
                }
                Ok(())
            })
        }
        Action::Reconcile {
            path,
            remote,
            has_record,
        } => run_reconcile(w, ctx, path, remote, *has_record, listed, out),
    }
}

fn run_rename(
    w: &mut World,
    ctx: &Ctx,
    action: &Action,
    listed: &BTreeMap<String, LocalEntry>,
) -> Result<(), Stop> {
    let (path, to_path, local) = match action {
        Action::RenameLocal { path, to_path, .. } => (path.clone(), to_path.clone(), true),
        Action::RenameRemote { path, to_path, .. } => (path.clone(), to_path.clone(), false),
        _ => unreachable!(),
    };
    let dev = ctx.dev;
    let metadata_only =
        local && w.devices[dev].node(&path).is_none() && w.devices[dev].node(&to_path).is_some();
    if local {
        let source = if metadata_only { &to_path } else { &path };
        let inside = |p: &str| p == source || paths::is_ancestor(source, p);
        let current = w.devices[dev].list();
        let current_paths: Vec<_> = current
            .iter()
            .filter(|e| inside(&e.path))
            .map(|e| &e.path)
            .collect();
        let expected_paths: Vec<_> = listed.keys().filter(|p| inside(p)).collect();
        if current_paths != expected_paths
            || (!metadata_only && w.devices[dev].node(&to_path).is_some())
        {
            return Err(Stop::Skip("rename tree changed".into()));
        }
        for p in expected_paths {
            recheck(w, dev, p, listed)?;
        }
    }
    step(
        w,
        dev,
        if local {
            "rename local"
        } else {
            "rename remote"
        },
        true,
        |w| {
            if local && !metadata_only {
                let entries = w.devices[dev].remove_tree(&path);
                for (p, n) in entries {
                    w.devices[dev]
                        .durable
                        .files
                        .insert(format!("{to_path}{}", &p[path.len()..]), n);
                }
            } else if !local {
                w.drives[ctx.drive].rename_case(&path, &to_path, &ctx.remote_listed)?;
            }
            // Rename and state migration are a durable transaction in the reference store.
            let d = &mut w.devices[dev].durable;
            for notice in &mut d.folder_notices {
                if notice.drive == ctx.drive_name
                    && (notice.path == path || paths::is_ancestor(&path, &notice.path))
                {
                    notice.path = format!("{to_path}{}", &notice.path[path.len()..]);
                }
            }
            let witnesses: Vec<_> = d
                .recreated_folder_identities
                .keys()
                .filter(|(drive, p)| {
                    drive == &ctx.drive_name && (p == &path || paths::is_ancestor(&path, p))
                })
                .cloned()
                .collect();
            for key in witnesses {
                let old_id = d.recreated_folder_identities.remove(&key).unwrap();
                d.recreated_folder_identities.insert(
                    (key.0, format!("{to_path}{}", &key.1[path.len()..])),
                    old_id,
                );
            }
            if let Some(records) = d.records.get_mut(&ctx.drive_name) {
                let old: Vec<_> = records
                    .keys()
                    .filter(|p| *p == &path || paths::is_ancestor(&path, p))
                    .cloned()
                    .collect();
                for p in old {
                    let mut record = records.remove(&p).unwrap();
                    record.path = format!("{to_path}{}", &p[path.len()..]);
                    records.insert(record.path.clone(), record);
                }
            }
            if let Some(bases) = d.bases.get_mut(&ctx.drive_name) {
                let old: Vec<_> = bases
                    .keys()
                    .filter(|p| *p == &path || paths::is_ancestor(&path, p))
                    .cloned()
                    .collect();
                for p in old {
                    let base = bases.remove(&p).unwrap();
                    bases.insert(format!("{to_path}{}", &p[path.len()..]), base);
                }
            }
            Ok(())
        },
    )
}

/// The hash recorded for an upload: of the bytes actually uploaded (the buggy executor records the listing's).
fn uploaded_hash(
    w: &World,
    path: &str,
    bytes: &[u8],
    listed: &BTreeMap<String, LocalEntry>,
) -> String {
    if w.bugs.record_listing_hash {
        if let Some(h) = listed.get(path).and_then(|e| e.sha256.clone()) {
            return h;
        }
    }
    sha_hex(bytes)
}

fn save_folder_record(w: &mut World, ctx: &Ctx, path: &str, remote_id: &str) -> Result<(), Stop> {
    let (dev, name) = (ctx.dev, ctx.drive_name.clone());
    let rec = SyncRecord {
        path: path.to_string(),
        is_folder: true,
        remote_id: remote_id.to_string(),
        remote_version: String::new(),
        local_hash: String::new(),
        synced_at: w.now(dev),

        local_deleted: false,

        remote_folder_preserved: false,
    };
    step(w, dev, "save record", true, |w| {
        let d = &mut w.devices[dev].durable;
        let replaced = d
            .records
            .get(&name)
            .and_then(|records| records.get(path))
            .is_some_and(|old| old.is_folder && old.remote_id != remote_id);
        if replaced {
            retire_folder_descendants(w, dev, ctx.drive, &name, path);
        }
        let d = &mut w.devices[dev].durable;
        d.recreated_folder_identities
            .remove(&(name.clone(), path.to_string()));
        d.retire_folder_notice(&name, path);
        d.records
            .entry(name)
            .or_default()
            .insert(rec.path.clone(), rec);
        Ok(())
    })
}

fn retire_folder_descendants(w: &mut World, dev: usize, drive: usize, name: &str, path: &str) {
    let d = &mut w.devices[dev].durable;
    let descendants: Vec<_> = d
        .records
        .get(name)
        .into_iter()
        .flat_map(|r| r.keys())
        .chain(d.bases.get(name).into_iter().flat_map(|b| b.keys()))
        .filter(|p| paths::is_ancestor(path, p))
        .cloned()
        .collect();
    for child in descendants {
        if let Some(records) = d.records.get_mut(name) {
            records.remove(&child);
        }
        if let Some(bases) = d.bases.get_mut(name) {
            bases.remove(&child);
        }
        w.oracle.on_forget(dev, drive, &child);
    }
    for notice in &mut d.folder_notices {
        if notice.drive == name && paths::is_ancestor(path, &notice.path) {
            notice.active = false;
        }
    }
}

pub fn acknowledge_folder_notice(w: &mut World, dev: usize, generation: u64) -> Result<(), Stop> {
    step(w, dev, "acknowledge folder notice", true, |w| {
        let notice = w.devices[dev]
            .durable
            .folder_notices
            .iter_mut()
            .find(|notice| notice.generation == generation)
            .ok_or("folder notice not found")?;
        notice.acknowledged = true;
        Ok(())
    })
}

fn run_reconcile(
    w: &mut World,
    ctx: &Ctx,
    path: &str,
    remote: &RemoteEntry,
    has_record: bool,
    listed: &BTreeMap<String, LocalEntry>,
    out: &mut SessionOutcome,
) -> Result<(), Stop> {
    let (dev, r) = (ctx.dev, ctx.drive);
    let (p, v) = (path.to_string(), remote.version.clone());
    let (remote_bytes, entry) = step(w, dev, "drive read", false, |w| w.drives[r].read(&p, &v))?;
    recheck(w, dev, path, listed)?;
    let Some((local, local_mtime)) = local_bytes(w, dev, path) else {
        return Err(Stop::Skip(format!("{path} is gone")));
    };
    let local_text = String::from_utf8_lossy(&local).into_owned();
    let remote_text = String::from_utf8_lossy(&remote_bytes).into_owned();
    let base = if has_record {
        w.devices[dev].base(&ctx.drive_name, path).cloned()
    } else {
        None
    };
    let existing_paths: Vec<String> = w.devices[dev].durable.files.keys().cloned().collect();
    let new_id = w.new_id();
    let conflict_copy_intent = w.devices[dev]
        .durable
        .copy_intents
        .get(&(ctx.drive_name.clone(), path.to_string()))
        .cloned();
    let existing_copy = conflict_copy_intent.as_ref().and_then(|c| {
        w.devices[dev]
            .file(&c.copy_path)
            .map(|(bytes, modified_millis)| ExistingCopy {
                path: c.copy_path.clone(),
                text: String::from_utf8_lossy(bytes).into_owned(),
                modified_millis,
            })
    });
    let outcome = reconcile(&ReconcileInput {
        path: path.to_string(),
        local: local_text.clone(),
        local_modified_millis: local_mtime,
        remote: remote_text.clone(),
        remote_entry: entry.clone(),
        base: base.clone(),
        existing_paths,
        new_id,
        conflict_copy_intent,
        existing_copy,
    });
    let now = w.now(dev);
    match outcome {
        ReconcileOutcome::Same => save_record(
            w,
            ctx,
            file_record(path, &entry, sha_hex(&local), now),
            Some(local_text),
        ),
        ReconcileOutcome::Merged { text } => {
            w.log(format!("  merged {path}"));
            let merged_at = w.now(dev);
            w.oracle.merge_written(
                &format!("merge of {path}"),
                base.as_deref(),
                &local_text,
                &remote_text,
                &text,
                merged_at,
            );
            recheck(w, dev, path, listed)?;
            let (p, t) = (path.to_string(), text.clone());
            step(w, dev, "write local", true, |w| {
                w.devices[dev].write(&p, t.into_bytes(), merged_at);
                Ok(())
            })?;
            out.local_changes += 1;
            let (p, t) = (path.to_string(), text.clone());
            let cond = (!w.bugs.unconditional).then_some(entry.version.clone());
            let id = entry.id.clone();
            let uploaded = step(w, dev, "drive upload revision", true, |w| {
                w.drives[r].update_file(&p, &id, cond.as_deref(), t.into_bytes(), merged_at)
            })?;
            save_record(
                w,
                ctx,
                file_record(path, &uploaded, sha_hex(text.as_bytes()), now),
                Some(text),
            )
        }
        ReconcileOutcome::NewestWins {
            winner,
            copy_path,
            copy_text,
            copy_modified_millis,
            conflict_copy_intent,
            reuse_copy,
        } => {
            w.log(format!("  {winner:?} wins {path}, copy {copy_path}"));
            let loser_mtime = copy_modified_millis;
            if !w.bugs.copy_after_overwrite && !reuse_copy {
                write_copy(
                    w,
                    ctx,
                    path,
                    &copy_path,
                    &copy_text,
                    loser_mtime,
                    base.clone(),
                    conflict_copy_intent.clone(),
                    out,
                )?;
            }
            match winner {
                Side::Remote => {
                    recheck(w, dev, path, listed)?;
                    let (p, b) = (path.to_string(), remote_bytes.clone());
                    let mtime = entry.modified_millis;
                    step(w, dev, "write local", true, |w| {
                        w.devices[dev].write(&p, b, mtime);
                        Ok(())
                    })?;
                    out.local_changes += 1;
                    save_record(
                        w,
                        ctx,
                        file_record(path, &entry, sha_hex(&remote_bytes), now),
                        Some(remote_text),
                    )?;
                }
                Side::Local => {
                    let (p, b) = (path.to_string(), local.clone());
                    let cond = (!w.bugs.unconditional).then_some(entry.version.clone());
                    let id = entry.id.clone();
                    let uploaded = step(w, dev, "drive upload revision", true, |w| {
                        w.drives[r].update_file(&p, &id, cond.as_deref(), b, local_mtime)
                    })?;
                    save_record(
                        w,
                        ctx,
                        file_record(path, &uploaded, sha_hex(&local), now),
                        Some(local_text),
                    )?;
                }
            }
            if w.bugs.copy_after_overwrite && !reuse_copy {
                write_copy(
                    w,
                    ctx,
                    path,
                    &copy_path,
                    &copy_text,
                    loser_mtime,
                    base,
                    conflict_copy_intent,
                    out,
                )?;
            }
            let (cp, ct) = (copy_path.clone(), copy_text.clone());
            let already_uploaded = w.devices[dev]
                .record(&ctx.drive_name, &copy_path)
                .zip(w.drives[r].entry(&copy_path))
                .is_some_and(|(record, remote)| {
                    record.remote_id == remote.id
                        && record.remote_version == remote.version
                        && record.local_hash == sha_hex(copy_text.as_bytes())
                        && remote.bytes == copy_text.as_bytes()
                });
            if already_uploaded {
                return Ok(());
            }
            let overwrite = w.bugs.upload_new_overwrites;
            let uploaded = step(w, dev, "upload copy", true, |w| {
                w.drives[r].create_file(&cp, ct.into_bytes(), loser_mtime, overwrite)
            })?;
            save_record(
                w,
                ctx,
                file_record(&copy_path, &uploaded, sha_hex(copy_text.as_bytes()), now),
                Some(copy_text),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_copy(
    w: &mut World,
    ctx: &Ctx,
    path: &str,
    copy_path: &str,
    copy_text: &str,
    mtime: i64,
    base: Option<String>,
    intent: ConflictCopyIntent,
    out: &mut SessionOutcome,
) -> Result<(), Stop> {
    let dev = ctx.dev;
    if w.devices[dev].node(copy_path).is_some() {
        return Err(Stop::Skip(format!("{copy_path} appeared during the sync")));
    }
    let conflict = PendingConflict {
        drive: ctx.drive_name.clone(),
        note_path: path.to_string(),
        copy_path: copy_path.to_string(),
        base,
        intent,
    };
    step(w, dev, "save pending conflict", true, |w| {
        w.devices[dev].durable.copy_intents.insert(
            (conflict.drive.clone(), conflict.note_path.clone()),
            conflict.intent.clone(),
        );
        w.devices[dev]
            .durable
            .pending_conflicts
            .retain(|c| c.drive != conflict.drive || c.note_path != conflict.note_path);
        w.devices[dev].durable.pending_conflicts.push(conflict);
        Ok(())
    })?;
    let (cp, ct) = (copy_path.to_string(), copy_text.to_string());
    step(w, dev, "write copy", true, |w| {
        w.devices[dev].write(&cp, ct.into_bytes(), mtime);
        Ok(())
    })
    .map_err(|e| match e {
        Stop::Error(m) => Stop::Error(format!("the conflicted copy could not be written: {m}")),
        other => other,
    })?;
    out.local_changes += 1;
    Ok(())
}
