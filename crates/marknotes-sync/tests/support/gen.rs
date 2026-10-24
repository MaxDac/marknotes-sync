//! Reusable proptest generators for marknotes-sync, and the deterministic proptest configuration.
//!
//! Every generator is a `pub fn` returning a strategy, so that property tests, the simulation harness and the
//! parity corpus generator draw from the same domains:
//!
//! - path segments and paths with Unicode case variants (`ß`/`ẞ`, `ſ`, Kelvin sign, `İ`/`ı`, final sigma, micro
//!   sign, combining marks, emoji, CJK, characters above U+FFFF and in U+E000..U+FFFF), and invalid segments;
//! - prefix-closed trees of folders, notes, attachments and ignored entries ([`tree`]);
//! - local, remote and record listings that are consistent ([`sync_input`], [`synced_input`]) or deliberately
//!   inconsistent ([`inconsistent_sync_input`], [`arbitrary_sync_input`]);
//! - texts with LF, CRLF, no trailing newline, a byte order mark, conflict-marker-looking and repeated lines, and
//!   edited families of them for merges ([`text_triple`]);
//! - front-matter blocks, valid and invalid note ids, manifests and manifest-like JSON.

// Each test crate that includes this module uses a subset of it.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use marknotes_sync::hashing::sha256_hex;
use marknotes_sync::manifest::{serialize_manifest, Manifest, ManifestEntry};
use marknotes_sync::model::{LocalEntry, RemoteEntry, SyncInput, SyncRecord};
use marknotes_sync::paths;
use proptest::collection::{hash_map, vec};
use proptest::prelude::*;
use proptest::sample::{select, Index};
use proptest::test_runner::{Config, RngSeed};
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------------------------------------------

/// Seed used when `MARKNOTES_PROPTEST_SEED` is not set, so that normal runs are reproducible.
pub const DEFAULT_SEED: u64 = 0x6d61_726b_6e6f_7465;
/// Cases per property when `PROPTEST_CASES` is not set.
pub const DEFAULT_CASES: u32 = 256;

/// The RNG seed: `MARKNOTES_PROPTEST_SEED` (decimal, or hex with `0x`) or [`DEFAULT_SEED`].
pub fn seed() -> u64 {
    std::env::var("MARKNOTES_PROPTEST_SEED")
        .ok()
        .and_then(|s| {
            let s = s.trim();
            match s.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16).ok(),
                None => s.parse().ok(),
            }
        })
        .unwrap_or(DEFAULT_SEED)
}

/// Cases per property: `PROPTEST_CASES` or [`DEFAULT_CASES`].
pub fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_CASES)
}

/// [`cases()`] cases with a fixed seed ([`seed()`]) and no failure-persistence files.
pub fn config() -> Config {
    config_with_cases(cases())
}

/// One `divisor`-th of [`cases()`] (at least one), for expensive properties.
pub fn config_fraction(divisor: u32) -> Config {
    config_with_cases((cases() / divisor.max(1)).max(1))
}

pub fn config_with_cases(cases: u32) -> Config {
    let seed = seed();
    // Captured by the test harness and shown only when the test fails.
    eprintln!(
        "proptest: seed {seed}, {cases} cases (reproduce with MARKNOTES_PROPTEST_SEED={seed})"
    );
    Config {
        cases,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------------------------

/// Characters for valid segments. Small on purpose so that names collide, exactly or by case folding.
pub const SEGMENT_CHARS: &[char] = &[
    'a', 'A', 'b', 'B', 's', 'S', 'ſ', 'ß', 'ẞ', 'k', 'K', '\u{212A}', 'i', 'I', 'İ', 'ı', 'σ',
    'Σ', 'ς', 'µ', 'μ', 'Μ', 'e', 'E', 'é', 'É', '\u{301}', '😀', '中', '文', '\u{FF5E}',
    '\u{E000}', '1', '-', ' ',
];

/// Characters that are the same as `c` under simple case folding (not including `c`).
pub fn case_partners(c: char) -> &'static [char] {
    match c {
        'a' => &['A'],
        'A' => &['a'],
        'b' => &['B'],
        'B' => &['b'],
        's' => &['S', 'ſ'],
        'S' => &['s', 'ſ'],
        'ſ' => &['s', 'S'],
        'ß' => &['ẞ'],
        'ẞ' => &['ß'],
        'k' => &['K', '\u{212A}'],
        'K' => &['k', '\u{212A}'],
        '\u{212A}' => &['k', 'K'],
        'i' => &['I'],
        'I' => &['i'],
        'σ' => &['Σ', 'ς'],
        'Σ' => &['σ', 'ς'],
        'ς' => &['σ', 'Σ'],
        'µ' => &['μ', 'Μ'],
        'μ' => &['µ', 'Μ'],
        'Μ' => &['µ', 'μ'],
        'e' => &['E'],
        'E' => &['e'],
        'é' => &['É'],
        'É' => &['é'],
        'c' => &['C'],
        'C' => &['c'],
        'd' => &['D'],
        'D' => &['d'],
        'f' => &['F'],
        'F' => &['f'],
        'm' => &['M'],
        'M' => &['m'],
        _ => &[],
    }
}

/// `s` with each character replaced by one of its case partners (or kept), using `picks` cyclically.
pub fn vary_case(s: &str, picks: &[Index]) -> String {
    s.chars()
        .enumerate()
        .map(|(i, c)| {
            let partners = case_partners(c);
            if picks.is_empty() || partners.is_empty() {
                return c;
            }
            let k = picks[i % picks.len()].index(partners.len() + 1);
            if k == 0 {
                c
            } else {
                partners[k - 1]
            }
        })
        .collect()
}

/// A spelling of `s` equal to it under `paths::fold_case` (possibly `s` itself).
pub fn case_variant(s: String) -> BoxedStrategy<String> {
    vec(any::<Index>(), 1..8)
        .prop_map(move |picks| vary_case(&s, &picks))
        .boxed()
}

pub fn segment_char() -> BoxedStrategy<char> {
    select(SEGMENT_CHARS).boxed()
}

/// A valid file or folder name: no `/`, `\`, control character, leading `.` or surrounding whitespace.
pub fn valid_segment() -> BoxedStrategy<String> {
    vec(segment_char(), 1..=4)
        .prop_map(|cs| {
            let s: String = cs.into_iter().collect();
            let t = s.trim();
            if t.is_empty() {
                "n".to_string()
            } else {
                t.to_string()
            }
        })
        .boxed()
}

/// U+0000..U+001F and U+007F.
pub fn control_char() -> BoxedStrategy<char> {
    prop_oneof![
        (0u32..0x20).prop_map(|c| char::from_u32(c).expect("ASCII")),
        Just('\u{7f}'),
    ]
    .boxed()
}

/// A segment that makes a path unsyncable: empty, `.`, `..`, hidden, surrounding whitespace, a control
/// character, or a backslash (which `normalize` turns into a separator).
pub fn invalid_segment() -> BoxedStrategy<String> {
    prop_oneof![
        Just(String::new()),
        Just(".".to_string()),
        Just("..".to_string()),
        Just(".hidden".to_string()),
        Just(".marknotes".to_string()),
        valid_segment().prop_map(|s| format!(" {s}")),
        valid_segment().prop_map(|s| format!("{s} ")),
        valid_segment().prop_map(|s| format!("{s}\t")),
        valid_segment().prop_map(|s| format!("\u{3000}{s}")),
        (valid_segment(), control_char()).prop_map(|(s, c)| format!("{s}{c}")),
        (control_char(), valid_segment()).prop_map(|(c, s)| format!("{c}{s}")),
        (valid_segment(), valid_segment()).prop_map(|(a, b)| format!("{a}\\{b}")),
    ]
    .boxed()
}

pub fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// A syncable path of 1 to `max_depth` valid segments.
pub fn valid_path(max_depth: usize) -> BoxedStrategy<String> {
    vec(valid_segment(), 1..=max_depth.max(1))
        .prop_map(|v| v.join("/"))
        .boxed()
}

pub fn note_extension() -> BoxedStrategy<&'static str> {
    prop_oneof![6 => Just(".md"), 1 => Just(".MD"), 1 => Just(".Md")].boxed()
}

