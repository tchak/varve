#![no_main]
//! Entry decoding never panics, and an accepted entry re-encodes to the
//! same canonical value.

mod common;

use libfuzzer_sys::fuzz_target;
use varve_record::canon::{entry_canonical, entry_from};

fuzz_target!(|data: &[u8]| {
    let Some(value) = common::canonical_from_bytes(data) else { return };
    let Ok(entry) = entry_from(&value) else { return };
    let canonical = entry_canonical(&entry);
    let again = entry_from(&canonical).expect("own canonical form decodes");
    assert_eq!(again, entry);
    assert_eq!(entry_canonical(&again), canonical);
});
