//! The world: devices, drives, the oracle, a clock, the fault injector, mid-sync hooks and the user operations.

use std::collections::BTreeMap;

use marknotes_sync::{hashing::sha256_hex, paths, Action, SyncInput, SyncPlan};

use super::device::{Device, Node};
use super::drive::Drive;
use super::oracle::{self, Oracle};
use super::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// The substep does not happen and reports an error.
    Fail,
    /// The substep happens but the caller sees an error (a commit whose response was lost).
    LostResponse,
    /// The substep happens, then the device crashes: the sync in flight is lost.
    Crash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaultAt {
    /// The n-th externally visible substep of the world (`World::steps` after it is counted).
    Step(u64),
    /// The next substep with this label on this device.
    Label(usize, &'static str),
}

#[derive(Clone, Debug)]
pub struct Fault {
    pub at: FaultAt,
    pub kind: FaultKind,
}

/// What the executor does at a substep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Proceed,
    Fail,
    LostResponse,
    Crash,
}

/// Deliberate executor bugs, each breaking one rule of the executor contract. The harness tests check that the
/// oracle flags every one of them, so that it is not vacuous.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bugs {
    /// Local writes and deletions do not hash the file again.
    pub no_recheck: bool,
    /// The conflicted copy is written after the losing side is overwritten.
    pub copy_after_overwrite: bool,
    /// Revision uploads and file deletions on the drive are not conditional.
    pub unconditional: bool,
    /// New files (including conflicted copies) overwrite an existing drive file.
    pub upload_new_overwrites: bool,
    /// Uploads record the hash from the listing instead of the hash of the uploaded bytes.
    pub record_listing_hash: bool,
    /// An incomplete listing is planned as if it were complete.
    pub trust_incomplete_listing: bool,
    /// Downloads set the local modification time to the current time.
    pub mtime_now_on_download: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum When {
    /// Before the executor runs an action on `dev`, optionally of one kind (`Action` tag) and path.
    BeforeAction {
        dev: usize,
        kind: Option<&'static str>,
        path: Option<String>,
    },
    /// When `dev` starts syncing a drive (before listing).
    SessionStart { dev: usize, drive: Option<usize> },
}

pub type HookOp = Box<dyn FnMut(&mut World)>;

pub struct Hook {
    pub when: When,
    pub repeat: bool,
    pub op: HookOp,
}

/// How a user edits a note: always with fresh lines, so that the oracle can follow every line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Append,
    Prepend,
    ReplaceFirst,
    RemoveFirst,
    RemoveLast,
}

pub const EDITS: [Edit; 5] = [
    Edit::Append,
    Edit::Prepend,
    Edit::ReplaceFirst,
    Edit::RemoveFirst,
    Edit::RemoveLast,
];

pub struct World {
    pub devices: Vec<Device>,
    pub drives: Vec<Drive>,
    pub oracle: Oracle,
    pub time: i64,
    pub steps: u64,
    pub fault: Option<Fault>,
    pub faults_hit: Vec<String>,
    pub hooks: Vec<Hook>,
    pub bugs: Bugs,
    pub rng: Rng,
    pub log: Vec<String>,
    /// Labels of the substeps, in order (for fault sweeps and debugging).
    pub trace: Vec<String>,
    pub check_idempotence: bool,
    pub hooks_fired: u64,
    counter: u64,
}

const ID_ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

impl World {
    /// `links[i]` lists the drives device `i` is linked to.
    pub fn new(drives: usize, links: &[&[usize]], seed: u64) -> World {
        World {
            devices: links
                .iter()
                .enumerate()
                .map(|(i, l)| Device::new(&format!("dev{i}"), l.to_vec()))
                .collect(),
            drives: (0..drives)
                .map(|i| Drive::new(&format!("drive{i}")))
                .collect(),
            oracle: Oracle::default(),
            time: 1_700_000_000_000,
            steps: 0,
            fault: None,
            faults_hit: Vec::new(),
            hooks: Vec::new(),
            bugs: Bugs::default(),
            rng: Rng::new(seed),
            log: Vec::new(),
            trace: Vec::new(),
            check_idempotence: true,
            hooks_fired: 0,
            counter: 0,
        }
    }

    /// Two devices and one drive.
    pub fn pair() -> World {
        World::new(1, &[&[0], &[0]], 1)
    }

    pub fn now(&self, dev: usize) -> i64 {
        self.time + self.devices[dev].skew
    }

    pub fn new_id(&mut self) -> String {
        (0..8)
            .map(|_| ID_ALPHABET[self.rng.below(ID_ALPHABET.len())] as char)
            .collect()
    }