/// A note path (`.md` in any case) with up to `max_depth - 1` folders, outside `attachments/`.
pub fn note_path(max_depth: usize) -> BoxedStrategy<String> {
    (
        vec(valid_segment(), 0..max_depth.max(1)),
        valid_segment(),
        note_extension(),
    )
        .prop_map(|(mut folders, name, ext)| {
            if folders.first().map(String::as_str) == Some(paths::ATTACHMENTS_FOLDER) {
                folders.remove(0);
            }
            folders.push(format!("{name}{ext}"));
            folders.join("/")
        })
        .boxed()
}

pub const ATTACHMENT_EXTENSIONS: &[&str] = &["png", "jpg", "gif", "webp"];

/// The hash in the name of the `i`-th test attachment.
pub fn attachment_hash(i: u8) -> String {
    sha256_hex(format!("image {i}").as_bytes())
}

/// `attachments/<sha256>.<ext>` from a small set of hashes.
pub fn attachment_path() -> BoxedStrategy<String> {
    (0u8..3, select(ATTACHMENT_EXTENSIONS))
        .prop_map(|(i, ext)| format!("attachments/{}.{ext}", attachment_hash(i)))
        .boxed()
}

/// Short strings of any characters, including controls, surrogate-pair characters and separators.
pub fn any_short_string(max_len: usize) -> BoxedStrategy<String> {
    vec(
        prop_oneof![
            4 => any::<char>(),
            2 => segment_char(),
            1 => select(vec!['/', '\\', '.', ' ', '"', '\n', '\r', '\u{feff}', '\u{1F600}', '\u{FF5E}']),
            1 => control_char(),
        ],
        0..=max_len,
    )
    .prop_map(String::from_iter).boxed()
}

/// Any path-like string: valid paths, notes, attachments, paths with invalid segments, stray separators and
/// arbitrary strings.
pub fn any_path() -> BoxedStrategy<String> {
    prop_oneof![
        3 => valid_path(4),
        3 => note_path(4),
        1 => attachment_path(),
        2 => vec(prop_oneof![2 => valid_segment(), 1 => invalid_segment()], 0..4).prop_map(|v| v.join("/")),
        1 => valid_path(3).prop_map(|p| format!("/{p}")),
        1 => valid_path(3).prop_map(|p| format!("{p}/")),
        1 => valid_path(3).prop_map(|p| p.replace('/', "//")),
        2 => any_short_string(12),
    ].boxed()
}

/// The name `paths::conflict_copy_name` tries for the `n`-th candidate (from 1) of `path`, per the spec:
/// `<stem> (conflicted copy).<ext>`, then `<stem> (conflicted copy) <n>.<ext>`.
pub fn conflict_copy_candidate(path: &str, n: u32) -> String {
    let dir = paths::parent(path);
    let file = paths::name(path);
    let (stem, ext) = match file.rfind('.') {
        Some(i) if i > 0 => (&file[..i], &file[i..]),
        _ => (file, ""),
    };
    let name = if n == 1 {
        format!("{stem} (conflicted copy){ext}")
    } else {
        format!("{stem} (conflicted copy) {n}{ext}")
    };
    join(dir, &name)
}

/// Paths that crowd the conflicted-copy names of `path`: case variants of the first candidates, of the path
/// itself and of its folder, and unrelated paths.
pub fn existing_paths_near(path: String) -> BoxedStrategy<Vec<String>> {
    let p = path.clone();
    let near = prop_oneof![
        3 => (1u32..5, vec(any::<Index>(), 1..8))
            .prop_map(move |(n, picks)| vary_case(&conflict_copy_candidate(&p, n), &picks)),
        1 => Just(path.clone()),
        1 => (valid_segment(), vec(any::<Index>(), 1..8)).prop_map({
            let dir = paths::parent(&path).to_string();
            move |(name, picks)| vary_case(&join(&dir, &name), &picks)
        }),
        1 => any_path(),
    ];
    vec(near, 0..8).boxed()
}

