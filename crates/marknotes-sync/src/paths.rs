//! Helpers for the relative, `/`-separated paths used by the apps. The root folder is the empty string.

use std::cmp::Ordering;

pub const NOTE_EXTENSION: &str = ".md";
pub const ATTACHMENTS_FOLDER: &str = "attachments";
const ATTACHMENT_EXTENSIONS: [&str; 4] = ["png", "jpg", "gif", "webp"];

/// Orders strings by UTF-16 code units, like Kotlin's and JavaScript's default string sort.
pub fn compare(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

pub fn depth(path: &str) -> usize {
    if path.is_empty() {
        0
    } else {
        path.split('/').count()
    }
}

/// Folders first by depth, then by path: parents always come before their children.
pub fn compare_by_depth_then_path(a: &str, b: &str) -> Ordering {
    depth(a).cmp(&depth(b)).then_with(|| compare(a, b))
}

pub fn parent(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

pub fn name(path: &str) -> &str {
    path.rfind('/').map_or(path, |i| &path[i + 1..])
}

pub fn is_ancestor(ancestor: &str, path: &str) -> bool {
    ancestor.is_empty()
        || (path.len() > ancestor.len()
            && path.starts_with(ancestor)
            && path.as_bytes()[ancestor.len()] == b'/')
}

pub fn is_note(path: &str) -> bool {
    path.len() >= NOTE_EXTENSION.len()
        && path.is_char_boundary(path.len() - NOTE_EXTENSION.len())
        && path[path.len() - NOTE_EXTENSION.len()..].eq_ignore_ascii_case(NOTE_EXTENSION)
}

fn is_valid_name(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed == value
        && !value.contains('/')
        && !value.contains('\\')
        && value != "."
        && value != ".."
        && !value.starts_with('.')
}

/// Normalizes a relative path (`\` becomes `/`, empty segments are dropped); `None` when a segment is not a
/// valid file or folder name.
pub fn normalize(path: &str) -> Option<String> {
    let replaced = path.replace('\\', "/");
    let parts: Vec<&str> = replaced.split('/').filter(|p| !p.is_empty()).collect();
    if parts.iter().all(|p| is_valid_name(p)) {
        Some(parts.join("/"))
    } else {
        None
    }
}

/// A path that can be synced as-is: non-empty and already normalized.
pub fn is_syncable_path(path: &str) -> bool {
    !path.is_empty() && normalize(path).as_deref() == Some(path)
}

/// `attachments/<64 lowercase hex>.<png|jpg|gif|webp>`, directly in the root attachments folder.
pub fn is_attachment_path(path: &str) -> bool {
    let Some(file) = path.strip_prefix("attachments/") else {
        return false;
    };
    let Some((hash, ext)) = file.split_once('.') else {
        return false;
    };
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && ATTACHMENT_EXTENSIONS.contains(&ext)
}

fn is_in_attachments_folder(path: &str) -> bool {
    is_ancestor(ATTACHMENTS_FOLDER, path)
}

/// Folders, markdown notes and attachments are synced; every other file (and notes inside `attachments/`)
/// is ignored.
pub fn is_synced_entry(path: &str, is_folder: bool) -> bool {
    is_folder || is_attachment_path(path) || (is_note(path) && !is_in_attachments_folder(path))
}

/// Name of the copy kept for the losing side of a conflict: `<stem> (conflicted copy).<ext>` next to the note,
/// or `<stem> (conflicted copy) <n>.<ext>` with the first free `n` (from 2) when that path is taken.
/// Paths are compared case-insensitively, because some file systems are.
pub fn conflict_copy_name(path: &str, existing_paths: &[String]) -> String {
    let dir = parent(path);
    let file = name(path);
    let (stem, ext) = match file.rfind('.') {
        Some(i) if i > 0 => (&file[..i], &file[i..]),
        _ => (file, ""),
    };
    let taken: std::collections::HashSet<String> =
        existing_paths.iter().map(|p| p.to_lowercase()).collect();
    let mut n = 1u32;
    loop {
        let candidate_name = if n == 1 {
            format!("{stem} (conflicted copy){ext}")
        } else {
            format!("{stem} (conflicted copy) {n}{ext}")
        };
        let candidate = if dir.is_empty() {
            candidate_name
        } else {
            format!("{dir}/{candidate_name}")
        };
        if !taken.contains(&candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncable_paths() {
        assert!(is_syncable_path("Work/Roadmap.md"));
        assert!(!is_syncable_path(""));
        assert!(!is_syncable_path("Work//Roadmap.md"));
        assert!(!is_syncable_path("Work\\Roadmap.md"));
        assert!(!is_syncable_path(".marknotes/manifest.json"));
        assert!(!is_syncable_path("Work/ Roadmap.md"));
        assert!(!is_syncable_path("../x.md"));
    }

    #[test]
    fn synced_entries() {
        assert!(is_synced_entry("Work", true));
        assert!(is_synced_entry("a.MD", false));
        assert!(!is_synced_entry("a.txt", false));
        assert!(!is_synced_entry("attachments/readme.md", false));
        let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(is_synced_entry(&format!("attachments/{hash}.png"), false));
        assert!(!is_attachment_path(&format!("attachments/{hash}.jpeg")));
        assert!(!is_attachment_path(&format!("attachments/x/{hash}.png")));
    }

    #[test]
    fn ancestors() {
        assert!(is_ancestor("", "a"));
        assert!(is_ancestor("a", "a/b"));
        assert!(!is_ancestor("a", "a"));
        assert!(!is_ancestor("a", "ab/c"));
    }

    #[test]
    fn utf16_order() {
        // U+FF5E sorts after U+1F600 in UTF-16 (surrogates are 0xD800..) but before it by code point.
        assert_eq!(compare("\u{1F600}", "\u{FF5E}"), Ordering::Less);
        assert_eq!(compare("B", "a"), Ordering::Less);
    }

    #[test]
    fn conflict_copies() {
        assert_eq!(
            conflict_copy_name("Work/v1.2 notes.md", &[]),
            "Work/v1.2 notes (conflicted copy).md"
        );
        let existing = vec![
            "a (Conflicted Copy).md".to_string(),
            "a (conflicted copy) 2.md".to_string(),
        ];
        assert_eq!(
            conflict_copy_name("a.md", &existing),
            "a (conflicted copy) 3.md"
        );
        assert_eq!(
            conflict_copy_name("README", &[]),
            "README (conflicted copy)"
        );
    }
}
