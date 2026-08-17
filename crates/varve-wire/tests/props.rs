//! Wire laws: read ∘ write is identity over generated streams (M3
//! byte-stability), and the reader is total over arbitrary bytes — the
//! property fuzzing will later hammer at scale.

use proptest::prelude::*;
use varve_core::primitives::Decimal;
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RecordId, RevisionId, RowPath};
use varve_value::{CellAddr, CellState, CellValue, Feature, ItemsAddr, RecordValues, Scalar};
use varve_wire::{Intent, Line, Manifest, Mode, RecordLine, read_stream, write_lines};

/// Geometry with the numbers JCS rendering has to get right: negative
/// zero, integral doubles, large and tiny exponents, numeric ids,
/// property numbers.
fn geometry() -> impl Strategy<Value = Scalar> {
    (any::<f64>(), any::<f64>(), any::<i32>(), any::<bool>()).prop_map(|(x, y, id, props)| {
        let finite = |f: f64| if f.is_finite() { f } else { 0.5 };
        let text = format!(
            r#"{{"type":"Feature","id":{id},"geometry":{{"type":"Point","coordinates":[{},{}]}},"properties":{}}}"#,
            finite(x),
            finite(y),
            if props { r#"{"n":-0.0,"m":1e300,"k":1.5e-7}"# } else { "null" }
        );
        Scalar::Geometry(Box::new(Feature::parse(&text).unwrap()))
    })
}

fn scalar() -> impl Strategy<Value = Scalar> {
    prop_oneof![
        "\\PC{0,8}".prop_map(Scalar::Text),
        any::<bool>().prop_map(Scalar::Boolean),
        // Full i64 range: exact integers travel as strings (§2.13).
        any::<i64>().prop_map(Scalar::Integer),
        any::<i32>().prop_map(|n| Scalar::Decimal(Decimal::from_i64(n.into()))),
        "[a-z0-9]{1,5}".prop_map(|s| Scalar::Enum(OptionId::new(s))),
        geometry(),
    ]
}

fn state() -> impl Strategy<Value = CellState> {
    prop_oneof![
        Just(CellState::Empty),
        scalar().prop_map(|s| CellState::Value(CellValue::One(s))),
        proptest::collection::vec(scalar(), 0..3)
            .prop_map(|v| CellState::Value(CellValue::Many(v))),
    ]
}

fn path() -> impl Strategy<Value = RowPath> {
    proptest::collection::vec(("[a-z]{1,3}", "[a-z0-9]{1,3}"), 0..3).prop_map(|segs| {
        segs.into_iter().fold(RowPath::root(), |p, (g, i)| {
            p.child(PathSeg { group: GroupId::new(g), item: ItemId::new(i) })
        })
    })
}

fn record_values() -> impl Strategy<Value = RecordValues> {
    (
        proptest::collection::btree_map(("[a-z]{1,4}", path()), state(), 0..5),
        proptest::collection::btree_map(
            ("[a-z]{1,3}", path()),
            proptest::collection::vec("[a-z0-9]{1,3}", 0..3),
            0..3,
        ),
    )
        .prop_map(|(cells, items)| {
            let mut v = RecordValues::new();
            for ((column, path), state) in cells {
                v.cells.insert(CellAddr { column: ColumnId::new(column), path }, state);
            }
            for ((group, parent), list) in items {
                v.items.insert(
                    ItemsAddr { group: GroupId::new(group), parent },
                    list.into_iter().map(ItemId::new).collect(),
                );
            }
            v
        })
}

fn snapshot_stream() -> impl Strategy<Value = Vec<Line>> {
    proptest::collection::vec(("[a-z0-9]{1,6}", record_values()), 0..4).prop_map(|records| {
        // Distinct record ids: dedup by id.
        let mut seen = std::collections::BTreeSet::new();
        let mut lines = Vec::new();
        for (id, values) in records {
            if seen.insert(id.clone()) {
                lines.push(Line::Record(RecordLine {
                    record: RecordId::new(id),
                    lens: RevisionId::new("lens"),
                    values,
                }));
            }
        }
        let header = Line::Header(Manifest {
            format_version: varve_wire::FORMAT_VERSION,
            source_instance: "gen".into(),
            mode: Mode::Snapshot,
            intent: Intent::Upsert,
            revisions: vec![RevisionId::new("lens")],
            record_count: seen.len() as u64,
            attachments_bundled: false,
        });
        std::iter::once(header).chain(lines).collect()
    })
}

proptest! {
    /// M3: read ∘ write is identity, and the bytes are stable.
    #[test]
    fn snapshot_streams_round_trip(lines in snapshot_stream()) {
        let bytes = write_lines(&lines).unwrap();
        let stream = read_stream(&bytes).unwrap();
        prop_assert_eq!(&stream.lines, &lines);
        prop_assert_eq!(write_lines(&stream.lines).unwrap(), bytes);
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