// ---------------------------------------------------------------------------------------------------------------
// Trees
// ---------------------------------------------------------------------------------------------------------------

/// An entry of a generated tree. Whether it is synced follows from its path (see [`is_synced`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub path: String,
    pub is_folder: bool,
}

/// Whether the planner syncs an entry (the spec's "What is synced").
pub fn is_synced(path: &str, is_folder: bool) -> bool {
    paths::is_syncable_path(path) && paths::is_synced_entry(path, is_folder)
}

impl Node {
    pub fn is_synced(&self) -> bool {
        is_synced(&self.path, self.is_folder)
    }
}

#[derive(Debug, Clone)]
enum Step {
    Folder(String),
    Note(String, &'static str),
    Attachment(u8, &'static str),
    IgnoredFile(&'static str),
    IgnoredFolder(&'static str),
    /// A note directly in `attachments/`, which is ignored.
    NoteInAttachments(String),
    /// A case variant of an existing entry, next to it.
    Twin,
}

const IGNORED_FILES: &[&str] = &[
    "doc.pdf",
    "notes.txt",
    "picture.png",
    "README",
    ".DS_Store",
    "bad\u{1}.md",
    " lead.md",
    "trail.md ",
];
const IGNORED_FOLDERS: &[&str] = &[".obsidian", ".marknotes", ".git", " spaced"];

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => valid_segment().prop_map(Step::Folder),
        4 => (valid_segment(), note_extension()).prop_map(|(n, e)| Step::Note(n, e)),
        1 => (0u8..3, select(ATTACHMENT_EXTENSIONS)).prop_map(|(i, e)| Step::Attachment(i, e)),
        1 => select(IGNORED_FILES).prop_map(Step::IgnoredFile),
        1 => select(IGNORED_FOLDERS).prop_map(Step::IgnoredFolder),
        1 => valid_segment().prop_map(Step::NoteInAttachments),
        2 => Just(Step::Twin),
    ]
}

fn build_tree(steps: Vec<(Index, Step, Index, Vec<Index>)>) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut folders: Vec<String> = vec![String::new()];
    let mut add =
        |nodes: &mut Vec<Node>, folders: &mut Vec<String>, path: String, is_folder: bool| {
            if seen.insert(path.clone()) {
                if is_folder {
                    folders.push(path.clone());
                }
                nodes.push(Node { path, is_folder });
            }
        };
    for (parent_ix, step, twin_ix, picks) in steps {
        let parent = parent_ix.get(&folders).clone();
        match step {
            Step::Folder(name) => add(&mut nodes, &mut folders, join(&parent, &name), true),
            Step::Note(name, ext) => add(
                &mut nodes,
                &mut folders,
                join(&parent, &format!("{name}{ext}")),
                false,
            ),
            Step::Attachment(i, ext) => {
                add(
                    &mut nodes,
                    &mut folders,
                    paths::ATTACHMENTS_FOLDER.to_string(),
                    true,
                );
                let path = format!("attachments/{}.{ext}", attachment_hash(i));
                add(&mut nodes, &mut folders, path, false);
            }
            Step::IgnoredFile(name) => add(&mut nodes, &mut folders, join(&parent, name), false),
            Step::IgnoredFolder(name) => add(&mut nodes, &mut folders, join(&parent, name), true),
            Step::NoteInAttachments(name) => {
                add(
                    &mut nodes,
                    &mut folders,
                    paths::ATTACHMENTS_FOLDER.to_string(),
                    true,
                );
                add(
                    &mut nodes,
                    &mut folders,
                    format!("attachments/{name}.md"),
                    false,
                );
            }
            Step::Twin => {
                if nodes.is_empty() {
                    continue;
                }
                let n = twin_ix.get(&nodes).clone();
                let twin = join(
                    paths::parent(&n.path),
                    &vary_case(paths::name(&n.path), &picks),
                );
                add(&mut nodes, &mut folders, twin, n.is_folder);
            }
        }
    }
    nodes
}

/// A tree of up to `max_steps` entries: unique paths, prefix-closed (every parent is a folder of the tree and
/// comes before its children), with case twins, attachments and ignored entries.
pub fn tree(max_steps: usize) -> BoxedStrategy<Vec<Node>> {
    vec(
        (
            any::<Index>(),
            step(),
            any::<Index>(),
            vec(any::<Index>(), 1..6),
        ),
        0..=max_steps,
    )
    .prop_map(build_tree)
    .boxed()
}

/// Every prefix of `path` (`a/b/c` gives `a`, `a/b`, `a/b/c`).
pub fn prefixes(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .map(move |(i, _)| &path[..i])
        .chain(std::iter::once(path))
}

/// `nodes` without the entries whose path, or a folder above it, differs only in case from an earlier one.
pub fn without_case_collisions(nodes: Vec<Node>) -> Vec<Node> {
    let mut spelling: HashMap<String, String> = HashMap::new();
    let mut kept = Vec::new();
    'nodes: for n in nodes {
        for p in prefixes(&n.path) {
            if let Some(s) = spelling.get(&paths::fold_case(p)) {
                if s != p {
                    continue 'nodes;
                }
            }
        }
        for p in prefixes(&n.path) {
            spelling
                .entry(paths::fold_case(p))
                .or_insert_with(|| p.to_string());
        }
        kept.push(n);
    }
    kept
}

// ---------------------------------------------------------------------------------------------------------------
// Listings
// ---------------------------------------------------------------------------------------------------------------

/// SHA-256 of the `i`-th test content.
pub fn content_hash(i: u8) -> String {
    sha256_hex(format!("content {i}").as_bytes())
}

pub fn version(i: u8) -> String {
    format!("v{i}")
}

