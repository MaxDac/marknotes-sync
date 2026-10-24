//! The independent no-loss oracle. It never reads sync records to decide what should exist: it follows what the
//! users did and compares path → bytes on every replica (see the module documentation of `sim` for the
//! definition).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use sha2::{Digest, Sha256};

use super::device::{Device, Node};
use super::drive::Drive;
use marknotes_sync::SyncRecord;

pub fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A note for the oracle: `.md` (any case) outside `attachments/`.
pub fn is_note_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") && !lower.starts_with("attachments/")
}

/// The hash in an attachment name, `attachments/<64 hex>.<ext>`.
pub fn attachment_hash(path: &str) -> Option<&str> {
    let file = path.strip_prefix("attachments/")?;
    let (hash, _ext) = file.split_once('.')?;
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

/// The text after the front-matter block (own implementation, not `front_matter::split`).
pub fn body(text: &str) -> &str {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = if let Some(r) = t.strip_prefix("---\n") {
        r
    } else if let Some(r) = t.strip_prefix("---\r\n") {
        r
    } else {
        return t;
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return &rest[offset..];
        }
    }
    t
}

/// The non-empty lines of `text`, without their line breaks.
pub fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n')
        .map(|l| l.trim_end_matches(['\r', '\n']))
        .filter(|l| !l.is_empty())
        .collect()
}

pub fn body_lines(text: &str) -> Vec<&str> {
    lines(body(text))
}

/// The lines a user write introduced.
#[derive(Clone, Debug)]
pub struct Version {
    pub label: String,
    pub own: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Oracle {
    /// Every body line a user ever typed.
    pub known: HashSet<String>,
    /// Lines a user removed (by editing or deleting a file holding them).
    pub retired: HashSet<String>,
    pub versions: Vec<Version>,
    /// Body → the modification times it may carry (when a user wrote it, or a merge produced it).
    pub origin_times: HashMap<String, BTreeSet<i64>>,
    /// Attachment path → (bytes, live).
    pub attachments: BTreeMap<String, (Vec<u8>, bool)>,
    /// (device, drive, path) → the drive bytes of the version last recorded for it: the agreed blob.
    pub agreed: HashMap<(usize, usize, String), Vec<u8>>,
    pub violations: Vec<String>,
    seen: HashSet<String>,
}

impl Oracle {
    pub fn violate(&mut self, at: &str, msg: String) {
        if self.seen.insert(msg.clone()) && self.violations.len() < 50 {
            self.violations.push(format!("{at}: {msg}"));
        }
    }

    pub fn user_wrote(&mut self, label: &str, old: Option<&[u8]>, new: &[u8], mtime: i64) {
        let new_text = String::from_utf8_lossy(new).into_owned();
        let new_lines: HashSet<&str> = body_lines(&new_text).into_iter().collect();
        if let Some(old) = old {
            let old_text = String::from_utf8_lossy(old).into_owned();
            for l in body_lines(&old_text) {
                if !new_lines.contains(l) {
                    self.retired.insert(l.to_string());
                }
            }
        }
        let own: Vec<String> = body_lines(&new_text)
            .into_iter()
            .filter(|l| !self.known.contains(*l))
            .map(str::to_string)
            .collect();
        for l in &own {
            self.known.insert(l.clone());
        }
        if !own.is_empty() {
            self.versions.push(Version {
                label: label.to_string(),
                own,
            });
        }
        self.origin_times
            .entry(body(&new_text).to_string())
            .or_default()
            .insert(mtime);
    }

    pub fn user_deleted(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        for l in body_lines(&text) {
            self.retired.insert(l.to_string());
        }
    }

    pub fn attachment_added(&mut self, path: &str, bytes: &[u8]) {
        self.attachments
            .insert(path.to_string(), (bytes.to_vec(), true));
    }

    pub fn attachment_deleted(&mut self, path: &str) {
        if let Some(a) = self.attachments.get_mut(path) {
            a.1 = false;
        }
    }

    /// A clean merge must keep every line either side added to the base, and add nothing else.
    pub fn merge_written(
        &mut self,
        at: &str,
        base: Option<&str>,
        local: &str,
        remote: &str,
        merged: &str,
        mtime: i64,
    ) {
        let Some(base) = base else {
            self.violate(at, "a merge was written without a base copy".into());
            return;
        };
        let base_lines: HashSet<&str> = lines(base).into_iter().collect();
        let merged_lines: HashSet<&str> = lines(merged).into_iter().collect();
        let local_lines: HashSet<&str> = lines(local).into_iter().collect();
        let remote_lines: HashSet<&str> = lines(remote).into_iter().collect();
        for side in [&local_lines, &remote_lines] {
            for l in side.iter() {
                if !base_lines.contains(l) && !merged_lines.contains(l) {
                    self.violate(at, format!("the clean merge dropped the added line {l:?}"));
                }
            }
        }
        for l in &merged_lines {
            if !local_lines.contains(l) && !remote_lines.contains(l) {
                self.violate(at, format!("the merge invented the line {l:?}"));
            }
        }
        self.origin_times
            .entry(body(merged).to_string())
            .or_default()
            .insert(mtime);
    }