    /// A line no one has written before.
    pub fn fresh_line(&mut self, dev: usize) -> String {
        self.counter += 1;
        format!("d{dev}-{} {}", self.counter, self.rng.next_u64() % 1000)
    }

    pub fn log(&mut self, s: impl Into<String>) {
        self.log.push(s.into());
    }

    // ----- fault injection -----

    /// Called by the executor at every externally visible substep.
    pub fn point(&mut self, dev: usize, label: &'static str) -> Point {
        self.steps += 1;
        self.time += 1;
        self.trace
            .push(format!("{}:{label}", self.devices[dev].name));
        let hit = match &self.fault {
            Some(Fault {
                at: FaultAt::Step(n),
                ..
            }) => *n == self.steps,
            Some(Fault {
                at: FaultAt::Label(d, l),
                ..
            }) => *d == dev && *l == label,
            None => false,
        };
        if !hit {
            return Point::Proceed;
        }
        let kind = self.fault.take().map(|f| f.kind).unwrap_or(FaultKind::Fail);
        self.faults_hit.push(format!(
            "{kind:?} at step {} ({}:{label})",
            self.steps, self.devices[dev].name
        ));
        self.log(format!(
            "FAULT {kind:?} at {}:{label}",
            self.devices[dev].name
        ));
        match kind {
            FaultKind::Fail => Point::Fail,
            FaultKind::LostResponse => Point::LostResponse,
            FaultKind::Crash => Point::Crash,
        }
    }

    pub fn fail_at(&mut self, dev: usize, label: &'static str, kind: FaultKind) {
        self.fault = Some(Fault {
            at: FaultAt::Label(dev, label),
            kind,
        });
    }

    /// The oracle, after every durable transition.
    pub fn after(&mut self, at: &str) {
        let label = format!("{at} (step {})", self.steps);
        self.oracle.check_world(&self.devices, &self.drives, &label);
    }

    pub fn crash(&mut self, dev: usize) {
        self.log(format!("{} crashes", self.devices[dev].name));
        self.devices[dev].volatile = None;
    }

    /// A restart starts from the durable state only: there is nothing to resume.
    pub fn restart(&mut self, dev: usize) {
        self.log(format!("{} restarts", self.devices[dev].name));
        self.devices[dev].volatile = None;
    }

    pub fn violations(&self) -> &[String] {
        &self.oracle.violations
    }

    pub fn assert_no_violations(&self) {
        if !self.oracle.violations.is_empty() {
            panic!(
                "oracle violations:\n  {}\nlog:\n  {}",
                self.oracle.violations.join("\n  "),
                self.log.join("\n  ")
            );
        }
    }

    // ----- hooks -----

    pub fn hook(&mut self, when: When, op: impl FnMut(&mut World) + 'static) {
        self.hooks.push(Hook {
            when,
            repeat: false,
            op: Box::new(op),
        });
    }

    pub fn hook_repeat(&mut self, when: When, op: impl FnMut(&mut World) + 'static) {
        self.hooks.push(Hook {
            when,
            repeat: true,
            op: Box::new(op),
        });
    }

    /// Runs the hooks matching `when`; returns whether any ran. Hooks do not fire inside hooks.
    pub fn fire(&mut self, matches: impl Fn(&When) -> bool) -> bool {
        if self.hooks.is_empty() {
            return false;
        }
        let mut hooks = std::mem::take(&mut self.hooks);
        let mut fired = false;
        let mut i = 0;
        while i < hooks.len() {
            if matches(&hooks[i].when) {
                fired = true;
                self.hooks_fired += 1;
                (hooks[i].op)(self);
                if !hooks[i].repeat {
                    hooks.remove(i);
                    continue;
                }
            }
            i += 1;
        }
        hooks.append(&mut self.hooks);
        self.hooks = hooks;
        fired
    }

    // ----- user operations -----

    fn user_write(&mut self, dev: usize, path: &str, bytes: Vec<u8>, what: &str) {
        let mtime = self.now(dev);
        let old = self.devices[dev].file(path).map(|(b, _)| b.to_vec());
        if oracle::is_note_path(path) {
            let label = format!("{what} {}:{path} @{mtime}", self.devices[dev].name);
            self.oracle
                .user_wrote(&label, old.as_deref(), &bytes, mtime);
        }
        self.log(format!(
            "{} {what} {path}: {:?}",
            self.devices[dev].name,
            String::from_utf8_lossy(&bytes)
        ));
        self.devices[dev].write(path, bytes, mtime);
        self.after(&format!("user {what} {path}"));
    }