/// Drive id of node `node` in its `variant` (0 is the usual one; another variant is a re-created file).
pub fn remote_id(node: usize, is_folder: bool, variant: u8) -> String {
    if is_folder {
        format!("d{node}-{variant}")
    } else {
        format!("f{node}-{variant}")
    }
}

/// Where a node is and what each side holds. Small value ranges so that sides often agree with the records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeState {
    pub local: bool,
    pub local_content: u8,
    pub local_millis: i64,
    pub remote: bool,
    pub remote_version: u8,
    pub remote_id: u8,
    pub remote_millis: i64,
    pub record: bool,
    pub record_hash: u8,
    pub record_version: u8,
    pub record_id: u8,
}

fn small_variant() -> impl Strategy<Value = u8> {
    prop_oneof![4 => Just(0u8), 1 => Just(1u8)]
}

pub fn node_state() -> BoxedStrategy<NodeState> {
    (
        (prop::bool::weighted(0.75), 0u8..3, 0i64..4),
        (prop::bool::weighted(0.75), 0u8..3, small_variant(), 0i64..4),
        (prop::bool::weighted(0.6), 0u8..3, 0u8..3, small_variant()),
    )
        .prop_map(
            |(
                (local, local_content, local_millis),
                (remote, remote_version, remote_id, remote_millis),
                (record, record_hash, record_version, record_id),
            )| NodeState {
                local,
                local_content,
                local_millis,
                remote,
                remote_version,
                remote_id,
                remote_millis,
                record,
                record_hash,
                record_version,
                record_id,
            },
        )
        .boxed()
}

/// What the drive's manifest holds.
#[derive(Debug, Clone)]
pub enum ManifestChoice {
    None,
    Garbage(String),
    /// Entries for some remote files: (which file, content, version).
    Entries(Vec<(Index, u8, u8)>),
}

pub fn manifest_choice() -> BoxedStrategy<ManifestChoice> {
    prop_oneof![
        2 => Just(ManifestChoice::None),
        1 => json_text().prop_map(ManifestChoice::Garbage),
        3 => vec((any::<Index>(), 0u8..3, 0u8..3), 0..6).prop_map(ManifestChoice::Entries),
    ]
    .boxed()
}

fn local_hash_of(node: &Node, content: u8) -> String {
    // An attachment normally holds the content its name is the hash of.
    match node.path.strip_prefix("attachments/") {
        Some(file) if paths::is_attachment_path(&node.path) && content == 0 => {
            file.split('.').next().unwrap_or_default().to_string()
        }
        _ => content_hash(content),
    }
}

/// Builds the planner input for `nodes` in `states`. Listings are prefix-closed (a listed entry's folders are
/// listed on the same side); records exist only for synced entries.
pub fn build_input(nodes: &[Node], states: &[NodeState], manifest: &ManifestChoice) -> SyncInput {
    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.path.as_str(), i))
        .collect();
    let mut local: Vec<LocalEntry> = Vec::new();
    let mut remote: Vec<RemoteEntry> = Vec::new();
    let mut records: Vec<SyncRecord> = Vec::new();
    let mut local_listed: HashSet<String> = HashSet::new();
    let mut remote_listed: HashSet<String> = HashSet::new();
    for (i, (n, s)) in nodes.iter().zip(states).enumerate() {
        if s.local {
            local_listed.insert(n.path.clone());
            local.push(LocalEntry {
                path: n.path.clone(),
                is_folder: n.is_folder,
                modified_millis: s.local_millis,
                sha256: (!n.is_folder).then(|| local_hash_of(n, s.local_content)),
            });
        }
        if s.remote {
            remote_listed.insert(n.path.clone());
            remote.push(RemoteEntry {
                path: n.path.clone(),
                id: remote_id(i, n.is_folder, s.remote_id),
                is_folder: n.is_folder,
                version: if n.is_folder {
                    String::new()
                } else {
                    version(s.remote_version)
                },
                modified_millis: s.remote_millis,
            });
        }
        if s.record && n.is_synced() {
            records.push(SyncRecord {
                path: n.path.clone(),
                is_folder: n.is_folder,
                remote_id: remote_id(i, n.is_folder, s.record_id),
                remote_version: if n.is_folder {
                    String::new()
                } else {
                    version(s.record_version)
                },
                local_hash: if n.is_folder {
                    String::new()
                } else {
                    local_hash_of(n, s.record_hash)
                },
                synced_at: 1,
            });
        }
    }
    // Prefix closure: every folder above a listed entry is listed on the same side.
    for n in nodes {
        let (in_local, in_remote) = (
            local_listed.contains(&n.path),
            remote_listed.contains(&n.path),
        );
        for a in prefixes(&n.path).filter(|p| p.len() < n.path.len()) {
            let i = index[a];
            if in_local && local_listed.insert(a.to_string()) {
                local.push(LocalEntry {
                    path: a.to_string(),
                    is_folder: true,
                    modified_millis: 0,
                    sha256: None,
                });
            }
            if in_remote && remote_listed.insert(a.to_string()) {
                remote.push(RemoteEntry {
                    path: a.to_string(),
                    id: remote_id(i, true, 0),
                    is_folder: true,
                    version: String::new(),
                    modified_millis: 0,
                });
            }
        }
    }
    let manifest = match manifest {
        ManifestChoice::None => None,
        ManifestChoice::Garbage(text) => Some(text.clone()),
        ManifestChoice::Entries(entries) => {
            let files: Vec<&RemoteEntry> = remote.iter().filter(|e| !e.is_folder).collect();
            let mut map = HashMap::new();
            if !files.is_empty() {
                for (ix, content, v) in entries {
                    let e = ix.get(&files);
                    map.insert(
                        e.path.clone(),
                        ManifestEntry {
                            sha256: content_hash(*content),
                            version: version(*v),
                            synced_at: 1,
                        },
                    );
                }
            }
            Some(serialize_manifest(&Manifest {
                device_id: "other-device".into(),
                updated_at: 1,
                files: map,
            }))
        }
    };
    SyncInput {
        local,
        remote,
        records,
        manifest,
    }
}

