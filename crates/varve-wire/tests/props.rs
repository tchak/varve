//! Wire laws: read ∘ write is identity over generated streams (M3
//! byte-stability), and the reader is total over arbitrary bytes — the
//! property fuzzing will later hammer at scale. Generators live in
//! `common` and cover every scalar kind (§2.13): text, boolean, full
//! i64, fractional decimals, dates and instants across the year range,
//! enums, attachments and geometry.

mod common;

use proptest::prelude::*;
use varve_core::RecordId;
use varve_wire::{Intent, Line, Mode, SnapshotRecord, read_stream, snapshot_records, write_lines};

fn snapshot_stream() -> impl Strategy<Value = (Vec<SnapshotRecord>, Vec<Line>)> {
    proptest::collection::vec(("[a-z0-9]{1,6}", common::record_values()), 0..4).prop_map(
        |records| {
            // Distinct record ids: dedup by id. The lens travels in-stream
            // as a revision line (§5), and its id is its content hash.
            let lens = common::lens();
            let mut seen = std::collections::BTreeSet::new();
            let mut logical = Vec::new();
            let mut lines = vec![common::revision_line()];
            for (id, values) in records {
                if seen.insert(id.clone()) {
                    let rec = SnapshotRecord {
                        record: RecordId::new(id),
                        lens: lens.clone(),
                        values,
                    };
                    lines.extend(rec.lines());
                    logical.push(rec);
                }
            }
            let header = Line::Header(common::manifest(
                Mode::Snapshot,
                Intent::Upsert,
                seen.len() as u64,
            ));
            (logical, std::iter::once(header).chain(lines).collect())
        },
    )
}

proptest! {
    /// M3: read ∘ write is identity, and the bytes are stable.
    #[test]
    fn snapshot_streams_round_trip((records, lines) in snapshot_stream()) {
        let bytes = write_lines(&lines).unwrap();
        let stream = read_stream(&bytes).unwrap();
        prop_assert_eq!(&stream.lines, &lines);
        prop_assert_eq!(write_lines(&stream.lines).unwrap(), bytes);
        // Explode ∘ reassemble is identity on whole records too.
        prop_assert_eq!(snapshot_records(&stream), records);
    }

    /// The reader never panics on arbitrary input.
    #[test]
    fn reader_is_total(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = read_stream(&bytes);
    }

    /// Nor on arbitrary *text* shaped like lines.
    #[test]
    fn reader_is_total_over_text(s in "\\PC{0,200}") {
        let _ = read_stream(s.as_bytes());
    }
}
