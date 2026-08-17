#![no_main]
//! The reader is total over arbitrary bytes, and what it accepts
//! re-emits stably: write ∘ read ∘ write == write ∘ read (M3).

use libfuzzer_sys::fuzz_target;
use varve_wire::{read_stream, write_lines};

fuzz_target!(|data: &[u8]| {
    let Ok(stream) = read_stream(data) else { return };
    let bytes = write_lines(&stream.lines).expect("accepted streams are writable");
    let again = read_stream(&bytes).expect("own output reads back");
    assert_eq!(write_lines(&again.lines).unwrap(), bytes, "write ∘ read is not a fixpoint");
    assert_eq!(again.lines, stream.lines);
});