/// A tree and its planner input.
pub fn scenario(max_steps: usize) -> BoxedStrategy<(Vec<Node>, SyncInput)> {
    tree(max_steps)
        .prop_flat_map(|nodes| {
            let n = nodes.len();
            (Just(nodes), vec(node_state(), n), manifest_choice()).prop_map(
                |(nodes, states, manifest)| {
                    let input = build_input(&nodes, &states, &manifest);
                    (nodes, input)
                },
            )
        })
        .boxed()
}

/// A consistent planner input: unique paths per side, prefix-closed listings, each path the same kind on every
/// side and in the records. Paths may differ only in case (the planner reports those).
pub fn sync_input(max_steps: usize) -> BoxedStrategy<SyncInput> {
    scenario(max_steps).prop_map(|(_, input)| input).boxed()
}

/// Both sides identical, every synced entry recorded as it is, no case collisions: a sync with nothing to do.
pub fn synced_input(max_steps: usize) -> BoxedStrategy<SyncInput> {
    (
        tree(max_steps),
        vec((0u8..3, 0u8..3, small_variant()), 0..=max_steps * 2),
        manifest_choice(),
    )
        .prop_map(|(nodes, picks, manifest)| {
            let nodes = without_case_collisions(nodes);
            let states: Vec<NodeState> = (0..nodes.len())
                .map(|i| {
                    let (content, v, id) = picks.get(i).copied().unwrap_or((0, 0, 0));
                    NodeState {
                        local: true,
                        local_content: content,
                        local_millis: 1,
                        remote: true,
                        remote_version: v,
                        remote_id: id,
                        remote_millis: 2,
                        record: true,
                        record_hash: content,
                        record_version: v,
                        record_id: id,
                    }
                })
                .collect();
            build_input(&nodes, &states, &manifest)
        })
        .boxed()
}

/// A change that makes an input inconsistent.
#[derive(Debug, Clone)]
pub enum Perturbation {
    /// List a local entry twice (the copy possibly of the other kind or content).
    DuplicateLocal(Index, bool, u8),
    DuplicateRemote(Index, bool, u8),
    DuplicateRecord(Index, u8),
    /// Make an entry a file on one side and a folder on the other.
    FlipLocalKind(Index),
    FlipRemoteKind(Index),
    FlipRecordKind(Index),
    /// Leave a folder out of a listing (it is then implied by its contents, if any).
    DropLocalFolder(Index),
    DropRemoteFolder(Index),
    /// A record for a path neither side has.
    StrayRecord(String, bool),
}

pub fn perturbation() -> BoxedStrategy<Perturbation> {
    prop_oneof![
        (any::<Index>(), any::<bool>(), 0u8..3)
            .prop_map(|(i, f, c)| Perturbation::DuplicateLocal(i, f, c)),
        (any::<Index>(), any::<bool>(), 0u8..3)
            .prop_map(|(i, f, c)| Perturbation::DuplicateRemote(i, f, c)),
        (any::<Index>(), 0u8..3).prop_map(|(i, c)| Perturbation::DuplicateRecord(i, c)),
        any::<Index>().prop_map(Perturbation::FlipLocalKind),
        any::<Index>().prop_map(Perturbation::FlipRemoteKind),
        any::<Index>().prop_map(Perturbation::FlipRecordKind),
        any::<Index>().prop_map(Perturbation::DropLocalFolder),
        any::<Index>().prop_map(Perturbation::DropRemoteFolder),
        (any_path(), any::<bool>()).prop_map(|(p, f)| Perturbation::StrayRecord(p, f)),
    ]
    .boxed()
}

pub fn perturb(input: &mut SyncInput, p: &Perturbation) {
    match p {
        Perturbation::DuplicateLocal(i, flip, c) if !input.local.is_empty() => {
            let mut e = i.get(&input.local).clone();
            if *flip {
                e.is_folder = !e.is_folder;
            }
            e.sha256 = (!e.is_folder).then(|| content_hash(*c));
            input.local.push(e);
        }
        Perturbation::DuplicateRemote(i, flip, c) if !input.remote.is_empty() => {
            let mut e = i.get(&input.remote).clone();
            if *flip {
                e.is_folder = !e.is_folder;
            }
            e.version = if e.is_folder {
                String::new()
            } else {
                version(*c)
            };
            input.remote.push(e);
        }
        Perturbation::DuplicateRecord(i, c) if !input.records.is_empty() => {
            let mut r = i.get(&input.records).clone();
            r.local_hash = content_hash(*c);
            input.records.push(r);
        }
        Perturbation::FlipLocalKind(i) if !input.local.is_empty() => {
            let e = i.get_mut(&mut input.local);
            e.is_folder = !e.is_folder;
            e.sha256 = (!e.is_folder).then(|| content_hash(0));
        }
        Perturbation::FlipRemoteKind(i) if !input.remote.is_empty() => {
            let e = i.get_mut(&mut input.remote);
            e.is_folder = !e.is_folder;
            e.version = if e.is_folder {
                String::new()
            } else {
                version(0)
            };
        }
        Perturbation::FlipRecordKind(i) if !input.records.is_empty() => {
            let r = i.get_mut(&mut input.records);
            r.is_folder = !r.is_folder;
        }
        Perturbation::DropLocalFolder(i) => {
            let folders: Vec<usize> = (0..input.local.len())
                .filter(|&k| input.local[k].is_folder)
                .collect();
            if !folders.is_empty() {
                input.local.remove(*i.get(&folders));
            }
        }
        Perturbation::DropRemoteFolder(i) => {
            let folders: Vec<usize> = (0..input.remote.len())
                .filter(|&k| input.remote[k].is_folder)
                .collect();
            if !folders.is_empty() {
                input.remote.remove(*i.get(&folders));
            }
        }
        Perturbation::StrayRecord(path, is_folder) => input.records.push(SyncRecord {
            path: path.clone(),
            is_folder: *is_folder,
            remote_id: "stray".into(),
            remote_version: if *is_folder {
                String::new()
            } else {
                version(0)
            },
            local_hash: if *is_folder {
                String::new()
            } else {
                content_hash(0)
            },
            synced_at: 1,
        }),
        _ => {}
    }
}

