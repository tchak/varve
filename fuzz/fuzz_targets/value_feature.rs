#![no_main]
//! GeoJSON parsing never panics; the canonical form round-trips and
//! `Display` is the JCS text of it (§2.13 decision 3).

use libfuzzer_sys::fuzz_target;
use varve_core::canonical::canonical_bytes;
use varve_value::Feature;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let Ok(feature) = Feature::parse(text) else { return };
    let again = Feature::from_canonical(feature.to_canonical()).expect("own canonical form decodes");
    assert_eq!(again, feature);
    let jcs = canonical_bytes(feature.to_canonical()).expect("finite doubles");
    assert_eq!(feature.to_string().as_bytes(), jcs.as_slice());
    assert_eq!(Feature::parse(&feature.to_string()).unwrap(), feature);
});
