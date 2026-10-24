//! FM-ID on arbitrary text: `with_id` never panics, the id reads back, the body is untouched, and setting the same
//! id again changes nothing.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| marknotes_sync_fuzz::check_front_matter(data));