/// A consistent input with 0 to 4 perturbations: duplicates, file/folder clashes, records of the wrong kind,
/// folders left out of a listing, stray records.
pub fn inconsistent_sync_input(max_steps: usize) -> BoxedStrategy<SyncInput> {
    (sync_input(max_steps), vec(perturbation(), 0..=4))
        .prop_map(|(mut input, ps)| {
            for p in &ps {
                perturb(&mut input, p);
            }
            input
        })
        .boxed()
}

pub fn arbitrary_local_entry() -> BoxedStrategy<LocalEntry> {
    (
        any_path(),
        any::<bool>(),
        any::<i64>(),
        prop::option::of(prop_oneof![
            (0u8..3).prop_map(content_hash),
            any_short_string(4)
        ]),
    )
        .prop_map(|(path, is_folder, modified_millis, sha256)| LocalEntry {
            path,
            is_folder,
            modified_millis,
            sha256,
        })
        .boxed()
}

pub fn arbitrary_remote_entry() -> BoxedStrategy<RemoteEntry> {
    (
        any_path(),
        any_short_string(3),
        any::<bool>(),
        any_short_string(3),
        any::<i64>(),
    )
        .prop_map(
            |(path, id, is_folder, version, modified_millis)| RemoteEntry {
                path,
                id,
                is_folder,
                version,
                modified_millis,
            },
        )
        .boxed()
}

pub fn arbitrary_record() -> BoxedStrategy<SyncRecord> {
    (
        any_path(),
        any::<bool>(),
        any_short_string(3),
        any_short_string(3),
        prop_oneof![(0u8..3).prop_map(content_hash), any_short_string(4)],
        any::<i64>(),
    )
        .prop_map(
            |(path, is_folder, remote_id, remote_version, local_hash, synced_at)| SyncRecord {
                path,
                is_folder,
                remote_id,
                remote_version,
                local_hash,
                synced_at,
            },
        )
        .boxed()
}

/// Any bounded input: arbitrary strings, duplicates, clashes, missing folders, any manifest text.
pub fn arbitrary_sync_input() -> BoxedStrategy<SyncInput> {
    prop_oneof![
        (
            vec(arbitrary_local_entry(), 0..10),
            vec(arbitrary_remote_entry(), 0..10),
            vec(arbitrary_record(), 0..10),
            prop::option::of(json_text()),
        )
            .prop_map(|(local, remote, records, manifest)| SyncInput {
                local,
                remote,
                records,
                manifest,
            }),
        inconsistent_sync_input(12),
    ]
    .boxed()
}

// ---------------------------------------------------------------------------------------------------------------
// Texts
// ---------------------------------------------------------------------------------------------------------------

/// Line contents, with conflict-marker-looking lines, fences and id lines. Few, so lines repeat.
pub const LINE_POOL: &[&str] = &[
    "a",
    "b",
    "c",
    "same",
    "",
    "<<<<<<< this device",
    "=======",
    ">>>>>>> drive",
    "---",
    "id: k7f3q2xa",
    "é 😀 中文",
    "  indented",
    "\t",
    "trailing \r",
];

pub fn line_ending() -> BoxedStrategy<&'static str> {
    prop_oneof![4 => Just("\n"), 1 => Just("\r\n")].boxed()
}

/// One line with its ending.
pub fn line() -> BoxedStrategy<String> {
    (select(LINE_POOL), line_ending())
        .prop_map(|(l, e)| format!("{l}{e}"))
        .boxed()
}

pub fn lines(max: usize) -> BoxedStrategy<Vec<String>> {
    vec(line(), 0..=max).boxed()
}

/// Removes the final line ending, if any.
pub fn strip_final_newline(mut text: String) -> String {
    if text.ends_with("\r\n") {
        text.truncate(text.len() - 2);
    } else if text.ends_with('\n') {
        text.pop();
    }
    text
}

/// A text without byte order mark: lines with LF or CRLF, sometimes without a final line break.
pub fn body_text() -> BoxedStrategy<String> {
    (lines(10), prop::bool::weighted(0.8))
        .prop_map(|(ls, final_newline)| {
            let text = ls.concat();
            if final_newline {
                text
            } else {
                strip_final_newline(text)
            }
        })
        .boxed()
}

/// [`body_text`] with sometimes a leading UTF-8 byte order mark.
pub fn text() -> BoxedStrategy<String> {
    (prop::bool::weighted(0.1), body_text())
        .prop_map(|(bom, t)| if bom { format!("\u{feff}{t}") } else { t })
        .boxed()
}

#[derive(Debug, Clone)]
enum Edit {
    Keep,
    Delete,
    Replace(String),
    InsertBefore(String),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        8 => Just(Edit::Keep),
        1 => Just(Edit::Delete),
        1 => line().prop_map(Edit::Replace),
        1 => line().prop_map(Edit::InsertBefore),
    ]
}

/// `base` with random line edits.
pub fn edited(base: Vec<String>) -> BoxedStrategy<Vec<String>> {
    let n = base.len();
    (vec(edit(), n), vec(line(), 0..2))
        .prop_map(move |(edits, tail)| {
            let mut out = Vec::new();
            for (l, e) in base.iter().zip(edits) {
                match e {
                    Edit::Keep => out.push(l.clone()),
                    Edit::Delete => {}
                    Edit::Replace(r) => out.push(r),
                    Edit::InsertBefore(r) => {
                        out.push(r);
                        out.push(l.clone());
                    }
                }
            }
            out.extend(tail);
            out
        })
        .boxed()
}

