//! Line diff (Myers' O(ND) algorithm) and line-based three-way merge (diff3).

use std::collections::HashMap;

#[cfg(feature = "uniffi")]
use uniffi;

/// Above this edit distance the inputs are treated as completely different, to bound memory.
const MAX_EDIT_DISTANCE: usize = 2000;

pub const MARKER_LOCAL: &str = "<<<<<<< this device";
pub const MARKER_SEPARATOR: &str = "=======";
pub const MARKER_REMOTE: &str = ">>>>>>> drive";

/// Splits `text` into lines, keeping each `\n` so that joining the result gives back `text`.
pub fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Matching index pairs `(a, b)` of a longest common subsequence of `a` and `b`, in increasing order.
pub fn matches<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mut result: Vec<(usize, usize)> = (0..prefix).map(|i| (i, i)).collect();
    let mid_a = &a[prefix..a.len() - suffix];
    let mid_b = &b[prefix..b.len() - suffix];
    result.extend(
        myers(mid_a, mid_b)
            .into_iter()
            .map(|(x, y)| (x + prefix, y + prefix)),
    );
    result.extend((0..suffix).map(|i| (a.len() - suffix + i, b.len() - suffix + i)));
    result
}

fn myers<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let max = n + m;
    let offset = max + 1;
    let mut v = vec![0isize; (2 * max + 3) as usize];
    // trace[d] holds V for diagonals -d..=d before step d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = false;
    for d in 0..=max {
        if d as usize > MAX_EDIT_DISTANCE {
            return Vec::new();
        }
        trace.push(v[(offset - d - 1) as usize..(offset + d + 2) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d
                || (k != d && v[(offset + k - 1) as usize] < v[(offset + k + 1) as usize])
            {
                v[(offset + k + 1) as usize]
            } else {
                v[(offset + k - 1) as usize] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(offset + k) as usize] = x;
            if x >= n && y >= m {
                found = true;
                break;
            }
            k += 2;
        }
        if found {
            break;
        }
    }

    let mut result = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let slice = &trace[d as usize];
        let vd = |k: isize| slice[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && vd(k - 1) < vd(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = if d == 0 { 0 } else { vd(prev_k) };
        let prev_y = if d == 0 { 0 } else { prev_x - prev_k };
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            result.push((x as usize, y as usize));
        }
        x = prev_x;
        y = prev_y;
    }
    result.reverse();
    result
}

/// A region of a three-way merge.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MergeChunk {
    /// Lines everyone agrees on, or that only one side changed (taken automatically).
    Resolved { lines: Vec<String>, changed: bool },
    /// Both sides changed the same region differently.
    Conflict {
        base: Vec<String>,
        local: Vec<String>,
        remote: Vec<String>,
    },
}

/// How a single conflicting region is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "camelCase")]
pub enum ConflictChoice {
    Local,
    Remote,
    Both,
    Neither,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub chunks: Vec<MergeChunk>,
    pub has_conflicts: bool,
    /// Number of conflicting regions.
    pub conflicts: i32,
    /// The merged text with unresolved conflicts written with markers.
    pub text: String,
}

fn push_lines(out: &mut String, lines: &[String]) {
    for l in lines {
        out.push_str(l);
    }
}

/// Appends lines making sure the last one ends with a newline, so that following text starts on a new line.
fn push_lines_terminated(out: &mut String, lines: &[String]) {
    push_lines(out, lines);
    if lines.last().is_some_and(|l| !l.ends_with('\n')) {
        out.push('\n');
    }
}

/// Builds the merged text. `choices` maps a conflict index (0 = first conflict) to its resolution; unresolved
/// conflicts are written with git-style markers.
pub fn merge_text(chunks: &[MergeChunk], choices: &HashMap<i32, ConflictChoice>) -> String {
    let mut out = String::new();
    let mut index = 0i32;
    for chunk in chunks {
        match chunk {
            MergeChunk::Resolved { lines, .. } => push_lines(&mut out, lines),
            MergeChunk::Conflict { local, remote, .. } => {
                match choices.get(&index) {
                    Some(ConflictChoice::Local) => push_lines(&mut out, local),
                    Some(ConflictChoice::Remote) => push_lines(&mut out, remote),
                    Some(ConflictChoice::Both) => {
                        push_lines_terminated(&mut out, local);
                        push_lines(&mut out, remote);
                    }
                    Some(ConflictChoice::Neither) => {}
                    None => {
                        out.push_str(MARKER_LOCAL);
                        out.push('\n');
                        push_lines_terminated(&mut out, local);
                        out.push_str(MARKER_SEPARATOR);
                        out.push('\n');
                        push_lines_terminated(&mut out, remote);
                        out.push_str(MARKER_REMOTE);
                        out.push('\n');
                    }
                }
                index += 1;
            }
        }
    }
    out
}