    /// Creates a note with an id and two fresh lines.
    pub fn create(&mut self, dev: usize, path: &str) -> String {
        self.time += 1000;
        let id = self.new_id();
        let (a, b) = (self.fresh_line(dev), self.fresh_line(dev));
        let text = format!("---\nid: {id}\n---\n{a}\n{b}\n");
        self.user_write(dev, path, text.clone().into_bytes(), "creates");
        text
    }

    /// Creates a note without front matter (as written by another editor).
    pub fn create_plain(&mut self, dev: usize, path: &str) -> String {
        self.time += 1000;
        let (a, b) = (self.fresh_line(dev), self.fresh_line(dev));
        let text = format!("{a}\n{b}\n");
        self.user_write(dev, path, text.clone().into_bytes(), "creates");
        text
    }

    /// Writes `text` as a user edit (its new lines must be fresh).
    pub fn set_text(&mut self, dev: usize, path: &str, text: &str) {
        self.time += 1000;
        self.user_write(dev, path, text.as_bytes().to_vec(), "writes");
    }

    pub fn edit(&mut self, dev: usize, path: &str, how: Edit) -> String {
        self.time += 1000;
        let old = self.devices[dev].text(path).expect("edit: no such note");
        let (header, body) = marknotes_sync::front_matter::split(&old);
        let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
        match how {
            Edit::Append => lines.push(self.fresh_line(dev)),
            Edit::Prepend => lines.insert(0, self.fresh_line(dev)),
            Edit::ReplaceFirst if !lines.is_empty() => lines[0] = self.fresh_line(dev),
            Edit::RemoveFirst if !lines.is_empty() => {
                lines.remove(0);
            }
            Edit::RemoveLast if !lines.is_empty() => {
                lines.pop();
            }
            _ => lines.push(self.fresh_line(dev)),
        }
        let mut text = header.to_string();
        for l in &lines {
            text.push_str(l);
            text.push('\n');
        }
        self.user_write(
            dev,
            path,
            text.clone().into_bytes(),
            &format!("edits ({how:?})"),
        );
        text
    }

    /// Edits the line `old` of the body into a fresh line (a conflicting edit when two devices do it).
    pub fn replace_line(&mut self, dev: usize, path: &str, old: &str) -> String {
        self.time += 1000;
        let text = self.devices[dev].text(path).expect("replace: no such note");
        let fresh = self.fresh_line(dev);
        assert!(text.contains(old), "replace: {old:?} not in {path}");
        let new = text.replacen(old, &fresh, 1);
        self.user_write(dev, path, new.into_bytes(), "replaces a line in");
        fresh
    }

    /// Deletes a file or a folder tree.
    pub fn delete(&mut self, dev: usize, path: &str) {
        self.time += 1000;
        let removed = self.devices[dev].remove_tree(path);
        for (p, n) in &removed {
            if let Node::File { bytes, .. } = n {
                if oracle::is_note_path(p) {
                    self.oracle.user_deleted(bytes);
                } else if oracle::attachment_hash(p).is_some() {
                    self.oracle.attachment_deleted(p);
                }
            }
        }
        self.log(format!("{} deletes {path}", self.devices[dev].name));
        self.after(&format!("user deletes {path}"));
    }

    pub fn mkdir(&mut self, dev: usize, path: &str) {
        self.time += 1000;
        self.devices[dev].mkdir_p(path);
        self.log(format!("{} creates folder {path}", self.devices[dev].name));
        self.after(&format!("user mkdir {path}"));
    }

    /// Renames a file or folder tree, keeping contents and modification times.
    pub fn rename(&mut self, dev: usize, from: &str, to: &str) {
        self.time += 1000;
        let removed = self.devices[dev].remove_tree(from);
        self.devices[dev].mkdir_p(paths::parent(to));
        for (p, n) in removed {
            let target = format!("{to}{}", &p[from.len()..]);
            self.devices[dev].durable.files.insert(target, n);
        }
        self.log(format!("{} renames {from} to {to}", self.devices[dev].name));
        self.after(&format!("user renames {from}"));
    }

    /// Adds a new attachment (unique bytes) and returns its path.
    pub fn add_attachment(&mut self, dev: usize) -> String {
        self.time += 1000;
        self.counter += 1;
        let bytes =
            format!("\u{89}PNG image {} {}", self.counter, self.rng.next_u64()).into_bytes();
        let path = format!("attachments/{}.png", sha256_hex(&bytes));
        self.oracle.attachment_added(&path, &bytes);
        let mtime = self.now(dev);
        self.devices[dev].write(&path, bytes, mtime);
        self.log(format!("{} adds attachment {path}", self.devices[dev].name));
        self.after("user adds attachment");
        path
    }