/// `(base, local, remote)`: two edits of a common base, each sometimes without a final line break, so merges are
/// clean, conflicting, or identical.
pub fn text_triple() -> BoxedStrategy<(String, String, String)> {
    lines(10)
        .prop_flat_map(|base| {
            (
                Just(base.clone()),
                edited(base.clone()),
                edited(base),
                vec(prop::bool::weighted(0.85), 3),
            )
        })
        .prop_map(|(b, l, r, nl)| {
            let finish = |ls: Vec<String>, keep: bool| {
                let t = ls.concat();
                if keep {
                    t
                } else {
                    strip_final_newline(t)
                }
            };
            (finish(b, nl[0]), finish(l, nl[1]), finish(r, nl[2]))
        })
        .boxed()
}

/// A large `(base, local, remote)` of `n` distinct lines, each replaced on each side with `replace` probability,
/// to cross the merge's maximum edit distance (2000 line insertions and deletions).
pub fn large_text_triple() -> BoxedStrategy<(String, String, String)> {
    (
        900usize..1150,
        select(vec![0.0, 0.85, 0.97, 1.0]),
        select(vec![0.0, 0.85, 0.97, 1.0]),
    )
        .prop_flat_map(|(n, pl, pr)| {
            (
                vec(prop::bool::weighted(pl), n),
                vec(prop::bool::weighted(pr), n),
            )
                .prop_map(move |(rl, rr)| {
                    let base: String = (0..n).map(|i| format!("b{i}\n")).collect();
                    let local: String = (0..n)
                        .map(|i| {
                            if rl[i] {
                                format!("l{i}\n")
                            } else {
                                format!("b{i}\n")
                            }
                        })
                        .collect();
                    let remote: String = (0..n)
                        .map(|i| {
                            if rr[i] {
                                format!("r{i}\n")
                            } else {
                                format!("b{i}\n")
                            }
                        })
                        .collect();
                    (base, local, remote)
                })
        })
        .boxed()
}

// ---------------------------------------------------------------------------------------------------------------
// Note ids and front matter
// ---------------------------------------------------------------------------------------------------------------

pub const ID_ALPHABET: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'j',
    'k', 'm', 'n', 'p', 'q', 'r', 's', 't', 'v', 'w', 'x', 'y', 'z',
];

/// 8 characters of the lowercase Crockford base32 alphabet.
pub fn valid_id() -> BoxedStrategy<String> {
    vec(select(ID_ALPHABET), 8)
        .prop_map(String::from_iter)
        .boxed()
}

/// Strings that are not valid ids: wrong length, a character outside the alphabet, uppercase, empty.
pub fn invalid_id() -> BoxedStrategy<String> {
    prop_oneof![
        vec(select(ID_ALPHABET), 0..8).prop_map(String::from_iter),
        vec(select(ID_ALPHABET), 9..12).prop_map(String::from_iter),
        (
            vec(select(ID_ALPHABET), 7),
            select(vec!['i', 'l', 'o', 'u', 'A', 'K', '-', '_', 'é', ' ']),
            any::<Index>()
        )
            .prop_map(|(mut cs, c, at)| {
                cs.insert(at.index(8), c);
                cs.into_iter().collect()
            }),
        valid_id().prop_map(|id| id.to_uppercase()),
    ]
    .prop_filter("still a valid id", |id| {
        !marknotes_sync::front_matter::is_valid_id(id)
    })
    .boxed()
}

/// A key line of a front-matter block (no line break).
pub fn front_matter_line() -> BoxedStrategy<String> {
    prop_oneof![
        3 => valid_id().prop_map(|id| format!("id: {id}")),
        1 => valid_id().prop_map(|id| format!("id: \"{id}\"")),
        1 => valid_id().prop_map(|id| format!("id:\t'{id}' ")),
        1 => invalid_id().prop_map(|id| format!("id: {id}")),
        1 => Just("id:".to_string()),
        2 => Just("title: Roadmap".to_string()),
        1 => Just("tags: [a, b]".to_string()),
        1 => Just("type: chords".to_string()),
        1 => valid_id().prop_map(|id| format!("ids: {id}")),
        1 => valid_id().prop_map(|id| format!("  id: {id}")),
        1 => Just(String::new()),
    ]
    .boxed()
}

/// A front-matter block (both fences and the final line break), LF or CRLF, with `--- ` fence variants.
pub fn front_matter_block() -> BoxedStrategy<String> {
    (
        select(vec!["---", "--- ", "---\t"]),
        vec(front_matter_line(), 0..4),
        select(vec!["---", "---  "]),
        prop_oneof![3 => Just("\n"), 1 => Just("\r\n")],
    )
        .prop_map(|(open, keys, close, nl)| {
            let mut out = format!("{open}{nl}");
            for k in keys {
                out.push_str(&k);
                out.push_str(nl);
            }
            out.push_str(close);
            out.push_str(nl);
            out
        })
        .boxed()
}

/// A note: sometimes a byte order mark, sometimes a front-matter block (whose closing fence may end the file),
/// then a body.
pub fn note_text() -> BoxedStrategy<String> {
    (
        prop::bool::weighted(0.15),
        prop::option::weighted(0.6, front_matter_block()),
        body_text(),
    )
        .prop_map(|(bom, block, body)| {
            let bom = if bom { "\u{feff}" } else { "" };
            match block {
                Some(b) if body.is_empty() => format!("{bom}{}", strip_final_newline(b)),
                Some(b) => format!("{bom}{b}{body}"),
                None => format!("{bom}{body}"),
            }
        })
        .boxed()
}

// ---------------------------------------------------------------------------------------------------------------
// Manifests
// ---------------------------------------------------------------------------------------------------------------

/// Strings for manifest paths and values: Unicode, quotes, backslashes, controls, characters above U+FFFF.
pub fn json_string() -> BoxedStrategy<String> {
    prop_oneof![
        2 => any_path(),
        1 => any_short_string(8),
        1 => any::<String>(),
    ]
    .boxed()
}