    /// A saved file record must name a committed drive version whose bytes hash to the recorded local hash.
    pub fn on_record(&mut self, at: &str, dev: usize, r: usize, rec: &SyncRecord, drive: &Drive) {
        if rec.is_folder {
            return;
        }
        match drive.bytes_at(&rec.path, &rec.remote_version) {
            None => self.violate(
                at,
                format!(
                    "{}: record names drive version {} that was never committed",
                    rec.path, rec.remote_version
                ),
            ),
            Some(bytes) => {
                if sha_hex(bytes) != rec.local_hash {
                    self.violate(
                        at,
                        format!(
                            "{}: recorded hash does not match the drive's content of version {}",
                            rec.path, rec.remote_version
                        ),
                    );
                }
                self.agreed
                    .insert((dev, r, rec.path.clone()), bytes.clone());
            }
        }
    }

    pub fn on_forget(&mut self, dev: usize, r: usize, path: &str) {
        self.agreed.remove(&(dev, r, path.to_string()));
    }

    /// A sync deletion of a file: the listing was complete and the deleted bytes are the agreed blob.
    pub fn check_deletion(
        &mut self,
        at: &str,
        dev: usize,
        r: usize,
        path: &str,
        bytes: &[u8],
        listing_complete: bool,
    ) {
        if !listing_complete {
            self.violate(at, format!("{path}: deleted after an incomplete listing"));
        }
        match self.agreed.get(&(dev, r, path.to_string())) {
            Some(agreed) if agreed.as_slice() == bytes => {}
            Some(_) => self.violate(
                at,
                format!("{path}: deleted bytes differ from the last agreed content"),
            ),
            None => self.violate(at, format!("{path}: deleted although it was never agreed")),
        }
    }

    /// Every manifest entry must describe a committed drive version with that content.
    pub fn check_manifest(&mut self, at: &str, drive: &Drive) {
        let Some(text) = &drive.manifest else { return };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            self.violate(at, "the manifest is not JSON".into());
            return;
        };
        let Some(files) = value.get("files").and_then(|f| f.as_object()) else {
            return;
        };
        let mut bad = Vec::new();
        for (path, e) in files {
            let sha = e.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
            let version = e.get("version").and_then(|v| v.as_str()).unwrap_or("");
            match drive.bytes_at(path, version) {
                Some(b) if sha_hex(b) == sha => {}
                _ => bad.push(path.clone()),
            }
        }
        for p in bad {
            self.violate(
                at,
                format!(
                    "{p}: the manifest of {} lists a version/hash the drive never held",
                    drive.name
                ),
            );
        }
    }

    /// The global checks, after every durable transition.
    pub fn check_world(&mut self, devices: &[Device], drives: &[Drive], at: &str) {
        // Every note on every replica, as (where, mtime, text).
        let mut notes: Vec<(String, i64, String)> = Vec::new();
        let mut problems: Vec<String> = Vec::new();
        for d in devices {
            for (path, node) in &d.durable.files {
                if let Node::File { bytes, mtime } = node {
                    if is_note_path(path) {
                        notes.push((
                            format!("{}:{path}", d.name),
                            *mtime,
                            String::from_utf8_lossy(bytes).into_owned(),
                        ));
                    } else if let Some(h) = attachment_hash(path) {
                        if sha_hex(bytes) != h {
                            problems.push(format!(
                                "{}:{path}: attachment content does not match its name",
                                d.name
                            ));
                        }
                    }
                }
            }
        }
        for r in drives {
            for e in r.entries() {
                if !e.is_folder && is_note_path(&e.path) {
                    notes.push((
                        format!("{}:{}", r.name, e.path),
                        e.modified_millis,
                        String::from_utf8_lossy(&e.bytes).into_owned(),
                    ));
                }
            }
        }
        let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut file_lines: Vec<HashSet<&str>> = Vec::new();
        for (i, (place, mtime, text)) in notes.iter().enumerate() {
            match self.origin_times.get(body(text)) {
                None => problems.push(format!("{place}: content no user wrote and no merge produced")),
                Some(times) if !times.contains(mtime) => problems.push(format!(
                    "{place}: modification time {mtime} is not a time this content was written ({times:?})"
                )),
                _ => {}
            }
            let ls: HashSet<&str> = body_lines(text).into_iter().collect();
            for l in &ls {
                if !self.known.contains(*l) {
                    problems.push(format!("{place}: unexplained line {l:?}"));
                }
                index.entry(l).or_default().push(i);
            }
            file_lines.push(ls);
        }
        let mut lost: Vec<&String> = self
            .known
            .iter()
            .filter(|l| !self.retired.contains(*l) && !index.contains_key(l.as_str()))
            .collect();
        lost.sort();
        for l in lost {
            problems.push(format!("LOST: line {l:?} is on no device and no drive"));
        }
        for v in &self.versions {
            let live: Vec<&str> = v
                .own
                .iter()
                .filter(|l| !self.retired.contains(*l))
                .map(String::as_str)
                .collect();
            let Some(first) = live.first() else { continue };
            let Some(candidates) = index.get(first) else {
                continue; // already reported as lost
            };
            if !candidates
                .iter()
                .any(|&i| live.iter().all(|l| file_lines[i].contains(l)))
            {
                problems.push(format!(
                    "LOST: the lines of {} are no longer together in any file",
                    v.label
                ));
            }
        }
        for (path, (bytes, live)) in &self.attachments {
            if !live {
                continue;
            }
            let on_device = devices
                .iter()
                .any(|d| matches!(d.node(path), Some(Node::File { bytes: b, .. }) if b == bytes));
            let on_drive = drives.iter().any(|r| {
                r.entry(path)
                    .is_some_and(|e| !e.is_folder && &e.bytes == bytes)
            });
            if !on_device && !on_drive {
                problems.push(format!(
                    "LOST: attachment {path} is on no device and no drive"
                ));
            }
        }
        for p in problems {
            self.violate(at, p);
        }
    }
}
