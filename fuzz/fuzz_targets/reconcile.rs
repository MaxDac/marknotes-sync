//! RECON-NOLOSS on arbitrary inputs: reconcile never panics, a merge result is the clean three-way merge, and when
//! the newest wins the losing text is kept (its body unchanged) in a copy with the new id.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| marknotes_sync_fuzz::check_reconcile(data));
