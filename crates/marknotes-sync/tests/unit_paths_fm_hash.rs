//! Unit tests for `paths.rs`, `front_matter.rs` and `hashing.rs`. Each test is named after the rule it checks and
//! cites the section of `spec/note-format.md` it comes from. Tests marked "Characterisation" pin behaviour the spec
//! leaves open, so that any change to it is deliberate.

use std::cmp::Ordering;

use marknotes_sync::{front_matter, hashing, paths};

const HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ID: &str = "m4n5p6q7";

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Notes and the `.md` extension
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "What is synced": notes are `.md` files, any case, whatever the characters before it.
#[test]
fn note_extension_matches_md_in_any_case() {
    for path in [
        "a.md",
        "a.MD",
        "a.Md",
        "a.mD",
        "Ünïcödé.md",
        "日本語.md",
        "😀.MD",
        "Work/€.md",
        "a.b.md",
    ] {
        assert!(paths::is_note(path), "{path:?} is a note");
    }
    for path in [
        "a.mdx",
        "a.markdown",
        "amd",
        "a.m d",
        "a.md.txt",
        "md",
        "",
        "a.ｍd",
    ] {
        assert!(!paths::is_note(path), "{path:?} is not a note");
    }
}

/// note-format.md, "Sync" > "What is synced": recognising `.md` must not split a multi-byte character (no panic, no
/// false positive) when the last three bytes fall inside one.
#[test]
fn note_extension_check_never_splits_a_multibyte_character() {
    // "a€x": the last three bytes start inside the three-byte "€".
    for path in ["a€x", "€", "é", "😀", "x😀", "md€"] {
        assert!(!paths::is_note(path), "{path:?}");
    }
}

/// note-format.md, "Sync" > "What is synced": `.md` alone names a hidden file (it starts with `.`), so it is never
/// synced although it ends with `.md`.
#[test]
fn bare_dot_md_is_hidden_and_not_synced() {
    assert!(!paths::is_syncable_path(".md"));
    assert!(!paths::is_syncable_path("Work/.md"));
}