    /// Adds a file sync ignores (it keeps its folder from being deleted).
    pub fn add_ignored(&mut self, dev: usize, path: &str) {
        self.time += 1000;
        let mtime = self.now(dev);
        self.devices[dev].write(path, b"not synced".to_vec(), mtime);
        self.log(format!("{} adds ignored {path}", self.devices[dev].name));
        self.after("user adds ignored file");
    }

    // ----- inspection -----

    /// The plan `dev` would compute for `drive` right now, with a complete listing.
    pub fn plan_for(&self, dev: usize, drive: usize) -> SyncPlan {
        let d = &self.devices[dev];
        let listing = self.drives[drive].listing(true);
        marknotes_sync::plan(&SyncInput {
            local: d.list(),
            remote: listing.entries,
            records: d.records(&self.drives[drive].name),
            manifest: listing.manifest,
        })
    }

    /// The synced view of a device: path → `None` for a folder, the bytes for a file.
    pub fn device_view(&self, dev: usize) -> BTreeMap<String, Option<Vec<u8>>> {
        self.devices[dev]
            .durable
            .files
            .iter()
            .filter(|(p, n)| paths::is_synced_entry(p, matches!(n, Node::Folder)))
            .map(|(p, n)| {
                (
                    p.clone(),
                    match n {
                        Node::Folder => None,
                        Node::File { bytes, .. } => Some(bytes.clone()),
                    },
                )
            })
            .collect()
    }

    pub fn drive_view(&self, drive: usize) -> BTreeMap<String, Option<Vec<u8>>> {
        self.drives[drive]
            .entries()
            .filter(|e| paths::is_synced_entry(&e.path, e.is_folder))
            .map(|e| {
                (
                    e.path.clone(),
                    if e.is_folder {
                        None
                    } else {
                        Some(e.bytes.clone())
                    },
                )
            })
            .collect()
    }

    /// Bytes, modification times and ids of everything: actions that only record must not change it.
    pub fn fingerprint(&self) -> String {
        let mut s = String::new();
        for d in &self.devices {
            for (p, n) in &d.durable.files {
                match n {
                    Node::Folder => s.push_str(&format!("{}:{p}/\n", d.name)),
                    Node::File { bytes, mtime } => {
                        s.push_str(&format!("{}:{p}={}@{mtime}\n", d.name, sha256_hex(bytes)))
                    }
                }
            }
        }
        for r in &self.drives {
            for e in r.entries() {
                s.push_str(&format!(
                    "{}:{}={}:{}:{}@{}\n",
                    r.name,
                    e.path,
                    e.id,
                    e.version,
                    sha256_hex(&e.bytes),
                    e.modified_millis
                ));
            }
        }
        s
    }

    /// Plans without errors: errors (case clashes) stay until the user acts.
    pub fn pending_actions(&self, dev: usize, drive: usize) -> Vec<Action> {
        self.plan_for(dev, drive)
            .actions
            .into_iter()
            .filter(|a| !matches!(a, Action::Error { .. }))
            .collect()
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        for (i, d) in self.devices.iter().enumerate() {
            s.push_str(&format!("{}:\n", d.name));
            for (p, v) in self.device_view(i) {
                s.push_str(&format!(
                    "  {p} {:?}\n",
                    v.map(|b| String::from_utf8_lossy(&b).into_owned())
                ));
            }
        }
        for (i, r) in self.drives.iter().enumerate() {
            s.push_str(&format!("{}:\n", r.name));
            for (p, v) in self.drive_view(i) {
                s.push_str(&format!(
                    "  {p} {:?}\n",
                    v.map(|b| String::from_utf8_lossy(&b).into_owned())
                ));
            }
        }
        s
    }

    /// Every device and every drive holds the same paths, types and bytes, and no plan has anything left to do.
    pub fn assert_converged(&self) {
        let reference = self.device_view(0);
        for i in 1..self.devices.len() {
            assert_eq!(
                self.device_view(i),
                reference,
                "{} differs from dev0\n{}\nlog:\n  {}",
                self.devices[i].name,
                self.describe(),
                self.log.join("\n  ")
            );
        }
        for r in 0..self.drives.len() {
            assert_eq!(
                self.drive_view(r),
                reference,
                "{} differs from dev0\n{}\nlog:\n  {}",
                self.drives[r].name,
                self.describe(),
                self.log.join("\n  ")
            );
        }
        for (i, d) in self.devices.iter().enumerate() {
            for &r in &d.drives {
                let left = self.pending_actions(i, r);
                assert!(
                    left.is_empty(),
                    "{} still has actions for {}: {left:?}",
                    d.name,
                    self.drives[r].name
                );
            }
        }
    }
}