fn result(chunks: Vec<MergeChunk>) -> MergeResult {
    let conflicts = chunks
        .iter()
        .filter(|c| matches!(c, MergeChunk::Conflict { .. }))
        .count() as i32;
    let text = merge_text(&chunks, &HashMap::new());
    MergeResult {
        chunks,
        has_conflicts: conflicts > 0,
        conflicts,
        text,
    }
}

fn owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_string()).collect()
}

/// Merges `local` and `remote` against their common ancestor `base`. Without a base (the two versions were
/// never in sync) nothing can be merged safely: lines common to both are kept and every difference is a
/// conflict.
pub fn merge3(base: Option<&str>, local: &str, remote: &str) -> MergeResult {
    let l = lines(local);
    let r = lines(remote);
    if local == remote {
        let chunks = if l.is_empty() {
            vec![]
        } else {
            vec![MergeChunk::Resolved {
                lines: owned(&l),
                changed: false,
            }]
        };
        return result(chunks);
    }
    let o: Vec<&str> = match base {
        Some(b) => lines(b),
        None => matches(&l, &r).into_iter().map(|(i, _)| l[i]).collect(),
    };
    result(diff3(&o, &l, &r, base.is_some()))
}

struct Chunks {
    chunks: Vec<MergeChunk>,
    auto_resolve: bool,
}

impl Chunks {
    fn add_resolved(&mut self, lines: &[&str], changed: bool) {
        if lines.is_empty() {
            return;
        }
        if let Some(MergeChunk::Resolved {
            lines: last,
            changed: last_changed,
        }) = self.chunks.last_mut()
        {
            if *last_changed == changed {
                last.extend(lines.iter().map(|l| (*l).to_string()));
                return;
            }
        }
        self.chunks.push(MergeChunk::Resolved {
            lines: owned(lines),
            changed,
        });
    }

    fn add_unstable(&mut self, o: &[&str], a: &[&str], b: &[&str]) {
        if a == b {
            self.add_resolved(a, a != o);
        } else if self.auto_resolve && a == o {
            self.add_resolved(b, true);
        } else if self.auto_resolve && b == o {
            self.add_resolved(a, true);
        } else {
            self.chunks.push(MergeChunk::Conflict {
                base: owned(o),
                local: owned(a),
                remote: owned(b),
            });
        }
    }
}

fn diff3(o: &[&str], a: &[&str], b: &[&str], auto_resolve: bool) -> Vec<MergeChunk> {
    let mut match_a = vec![usize::MAX; o.len()];
    let mut match_b = vec![usize::MAX; o.len()];
    for (i, j) in matches(o, a) {
        match_a[i] = j;
    }
    for (i, j) in matches(o, b) {
        match_b[i] = j;
    }
    let mut out = Chunks {
        chunks: Vec::new(),
        auto_resolve,
    };
    let (mut lo, mut la, mut lb) = (0usize, 0usize, 0usize);
    loop {
        let mut i = 0;
        while lo + i < o.len() && match_a[lo + i] == la + i && match_b[lo + i] == lb + i {
            i += 1;
        }
        if i > 0 {
            out.add_resolved(&o[lo..lo + i], false);
            lo += i;
            la += i;
            lb += i;
            continue;
        }
        let mut next = lo;
        while next < o.len() && (match_a[next] == usize::MAX || match_b[next] == usize::MAX) {
            next += 1;
        }
        if next >= o.len() {
            if lo < o.len() || la < a.len() || lb < b.len() {
                out.add_unstable(&o[lo..], &a[la..], &b[lb..]);
            }
            break;
        }
        out.add_unstable(&o[lo..next], &a[la..match_a[next]], &b[lb..match_b[next]]);
        lo = next;
        la = match_a[next];
        lb = match_b[next];
    }
    out.chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_merge() {
        let m = merge3(Some("a\nb\nc\n"), "A\nb\nc\n", "a\nb\nC\n");
        assert!(!m.has_conflicts);
        assert_eq!(m.text, "A\nb\nC\n");
    }

    #[test]
    fn conflict_markers() {
        let m = merge3(Some("x"), "local", "remote");
        assert_eq!(m.conflicts, 1);
        assert_eq!(
            m.text,
            "<<<<<<< this device\nlocal\n=======\nremote\n>>>>>>> drive\n"
        );
    }

    #[test]
    fn no_base_never_auto_resolves() {
        let m = merge3(None, "a\nb\n", "a\nc\n");
        assert!(m.has_conflicts);
    }

    #[test]
    fn choices() {
        let m = merge3(Some("a\nb\nc\n"), "a\nl\nc\n", "a\nr\nc\n");
        let both = HashMap::from([(0, ConflictChoice::Both)]);
        assert_eq!(merge_text(&m.chunks, &both), "a\nl\nr\nc\n");
    }

    #[test]
    fn diff_is_lcs() {
        let a = lines("a\nb\nc\nd\n");
        let b = lines("a\nc\nd\ne\n");
        assert_eq!(matches(&a, &b), vec![(0, 0), (2, 1), (3, 2)]);
    }
}
