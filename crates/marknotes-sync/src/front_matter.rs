//! Minimal YAML front-matter handling: only the `id` key is interpreted, every other line is kept verbatim.

const DELIMITER: &str = "---";
const ID_ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";
const ID_LENGTH: usize = 8;

pub fn is_valid_id(id: &str) -> bool {
    id.len() == ID_LENGTH && id.chars().all(|c| ID_ALPHABET.contains(c))
}

fn is_fence(line: &str) -> bool {
    line.trim_end_matches([' ', '\t']) == DELIMITER
}

const BOM: &str = "\u{feff}";

/// `text` split into its leading UTF-8 byte order mark ("" when there is none) and the rest.
fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix(BOM) {
        Some(rest) => (BOM, rest),
        None => ("", text),
    }
}

/// Splits `text` into the front-matter block (a leading byte order mark, both fences and the trailing line break,
/// or "" when there is no block) and the body.
pub fn split(text: &str) -> (&str, &str) {
    let (bom, rest) = split_bom(text);
    let (header, body) = split_block(rest);
    if header.is_empty() {
        ("", text)
    } else {
        (&text[..bom.len() + header.len()], body)
    }
}

fn split_block(text: &str) -> (&str, &str) {
    let Some(first_end) = text.find('\n') else {
        return ("", text);
    };
    if !is_fence(text[..first_end].trim_end_matches('\r')) {
        return ("", text);
    }
    let mut start = first_end + 1;
    while start < text.len() {
        let (content_end, next) = match text[start..].find('\n') {
            Some(i) => (start + i, start + i + 1),
            None => (text.len(), text.len()),
        };
        if is_fence(text[start..content_end].trim_end_matches('\r')) {
            return (&text[..next], &text[next..]);
        }
        start = next;
    }
    ("", text)
}

/// The key lines between the two fences.
fn header_lines(header: &str) -> Vec<&str> {
    if header.is_empty() {
        return Vec::new();
    }
    let trimmed = header.strip_suffix('\n').unwrap_or(header);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    let lines: Vec<&str> = trimmed
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    lines[1..lines.len() - 1].to_vec()
}

/// The raw value of an `id:` line, without surrounding spaces or quotes.
fn id_line_value(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("id:")?;
    let value = rest.trim_matches([' ', '\t']);
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    let value = value
        .strip_prefix('\'')
        .and_then(|v| v.strip_suffix('\''))
        .unwrap_or(value);
    Some(value)
}

/// The note id from the front-matter, or `None` when there is no valid id.
pub fn read_id(text: &str) -> Option<String> {
    let (header, _) = split(text);
    header_lines(header)
        .into_iter()
        .find_map(id_line_value)
        .filter(|v| is_valid_id(v))
        .map(str::to_string)
}

/// `text` with its front-matter `id` set to `id`: the first `id:` line is replaced (keeping its line ending),
/// otherwise `id` is added as the first key with the opening fence's line ending (and the block is created when
/// missing). Every other byte of the text is kept as it is: the other keys, the fences and their line endings, a
/// leading byte order mark.
pub fn with_id(text: &str, id: &str) -> String {
    let (bom, rest) = split_bom(text);
    let (header, body) = split_block(rest);
    if header.is_empty() {
        return format!("{bom}{DELIMITER}\nid: {id}\n{DELIMITER}\n{body}");
    }
    // Byte ranges of the header's lines, each without its '\n' (and without one '\r' before it).
    let mut lines = Vec::new();
    let mut start = 0;
    while start < header.len() {
        let end = header[start..]
            .find('\n')
            .map_or(header.len(), |i| start + i);
        let content_end = if header[start..end].ends_with('\r') {
            end - 1
        } else {
            end
        };
        lines.push(start..content_end);
        start = end + 1;
    }
    let inner = &lines[1..lines.len() - 1];
    let mut out = String::with_capacity(text.len() + id.len() + 6);
    out.push_str(bom);
    match inner
        .iter()
        .find(|r| id_line_value(&header[(*r).clone()]).is_some())
    {
        Some(r) => {
            out.push_str(&header[..r.start]);
            out.push_str("id: ");
            out.push_str(id);
            out.push_str(&header[r.end..]);
        }
        None => {
            let after_fence = lines[0].end;
            let fence_end = header[after_fence..]
                .find('\n')
                .map_or(header.len(), |i| after_fence + i + 1);
            out.push_str(&header[..fence_end]);
            out.push_str("id: ");
            out.push_str(id);
            out.push_str(&header[after_fence..fence_end]);
            out.push_str(&header[fence_end..]);
        }
    }
    out.push_str(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ids() {
        assert_eq!(
            read_id("---\nid: k7f3q2xa\n---\nBody").as_deref(),
            Some("k7f3q2xa")
        );
        assert_eq!(
            read_id("---\r\nid: k7f3q2xa\r\n---\r\nBody").as_deref(),
            Some("k7f3q2xa")
        );
        assert_eq!(
            read_id("---\nid: k7f3q2xa\n---").as_deref(),
            Some("k7f3q2xa")
        );
        assert_eq!(
            read_id("---  \nid: k7f3q2xa\n---\n").as_deref(),
            Some("k7f3q2xa")
        );
        assert_eq!(read_id("---\nid: not-an-id\n---\n"), None);
        assert_eq!(read_id("---\nid: k7f3q2xa\nBody\n"), None);
    }

    #[test]
    fn sets_ids() {
        assert_eq!(with_id("", "m4n5p6q7"), "---\nid: m4n5p6q7\n---\n");
        assert_eq!(
            with_id("---\r\ntitle: R\r\n---\r\nBody", "m4n5p6q7"),
            "---\r\nid: m4n5p6q7\r\ntitle: R\r\n---\r\nBody"
        );
        assert_eq!(
            with_id("---\nid: not valid\n---\nBody", "m4n5p6q7"),
            "---\nid: m4n5p6q7\n---\nBody"
        );
        assert_eq!(
            with_id("---\ntags: [a]\nid: k7f3q2xa\nx: 1\n---\nBody", "m4n5p6q7"),
            "---\ntags: [a]\nid: m4n5p6q7\nx: 1\n---\nBody"
        );
    }
}
