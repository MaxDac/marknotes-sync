//! PATH-NORM and PATH-COPY on arbitrary text (a path, then existing paths, one per line): normalization is
//! idempotent, and a conflicted copy name is free under case folding and in the same folder.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| marknotes_sync_fuzz::check_paths(data));
