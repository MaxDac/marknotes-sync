//! MAN-REJECT and MAN-ROUNDTRIP on arbitrary text: parsing never panics, and a manifest that parses serializes to
//! text that parses back to the same manifest and serializes the same way (canonical form).
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| marknotes_sync_fuzz::check_manifest(data));
