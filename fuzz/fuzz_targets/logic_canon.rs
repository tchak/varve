#![no_main]
//! `from_canonical` never panics, and `to_canonical` of an accepted
//! expression decodes back to the same expression.

mod common;

use libfuzzer_sys::fuzz_target;
use varve_logic::{from_canonical, to_canonical};

fuzz_target!(|data: &[u8]| {
    let Some(value) = common::canonical_from_bytes(data) else { return };
    let Ok(expr) = from_canonical(&value) else { return };
    let canonical = to_canonical(&expr);
    let again = from_canonical(&canonical).expect("own canonical form decodes");
    assert_eq!(again, expr);
    assert_eq!(to_canonical(&again), canonical);
});
