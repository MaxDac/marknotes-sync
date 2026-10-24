//! MERGE-PRESERVE and MERGE-RENDER on arbitrary texts (base, local and remote separated by NUL bytes): the merge
//! never panics, its text is the rendering of its chunks, and resolving every conflict with "both" keeps every line
//! of both sides.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| marknotes_sync_fuzz::check_merge(data));