// ---------------------------------------------------------------------------------------------------------------
// Case folding
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "What is synced": paths are identified by Unicode simple case folding (`Über.md` and
/// `über.md` are the same path).
#[test]
fn paths_equal_under_simple_case_folding_are_the_same_path() {
    for (a, b) in [
        ("Über.md", "über.md"),
        ("Work/Roadmap.MD", "work/roadmap.md"),
        // LATIN CAPITAL LETTER SHARP S folds to ß (the `S` mapping), not to "ss".
        ("ẞ.md", "ß.md"),
        // LATIN SMALL LETTER LONG S folds to s.
        ("ſ.md", "S.md"),
        // KELVIN SIGN, ANGSTROM SIGN and OHM SIGN fold to the letters.
        ("\u{212A}elvin", "kelvin"),
        ("\u{212B}", "å"),
        ("\u{2126}", "ω"),
        // Final sigma and capital sigma fold to σ.
        ("ΣΑΣ", "σας"),
        // MICRO SIGN folds to GREEK SMALL LETTER MU.
        ("µ", "\u{03BC}"),
        // Title-case digraph.
        ("ǅ", "ǆ"),
        // Georgian Asomtavruli folds to Nuskhuri.
        ("Ⴀ", "ⴀ"),
        // Cherokee small letters fold to the capital letters.
        ("\u{AB70}", "\u{13A0}"),
    ] {
        assert_eq!(paths::fold_case(a), paths::fold_case(b), "{a:?} ~ {b:?}");
    }
}

/// note-format.md, "Sync" > "What is synced": simple case folding never expands a character (`Straße.md` and
/// `STRASSE.md` are not the same path), and dotted/dotless i keep their identity.
#[test]
fn paths_that_only_full_case_folding_would_join_stay_distinct() {
    for (a, b) in [
        ("Straße.md", "STRASSE.md"),
        ("ß", "ss"),
        ("ẞ", "ss"),
        ("İ", "i"),
        ("İ", "i\u{307}"),
        ("ı", "i"),
        ("I", "ı"),
        ("ﬀ", "ff"),
        ("ŉ", "ʼn"),
    ] {
        assert_ne!(paths::fold_case(a), paths::fold_case(b), "{a:?} !~ {b:?}");
    }
}

/// note-format.md, "Sync" > "What is synced": simple case folding maps one character to one character.
#[test]
fn simple_case_folding_maps_each_character_to_one_character() {
    for s in ["ß", "ẞ", "İ", "ŉ", "ﬀ", "ΐ", "ᾈ", "Ǆ", "Work/Ünï 😀.md"] {
        assert_eq!(
            paths::fold_case(s).chars().count(),
            s.chars().count(),
            "{s:?}"
        );
    }
}

/// note-format.md, "Sync" > "What is synced": folding a folded path changes nothing.
#[test]
fn case_folding_is_idempotent() {
    for s in [
        "Über/ẞ/ſ.MD",
        "\u{212A}\u{212B}\u{2126}",
        "ΣΑΣ",
        "\u{AB70}\u{13F8}",
        "İı",
    ] {
        let once = paths::fold_case(s);
        assert_eq!(paths::fold_case(&once), once, "{s:?}");
    }
}

/// note-format.md, "Sync" > "What is synced": only simple case folding identifies paths, so the NFC and NFD
/// spellings of the same name are two different paths (no Unicode normalisation is applied).
#[test]
fn case_folding_leaves_unicode_normalisation_alone() {
    let nfc = "\u{00E9}.md";
    let nfd = "e\u{0301}.md";
    assert_ne!(paths::fold_case(nfc), paths::fold_case(nfd));
    assert_eq!(paths::fold_case("E\u{0301}.md"), nfd);
    assert_eq!(paths::fold_case("\u{00C9}.md"), nfc);
    assert!(paths::is_syncable_path(nfc) && paths::is_syncable_path(nfd));
    assert_eq!(paths::normalize(nfd).as_deref(), Some(nfd));
}

// ---------------------------------------------------------------------------------------------------------------
// Valid and invalid paths
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "What is synced": a relative, `/`-separated path whose segments break none of the rules
/// is synced as-is.
#[test]
fn well_formed_paths_are_syncable() {
    for path in [
        "a.md",
        "Work/Roadmap.md",
        "Ünï/日本.md",
        "a b/c d.md",
        "a.b.c.md",
        "x/y/z",
        "-dash.md",
        "~tilde.md",
        "a.",
        "Notes (conflicted copy) 2.md",
        // U+0085 and U+00A0 inside a name: neither is in the spec's control-character ranges.
        "a\u{85}b.md",
        "a\u{a0}b.md",
        "😀/😀.md",
    ] {
        assert!(paths::is_syncable_path(path), "{path:?} is syncable");
    }
}

/// note-format.md, "Sync" > "What is synced": no empty segment, no `.`/`..`, no segment starting with `.`, no
/// leading or trailing whitespace, `/`-separated (a backslash makes the path not normalised).
#[test]
fn malformed_paths_are_not_syncable() {
    for (path, why) in [
        ("", "empty path"),
        ("/a.md", "leading slash: empty first segment"),
        ("a.md/", "trailing slash: empty last segment"),
        ("a//b.md", "empty segment"),
        (".", "dot segment"),
        ("..", "dot-dot segment"),
        ("./a.md", "dot segment"),
        ("a/./b.md", "dot segment"),
        ("../a.md", "dot-dot segment"),
        ("a/../b.md", "dot-dot segment"),
        (".hidden.md", "hidden file"),
        ("a/.obsidian/x.md", "hidden folder"),
        (".marknotes/manifest.json", "hidden folder"),
        (" a.md", "leading space"),
        ("a.md ", "trailing space"),
        ("a/ b/c.md", "leading space in a folder"),
        ("a /b.md", "trailing space in a folder"),
        ("\ta.md", "leading tab"),
        ("a\\b.md", "backslash separator"),
        ("\\a.md", "leading backslash"),
    ] {
        assert!(!paths::is_syncable_path(path), "{path:?}: {why}");
    }
}

/// note-format.md, "Sync" > "What is synced": a name containing a control character (U+0000 to U+001F, or U+007F)
/// is not synced; other code points, including the C1 range, are allowed inside a name.
#[test]
fn control_characters_are_exactly_u0000_to_u001f_and_u007f() {
    for cp in 0u32..=0x9F {
        let c = char::from_u32(cp).unwrap();
        let path = format!("a{c}b.md");
        // `/` splits the name in two valid segments; `\` is not normalised.
        let expected_invalid = cp <= 0x1F || cp == 0x7F || c == '\\';
        assert_eq!(
            paths::is_syncable_path(&path),
            !expected_invalid,
            "U+{cp:04X} in {path:?}"
        );
    }
}

/// Characterisation (the spec says "whitespace" without defining it): leading or trailing Unicode white space such
/// as NO-BREAK SPACE or IDEOGRAPHIC SPACE also makes a name invalid.
#[test]
fn unicode_white_space_at_either_end_makes_a_name_invalid() {
    for path in [
        "\u{a0}a.md",
        "a.md\u{a0}",
        "\u{3000}a.md",
        "a\u{2003}/b.md",
        "\u{85}a.md",
    ] {
        assert!(!paths::is_syncable_path(path), "{path:?}");
    }
}

/// Characterisation of `normalize` (paths.rs): `\` becomes `/` and empty segments are dropped, so the root is "".
#[test]
fn normalize_turns_backslashes_into_slashes_and_drops_empty_segments() {
    for (input, expected) in [
        ("a\\b.md", "a/b.md"),
        ("/a//b/", "a/b"),
        ("\\a\\\\b\\", "a/b"),
        ("", ""),
        ("/", ""),
        ("\\\\", ""),
        ("Work/Roadmap.md", "Work/Roadmap.md"),
    ] {
        assert_eq!(
            paths::normalize(input).as_deref(),
            Some(expected),
            "{input:?}"
        );
    }
}

/// note-format.md, "Sync" > "What is synced": `normalize` rejects a path with an invalid segment.
#[test]
fn normalize_rejects_invalid_segments() {
    for input in [
        "../a.md", "a/./b", ".x", " a", "a ", "a\u{0}b", "a/\u{7f}", "a\\..\\b",
    ] {
        assert_eq!(paths::normalize(input), None, "{input:?}");
    }
}

const PATH_CORPUS: [&str; 18] = [
    "",
    "a.md",
    "/a.md",
    "a.md/",
    "a//b.md",
    "a\\b.md",
    "\\\\a\\\\b\\\\",
    "Work/Roadmap.md",
    "../a.md",
    ".hidden",
    " a",
    "a\tb",
    "Über/ẞ.md",
    "e\u{301}.md",
    "x/y/z/",
    "a.",
    "😀\\😀.MD",
    "/",
];

/// paths.rs `normalize`: normalising a normalised path changes nothing, and the result is either the root or a
/// syncable path (note-format.md, "Sync" > "What is synced": paths are "already normalised").
#[test]
fn normalize_is_idempotent_and_yields_syncable_paths() {
    for p in PATH_CORPUS {
        if let Some(n) = paths::normalize(p) {
            assert_eq!(paths::normalize(&n).as_deref(), Some(n.as_str()), "{p:?}");
            assert!(
                n.is_empty() || paths::is_syncable_path(&n),
                "{p:?} -> {n:?}"
            );
        }
    }
}

/// note-format.md, "Sync" > "What is synced": a syncable path is already in normal form.
#[test]
fn a_syncable_path_is_its_own_normal_form() {
    for p in PATH_CORPUS {
        if paths::is_syncable_path(p) {
            assert_eq!(paths::normalize(p).as_deref(), Some(p));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Order and path helpers
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "What is synced": paths are ordered by UTF-16 code units, which differs from code-point
/// order for characters above U+FFFF against BMP characters at or above U+E000.
#[test]
fn paths_sort_by_utf16_code_units() {
    // U+10000 is the surrogate pair D800 DC00, below U+E000 in UTF-16 but above it by code point.
    assert_eq!(paths::compare("\u{10000}", "\u{E000}"), Ordering::Less);
    assert_eq!(
        paths::compare("\u{1F600}.md", "\u{FF5E}.md"),
        Ordering::Less
    );
    assert_eq!(paths::compare("\u{D7FF}", "\u{10000}"), Ordering::Less);
    let mut v = vec![
        "b.md",
        "\u{FF5E}.md",
        "a/b",
        "a.md",
        "B.md",
        "\u{1F600}.md",
        "a",
        "a b",
    ];
    v.sort_by(|a, b| paths::compare(a, b));
    assert_eq!(
        v,
        [
            "B.md",
            "a",
            "a b",
            "a.md",
            "a/b",
            "b.md",
            "\u{1F600}.md",
            "\u{FF5E}.md"
        ]
    );
    assert_eq!(paths::compare("a", "a"), Ordering::Equal);
}

/// paths.rs helpers used by the plan: the root is "", depth counts segments, parent and name split at the last `/`.
#[test]
fn depth_parent_and_name_split_on_slashes() {
    assert_eq!(paths::depth(""), 0);
    assert_eq!(paths::depth("a"), 1);
    assert_eq!(paths::depth("a/b/c.md"), 3);
    assert_eq!(paths::parent("a/b/c.md"), "a/b");
    assert_eq!(paths::parent("a"), "");
    assert_eq!(paths::name("a/b/c.md"), "c.md");
    assert_eq!(paths::name("€/日本.md"), "日本.md");
    assert_eq!(
        paths::compare_by_depth_then_path("Z", "A/B"),
        Ordering::Less
    );
}

/// note-format.md, "Sync" > "Sync plan": "everything inside" a folder means below a whole segment boundary.
#[test]
fn ancestry_requires_a_segment_boundary() {
    assert!(paths::is_ancestor("é", "é/x"));
    assert!(!paths::is_ancestor("é", "éa/x"));
    assert!(paths::is_ancestor("a/b", "a/b/c/d"));
    assert!(!paths::is_ancestor("a/b", "a/bc"));
    assert!(!paths::is_ancestor("a/b", "a"));
}

// ---------------------------------------------------------------------------------------------------------------
// Attachments
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Images (attachments)": `attachments/<64 lowercase hex>.<png|jpg|gif|webp>` at the root; any
/// other name in `attachments/` is not an attachment.
#[test]
fn attachment_paths_need_64_lowercase_hex_and_a_known_extension() {
    for ext in ["png", "jpg", "gif", "webp"] {
        assert!(paths::is_attachment_path(&format!(
            "attachments/{HASH}.{ext}"
        )));
    }
    let upper = HASH.to_uppercase();
    let short = &HASH[..63];
    for path in [
        format!("attachments/{upper}.png"),
        format!("attachments/{short}.png"),
        format!("attachments/{HASH}0.png"),
        format!("attachments/{HASH}.jpeg"),
        format!("attachments/{HASH}.PNG"),
        format!("attachments/{HASH}.svg"),
        format!("attachments/{HASH}"),
        format!("attachments/{HASH}.png.md"),
        format!("attachments/{HASH}.png/"),
        format!("attachments/sub/{HASH}.png"),
        format!("Work/attachments/{HASH}.png"),
        format!("/attachments/{HASH}.png"),
        format!("{HASH}.png"),
        format!("attachments/g{}.png", &HASH[1..]),
    ] {
        assert!(!paths::is_attachment_path(&path), "{path:?}");
    }
}

/// note-format.md, "Sync" > "What is synced": notes inside `attachments/` are ignored; folders are always synced;
/// a folder merely starting with "attachments" is not the attachments folder.
#[test]
fn notes_inside_attachments_are_not_synced() {
    assert!(!paths::is_synced_entry("attachments/readme.md", false));
    assert!(!paths::is_synced_entry("attachments/sub/readme.md", false));
    assert!(paths::is_synced_entry("attachments", true));
    assert!(paths::is_synced_entry("attachments/sub", true));
    assert!(paths::is_synced_entry("attachmentsX/a.md", false));
    assert!(paths::is_synced_entry("Work/attachments/a.md", false));
    assert!(paths::is_synced_entry(
        &format!("attachments/{HASH}.webp"),
        false
    ));
    assert!(!paths::is_synced_entry("photo.png", false));
}

/// Characterisation (the spec names the folder `attachments/` but doesn't say whether that match folds case): a
/// differently cased `Attachments/` folder is an ordinary folder, so notes in it sync and image names are not
/// attachments.
#[test]
fn attachments_folder_is_matched_case_sensitively() {
    assert!(paths::is_synced_entry("Attachments/a.md", false));
    assert!(!paths::is_attachment_path(&format!(
        "Attachments/{HASH}.png"
    )));
}

// ---------------------------------------------------------------------------------------------------------------
// Conflicted copy names
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Sync" > "Conflicts": `<name> (conflicted copy) <n>.md` takes the first free `n` from 2, even
/// when a higher number is taken and a lower one is free.
#[test]
fn conflict_copy_takes_the_first_free_number_from_2() {
    for (existing, expected) in [
        (vec!["a (conflicted copy).md"], "a (conflicted copy) 2.md"),
        (
            vec!["a (conflicted copy).md", "a (conflicted copy) 3.md"],
            "a (conflicted copy) 2.md",
        ),
        (
            vec![
                "a (conflicted copy).md",
                "a (conflicted copy) 2.md",
                "a (conflicted copy) 3.md",
            ],
            "a (conflicted copy) 4.md",
        ),
        // A free unnumbered name is used first even when numbered ones are taken.
        (
            vec!["a (conflicted copy) 2.md", "a (conflicted copy) 3.md"],
            "a (conflicted copy).md",
        ),
    ] {
        assert_eq!(
            paths::conflict_copy_name("a.md", &strings(&existing)),
            expected
        );
    }
}

/// note-format.md, "Sync" > "Conflicts": numbering keeps going past one digit.
#[test]
fn conflict_copy_numbering_goes_past_nine() {
    let mut existing = vec!["n (conflicted copy).md".to_string()];
    existing.extend((2..=10).map(|n| format!("n (conflicted copy) {n}.md")));
    assert_eq!(
        paths::conflict_copy_name("n.md", &existing),
        "n (conflicted copy) 11.md"
    );
}

/// note-format.md, "Sync" > "Conflicts": a name is taken only when an existing path equals it (by simple case
/// folding); the note itself, paths in other folders and paths inside a folder of that name don't take it, the
/// folder itself does.
#[test]
fn conflict_copy_name_is_taken_only_by_an_equal_path() {
    let free = [
        "a.md",
        "Other/a (conflicted copy).md",
        "a (conflicted copy).md/x.md",
        "a (conflicted copy).md.bak",
    ];
    assert_eq!(
        paths::conflict_copy_name("a.md", &strings(&free)),
        "a (conflicted copy).md"
    );
    assert_eq!(
        paths::conflict_copy_name("a.md", &strings(&["a (conflicted copy).md"])),
        "a (conflicted copy) 2.md"
    );
}

/// note-format.md, "Sync" > "Conflicts": taken names are compared by simple case folding (Kelvin sign is `k`; `ß`
/// is not `ss`; NFC and NFD differ).
#[test]
fn conflict_copy_name_is_compared_by_simple_case_folding() {
    assert_eq!(
        paths::conflict_copy_name("k.md", &strings(&["\u{212A} (CONFLICTED COPY).MD"])),
        "k (conflicted copy) 2.md"
    );
    assert_eq!(
        paths::conflict_copy_name("ß.md", &strings(&["ss (conflicted copy).md"])),
        "ß (conflicted copy).md"
    );
    assert_eq!(
        paths::conflict_copy_name("ẞ.md", &strings(&["ß (conflicted copy).md"])),
        "ẞ (conflicted copy) 2.md"
    );
    assert_eq!(
        paths::conflict_copy_name("Straße.md", &strings(&["STRASSE (conflicted copy).md"])),
        "Straße (conflicted copy).md"
    );
    assert_eq!(
        paths::conflict_copy_name("e\u{301}.md", &strings(&["\u{e9} (conflicted copy).md"])),
        "e\u{301} (conflicted copy).md"
    );
}

/// note-format.md, "Sync" > "Conflicts": the copy is saved next to the note (same folder), and multi-byte names keep
/// every character.
#[test]
fn conflict_copy_is_in_the_same_folder() {
    for (path, expected) in [
        ("日本.md", "日本 (conflicted copy).md"),
        ("Ünï/€.md", "Ünï/€ (conflicted copy).md"),
        ("a/b.c/d.md", "a/b.c/d (conflicted copy).md"),
        ("😀/😀.MD", "😀/😀 (conflicted copy).MD"),
    ] {
        let copy = paths::conflict_copy_name(path, &[]);
        assert_eq!(copy, expected);
        assert_eq!(paths::parent(&copy), paths::parent(path));
        assert!(paths::is_syncable_path(&copy));
        assert!(paths::is_note(&copy));
    }
}

/// Characterisation (the spec writes the copy as `<name> (conflicted copy).md`): the extension keeps its case, so
/// `a.MD` gives `a (conflicted copy).MD`, still a note since `.md` matches in any case.
#[test]
fn conflict_copy_keeps_the_extension_case() {
    assert_eq!(
        paths::conflict_copy_name("a.MD", &[]),
        "a (conflicted copy).MD"
    );
    assert_eq!(
        paths::conflict_copy_name("a.MD", &strings(&["A (conflicted copy).md"])),
        "a (conflicted copy) 2.MD"
    );
}

/// Characterisation (the spec only covers `.md` notes): a name without an extension gets the suffix at the end,
/// a dot in a folder name doesn't count, and a leading dot is not an extension separator.
#[test]
fn conflict_copy_of_extensionless_and_dot_names() {
    for (path, expected) in [
        ("README", "README (conflicted copy)"),
        ("v1.0/README", "v1.0/README (conflicted copy)"),
        (".hidden", ".hidden (conflicted copy)"),
        (".hidden.md", ".hidden (conflicted copy).md"),
        (".md", ".md (conflicted copy)"),
        ("a.", "a (conflicted copy)."),
    ] {
        assert_eq!(paths::conflict_copy_name(path, &[]), expected, "{path:?}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Front matter
// ---------------------------------------------------------------------------------------------------------------

const TEXT_CORPUS: [&str; 24] = [
    "",
    "Body",
    "Body\n",
    "---",
    "---\n",
    "---\n---",
    "---\n---\nBody",
    "---\nid: k7f3q2xa\n---\nBody",
    "---\r\nid: k7f3q2xa\r\n---\r\nBody\r\n",
    "---\nid: k7f3q2xa\n---",
    "---\nid: k7f3q2xa\nBody",
    "\n---\nid: k7f3q2xa\n---\n",
    "\u{feff}---\nid: k7f3q2xa\n---\nBody",
    "\u{feff}Body",
    "\u{feff}\u{feff}---\nid: k7f3q2xa\n---\n",
    "---\ntitle: a\nid: bad\nx: 1\n---\nBody\n---\nmore\n---\n",
    "---\nid: \"k7f3q2xa\"\n---\n",
    "---\nid: bad\nid: k7f3q2xa\n---\n",
    "---\r\ntitle: a\n---\nBody",
    "---\ntitle: a\r\n---\r\nBody",
    "----\nid: k7f3q2xa\n----\n",
    "--- \t\nid: k7f3q2xa\n---\t \nBody",
    " ---\nid: k7f3q2xa\n---\n",
    "---\n\n---\n\n",
];

/// note-format.md, "Note ids": splitting into block and body loses nothing: block + body is the note.
#[test]
fn split_gives_back_the_input() {
    for text in TEXT_CORPUS {
        let (block, body) = front_matter::split(text);
        assert_eq!(format!("{block}{body}"), text, "{text:?}");
    }
}

/// note-format.md, "Note ids": the block is at the very top of the file, so a fence that is not on the first line
/// (or is indented) starts no block.
#[test]
fn opening_fence_must_be_the_first_line() {
    for text in [
        "\n---\nid: k7f3q2xa\n---\n",
        " ---\nid: k7f3q2xa\n---\n",
        "x\n---\nid: k7f3q2xa\n---\n",
        "\r\n---\nid: k7f3q2xa\n---\n",
    ] {
        assert_eq!(front_matter::split(text), ("", text), "{text:?}");
        assert_eq!(front_matter::read_id(text), None, "{text:?}");
    }
}

/// note-format.md, "Chord sequences" > "Detection" (front-matter as in "Note ids"): the opening fence may have
/// trailing spaces or tabs. Characterisation: the closing fence may too.
#[test]
fn fences_allow_trailing_spaces_and_tabs() {
    let text = "--- \t\nid: k7f3q2xa\n---\t \nBody";
    assert_eq!(front_matter::read_id(text).as_deref(), Some("k7f3q2xa"));
    assert_eq!(
        front_matter::split(text),
        ("--- \t\nid: k7f3q2xa\n---\t \n", "Body")
    );
}

/// note-format.md, "Note ids": the fence is `---`; four dashes are not a fence.
#[test]
fn four_dashes_are_not_a_fence() {
    let text = "----\nid: k7f3q2xa\n----\n";
    assert_eq!(front_matter::split(text), ("", text));
    assert_eq!(front_matter::read_id(text), None);
}

/// note-format.md, "Note ids": a block needs a closing `---` line; an unclosed one is body, so a new block is added
/// in front of it and the old lines stay in the body.
#[test]
fn unclosed_block_is_body_and_gets_a_new_block() {
    let text = "---\nid: k7f3q2xa\nBody";
    assert_eq!(front_matter::read_id(text), None);
    assert_eq!(front_matter::split(text), ("", text));
    assert_eq!(
        front_matter::with_id(text, ID),
        "---\nid: m4n5p6q7\n---\n---\nid: k7f3q2xa\nBody"
    );
}

/// note-format.md, "Note ids": a lone opening fence (with or without line break) is not a block.
#[test]
fn a_lone_fence_is_body() {
    for text in ["---", "---\n", "---\r\n"] {
        assert_eq!(front_matter::split(text), ("", text), "{text:?}");
        assert_eq!(
            front_matter::with_id(text, ID),
            format!("---\nid: m4n5p6q7\n---\n{text}")
        );
    }
}

/// note-format.md, "Note ids": an empty block has no `id:` line, so `id` is added as the first key.
#[test]
fn empty_block_gets_the_id_as_first_key() {
    let text = "---\n---\nBody";
    assert_eq!(front_matter::read_id(text), None);
    assert_eq!(front_matter::split(text), ("---\n---\n", "Body"));
    assert_eq!(
        front_matter::with_id(text, ID),
        "---\nid: m4n5p6q7\n---\nBody"
    );
}

/// note-format.md, "Note ids": a byte order mark before the opening fence belongs to the block and stays first;
/// a CRLF block keeps CRLF.
#[test]
fn bom_before_a_crlf_block_stays_first() {
    let text = "\u{feff}---\r\nid: k7f3q2xa\r\n---\r\nBody\r\n";
    assert_eq!(front_matter::read_id(text).as_deref(), Some("k7f3q2xa"));
    assert_eq!(
        front_matter::split(text),
        ("\u{feff}---\r\nid: k7f3q2xa\r\n---\r\n", "Body\r\n")
    );
    assert_eq!(
        front_matter::with_id(text, ID),
        "\u{feff}---\r\nid: m4n5p6q7\r\n---\r\nBody\r\n"
    );
}

/// note-format.md, "Note ids": the block may follow one byte order mark; a second one is content, so there is no
/// block and a new one is added after the first mark.
#[test]
fn only_one_leading_bom_belongs_to_the_block() {
    let text = "\u{feff}\u{feff}---\nid: k7f3q2xa\n---\n";
    assert_eq!(front_matter::read_id(text), None);
    assert_eq!(front_matter::split(text), ("", text));
    assert_eq!(
        front_matter::with_id(text, ID),
        "\u{feff}---\nid: m4n5p6q7\n---\n\u{feff}---\nid: k7f3q2xa\n---\n"
    );
}

/// note-format.md, "Note ids": the block keeps its own line endings; the body is never touched, even when it uses
/// other line endings.
#[test]
fn crlf_block_with_lf_body_keeps_both() {
    let text = "---\r\ntitle: R\r\n---\r\nBody\nmore\n";
    assert_eq!(
        front_matter::with_id(text, ID),
        "---\r\nid: m4n5p6q7\r\ntitle: R\r\n---\r\nBody\nmore\n"
    );
}

/// note-format.md, "Note ids": other keys are "kept verbatim" and the block keeps its line endings, so with mixed
/// line endings every line keeps its own; an added `id` line takes the opening fence's, a replaced one keeps its own.
#[test]
fn mixed_block_line_endings_are_kept_line_by_line() {
    assert_eq!(
        front_matter::with_id("---\r\ntitle: a\n---\nBody", ID),
        "---\r\nid: m4n5p6q7\r\ntitle: a\n---\nBody"
    );
    assert_eq!(
        front_matter::with_id("---\ntitle: a\r\n---\r\nBody", ID),
        "---\nid: m4n5p6q7\ntitle: a\r\n---\r\nBody"
    );
    assert_eq!(
        front_matter::with_id("---\r\nid: k7f3q2xa\n---\r\nBody", ID),
        "---\r\nid: m4n5p6q7\n---\r\nBody"
    );
}

/// note-format.md, "Note ids": only the `id` line changes, so a closing fence that is the last line of the file
/// stays without a line break (the editor adds one only when re-attaching a non-empty body).
#[test]
fn closing_fence_at_end_of_file_is_kept() {
    assert_eq!(
        front_matter::with_id("---\nid: k7f3q2xa\n---", ID),
        "---\nid: m4n5p6q7\n---"
    );
    assert_eq!(
        front_matter::with_id("---\r\ntitle: a\r\n---", ID),
        "---\r\nid: m4n5p6q7\r\ntitle: a\r\n---"
    );
}

/// note-format.md, "Note ids": fences and key lines are kept byte for byte, including trailing spaces on a fence
/// and a carriage return that is part of a key line in an LF block.
#[test]
fn fences_and_stray_carriage_returns_are_kept() {
    assert_eq!(
        front_matter::with_id("--- \t\ntitle: a\n---  \nBody", ID),
        "--- \t\nid: m4n5p6q7\ntitle: a\n---  \nBody"
    );
    assert_eq!(
        front_matter::with_id("---\ntrailing \r\r\n---\n", ID),
        "---\nid: m4n5p6q7\ntrailing \r\r\n---\n"
    );
}

/// note-format.md, "Note ids": an `id:` line is a line starting with `id:`; indented, upper-case or other keys are
/// not id lines, so the id is added as the first key.
#[test]
fn id_line_must_start_with_id_colon() {
    for line in [
        " id: k7f3q2xa",
        "\tid: k7f3q2xa",
        "ID: k7f3q2xa",
        "Id: k7f3q2xa",
        "id : k7f3q2xa",
        "ids: k7f3q2xa",
        "# id: k7f3q2xa",
    ] {
        let text = format!("---\n{line}\n---\nBody");
        assert_eq!(front_matter::read_id(&text), None, "{line:?}");
        assert_eq!(
            front_matter::with_id(&text, ID),
            format!("---\nid: m4n5p6q7\n{line}\n---\nBody"),
            "{line:?}"
        );
    }
}

/// Characterisation (the spec doesn't describe YAML quoting or spacing for `id`): the value is trimmed of spaces and
/// tabs and may be wrapped in one pair of matching quotes; `id:` needs no space after the colon.
#[test]
fn id_value_may_be_quoted_or_unspaced() {
    for line in [
        "id: \"k7f3q2xa\"",
        "id: 'k7f3q2xa'",
        "id:k7f3q2xa",
        "id:\tk7f3q2xa \t",
        "id:   'k7f3q2xa'   ",
    ] {
        let text = format!("---\n{line}\n---\n");
        assert_eq!(
            front_matter::read_id(&text).as_deref(),
            Some("k7f3q2xa"),
            "{line:?}"
        );
        assert_eq!(front_matter::with_id(&text, ID), "---\nid: m4n5p6q7\n---\n");
    }
    for line in [
        "id: \"k7f3q2xa'",
        "id: \"k7f3q2xa",
        "id: k7f3q2xa # comment",
        "id: [k7f3q2xa]",
        "id: ''k7f3q2xa''",
    ] {
        let text = format!("---\n{line}\n---\n");
        assert_eq!(front_matter::read_id(&text), None, "{line:?}");
    }
}

/// Characterisation (the spec says "when the block has an `id:` line it is replaced" without covering duplicates):
/// only the first `id:` line counts, both for reading and for replacing.
#[test]
fn only_the_first_id_line_counts() {
    assert_eq!(
        front_matter::read_id("---\nid: bad\nid: k7f3q2xa\n---\n"),
        None
    );
    assert_eq!(
        front_matter::read_id("---\nid: k7f3q2xa\nid: m4n5p6q7\n---\n").as_deref(),
        Some("k7f3q2xa")
    );
    assert_eq!(
        front_matter::with_id("---\nid: bad\nid: k7f3q2xa\n---\nB", ID),
        "---\nid: m4n5p6q7\nid: k7f3q2xa\n---\nB"
    );
}

/// note-format.md, "Note ids": an id is 8 characters from `0123456789abcdefghjkmnpqrstvwxyz`; anything else is not a
/// valid id, so `read_id` returns none and `with_id` replaces the line.
#[test]
fn invalid_id_values_are_not_ids() {
    for value in [
        "",
        "k7f3q2x",
        "k7f3q2xa0",
        "K7F3Q2XA",
        "k7f3q2xA",
        "k7f3q2xi",
        "k7f3q2xl",
        "k7f3q2xo",
        "k7f3q2xu",
        "k7f3-2xa",
        "k7f3 2xa",
        "k7f3q2xé",
        "ｋ7f3q2xa",
        "k7f3q2x\u{0}",
        "k7f3q2x\u{7f}",
    ] {
        assert!(!front_matter::is_valid_id(value), "{value:?}");
        let text = format!("---\nid: {value}\n---\nBody");
        assert_eq!(front_matter::read_id(&text), None, "{value:?}");
        assert_eq!(
            front_matter::with_id(&text, ID),
            "---\nid: m4n5p6q7\n---\nBody",
            "{value:?}"
        );
    }
    for value in ["k7f3q2xa", "01234567", "zzzzzzzz", "0123abcd", "vwxyz012"] {
        assert!(front_matter::is_valid_id(value), "{value:?}");
    }
}

/// note-format.md, "Note ids": every letter of the alphabet is valid and the four excluded ones are not.
#[test]
fn id_alphabet_is_lowercase_crockford_base32() {
    for c in ('0'..='9').chain('a'..='z') {
        let id = c.to_string().repeat(8);
        let expected = !matches!(c, 'i' | 'l' | 'o' | 'u');
        assert_eq!(front_matter::is_valid_id(&id), expected, "{c}");
    }
}

/// note-format.md, "Note ids": other keys are kept verbatim and in order; `id` is added as the first key.
#[test]
fn with_id_keeps_other_keys_verbatim() {
    let text = "---\ntitle: \"a: b\"\ntags:\n  - x\n  - y\n# comment\n\nempty:\n---\nBody";
    assert_eq!(
        front_matter::with_id(text, ID),
        "---\nid: m4n5p6q7\ntitle: \"a: b\"\ntags:\n  - x\n  - y\n# comment\n\nempty:\n---\nBody"
    );
}

/// note-format.md, "Note ids": fence-like lines after the block are body.
#[test]
fn fence_lines_in_the_body_are_body() {
    let text = "---\nid: k7f3q2xa\n---\nBody\n---\nid: m4n5p6q7\n---\n";
    assert_eq!(
        front_matter::split(text),
        ("---\nid: k7f3q2xa\n---\n", "Body\n---\nid: m4n5p6q7\n---\n")
    );
    assert_eq!(front_matter::read_id(text).as_deref(), Some("k7f3q2xa"));
}

/// note-format.md, "Note ids": setting the same id twice gives the same text as setting it once.
#[test]
fn with_id_is_idempotent() {
    for text in TEXT_CORPUS {
        let once = front_matter::with_id(text, ID);
        assert_eq!(front_matter::with_id(&once, ID), once, "{text:?}");
    }
}

/// note-format.md, "Note ids": setting the id reads back that id, and never changes the body (a note without a
/// block becomes the body of the new block, after a leading byte order mark moves into the block).
#[test]
fn with_id_reads_back_and_preserves_the_body() {
    for text in TEXT_CORPUS {
        let out = front_matter::with_id(text, ID);
        assert_eq!(front_matter::read_id(&out).as_deref(), Some(ID), "{text:?}");
        let (block, body) = front_matter::split(text);
        let expected_body = if block.is_empty() {
            text.strip_prefix('\u{feff}').unwrap_or(text)
        } else {
            body
        };
        assert_eq!(front_matter::split(&out).1, expected_body, "{text:?}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------------------------------------------

/// note-format.md, "Images (attachments)" and "Records and base copies": SHA-256 as lowercase hex; NIST FIPS 180-2
/// test vectors.
#[test]
fn sha256_matches_the_nist_test_vectors() {
    for (input, expected) in [
        (
            b"".to_vec(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc".to_vec(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".to_vec(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
                .to_vec(),
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
        ),
        (
            vec![b'a'; 1_000_000],
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
        ),
    ] {
        assert_eq!(hashing::sha256_hex(&input), expected, "{} bytes", input.len());
    }
}

/// note-format.md, "Images (attachments)": the hash is 64 lowercase hex characters, so it names a valid attachment.
#[test]
fn sha256_is_64_lowercase_hex_and_names_an_attachment() {
    for bytes in [&b""[..], b"\x00\xff", "Über".as_bytes()] {
        let hash = hashing::sha256_hex(bytes);
        assert_eq!(hash.len(), 64);
        assert!(hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert!(paths::is_attachment_path(&format!(
            "attachments/{hash}.png"
        )));
    }
}