pub fn manifest_entry() -> BoxedStrategy<ManifestEntry> {
    (
        prop_oneof![(0u8..3).prop_map(content_hash), json_string()],
        json_string(),
        any::<i64>(),
    )
        .prop_map(|(sha256, version, synced_at)| ManifestEntry {
            sha256,
            version,
            synced_at,
        })
        .boxed()
}

/// Any manifest: arbitrary device id, timestamps over the whole `i64` range and arbitrary paths.
pub fn manifest() -> BoxedStrategy<Manifest> {
    (
        json_string(),
        any::<i64>(),
        hash_map(json_string(), manifest_entry(), 0..8),
    )
        .prop_map(|(device_id, updated_at, files)| Manifest {
            device_id,
            updated_at,
            files,
        })
        .boxed()
}

/// Any JSON value (bounded depth and size).
pub fn json_value() -> BoxedStrategy<Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::from),
        any_short_string(6).prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 5, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..4).prop_map(Value::Array),
            hash_map(any_short_string(5), inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
    .boxed()
}

/// A value of a manifest field: missing (`None`), well-typed, or of another type.
fn field(good: Vec<Value>) -> impl Strategy<Value = Option<Value>> {
    let bad = vec![
        Value::Null,
        json!("1"),
        json!(true),
        json!([]),
        json!({}),
        json!(1.5),
        json!(-3),
        json!(1e300),
    ];
    prop_oneof![
        1 => Just(None),
        4 => select(good).prop_map(Some),
        2 => select(bad).prop_map(Some),
    ]
}

/// An object shaped like a manifest whose fields are each missing, well-typed or wrongly typed, with unknown keys.
pub fn manifest_like_json() -> BoxedStrategy<Value> {
    let entry = (
        field(vec![json!("aa"), json!("")]),
        field(vec![json!("1"), json!("")]),
        field(vec![json!(10), json!(0), json!(-1)]),
        any::<bool>(),
    )
        .prop_map(|(sha, version, synced_at, extra)| {
            let mut m = Map::new();
            for (k, v) in [
                ("sha256", sha),
                ("version", version),
                ("syncedAt", synced_at),
            ] {
                if let Some(v) = v {
                    m.insert(k.into(), v);
                }
            }
            if extra {
                m.insert("extra".into(), json!(1));
            }
            Value::Object(m)
        });
    let files = prop_oneof![
        1 => Just(None),
        1 => select(vec![Value::Null, json!([]), json!("x"), json!(1)]).prop_map(Some),
        4 => hash_map(json_string(), prop_oneof![6 => entry, 1 => json_value()], 0..4)
            .prop_map(|m| Some(Value::Object(m.into_iter().collect()))),
    ];
    (
        field(vec![
            json!(1),
            json!(1.0),
            json!(0),
            json!(-2),
            json!(0.5),
            json!(2),
            json!(1.0000001),
        ]),
        field(vec![json!("device"), json!("")]),
        field(vec![
            json!(1_700_000_000_000_i64),
            json!(0),
            json!(-5),
            json!(i64::MAX),
            json!(2.5),
        ]),
        files,
        prop::option::of(json_value()),
    )
        .prop_map(|(format, device_id, updated_at, files, unknown)| {
            let mut m = Map::new();
            let fields = [
                ("format", format),
                ("deviceId", device_id),
                ("updatedAt", updated_at),
                ("files", files),
            ];
            for (k, v) in fields {
                if let Some(v) = v {
                    m.insert(k.into(), v);
                }
            }
            if let Some(v) = unknown {
                m.insert("unknown".into(), v);
            }
            Value::Object(m)
        })
        .boxed()
}

/// Text that may or may not be a manifest: arbitrary strings, JSON values, manifest-like objects, valid manifests
/// and truncated ones.
pub fn json_text() -> BoxedStrategy<String> {
    prop_oneof![
        1 => any::<String>(),
        1 => json_value().prop_map(|v| v.to_string()),
        2 => manifest_like_json().prop_map(|v| v.to_string()),
        1 => manifest().prop_map(|m| serialize_manifest(&m)),
        1 => (manifest(), any::<Index>()).prop_map(|(m, cut)| {
            let text = serialize_manifest(&m);
            let mut at = cut.index(text.len() + 1);
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            text[..at].to_string()
        }),
    ]
    .boxed()
}

/// Records with unique paths (folders and files), with small hash and version ranges.
pub fn records(max: usize) -> BoxedStrategy<Vec<SyncRecord>> {
    hash_map(
        prop_oneof![note_path(3), attachment_path(), valid_path(3)],
        (any::<bool>(), 0u8..3, 0u8..3, 0i64..5),
        0..=max,
    )
    .prop_map(|m| {
        let mut v: Vec<SyncRecord> = m
            .into_iter()
            .map(|(path, (is_folder, h, ver, at))| SyncRecord {
                is_folder,
                remote_id: format!("id-{path}"),
                remote_version: if is_folder {
                    String::new()
                } else {
                    version(ver)
                },
                local_hash: if is_folder {
                    String::new()
                } else {
                    content_hash(h)
                },
                synced_at: at,
                path,
            })
            .collect();
        v.sort_by(|a, b| paths::compare(&a.path, &b.path));
        v
    })
    .boxed()
}

// ---------------------------------------------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------------------------------------------

/// 0 to 5 distinct drive ids.
pub fn drive_ids() -> BoxedStrategy<Vec<String>> {
    prop::collection::hash_set(
        select(vec!["dropbox", "onedrive", "proton", "d1", "d2", "d3", ""]),
        0..=5,
    )
    .prop_map(|s| {
        let mut v: Vec<String> = s.into_iter().map(str::to_string).collect();
        v.sort();
        v
    })
    .boxed()
}

/// Reports fed back to the scheduler, used in turn: (local changes, failed).
pub fn drive_reports() -> BoxedStrategy<Vec<(u32, bool)>> {
    vec(
        (
            prop_oneof![2 => Just(0u32), 2 => 1u32..4, 1 => Just(u32::MAX)],
            prop::bool::weighted(0.2),
        ),
        1..40,
    )
    .boxed()
}
