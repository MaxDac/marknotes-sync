//! PLAN-TOTAL and PLAN-DET on arbitrary inputs: any SyncInput that deserializes plans without panicking, the same
//! way twice, and its JSON form round-trips (what the bindings send).
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| marknotes_sync_fuzz::check_plan(data));
