//! Wire laws: read ∘ write is identity over generated streams (M3
//! byte-stability), and the reader is total over arbitrary bytes — the
//! property fuzzing will later hammer at scale.

use proptest::prelude::*;
use varve_core::primitives::Decimal;
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RecordId, RowPath};
use varve_value::{CellAddr, CellState, CellValue, Feature, ItemsAddr, RecordValues, Scalar};
use varve_wire::{Intent, Line, Manifest, Mode, SnapshotRecord, read_stream, snapshot_records, write_lines};

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
        // Never empty: a blank `many` cell is `Empty` (§2.4).
        proptest::collection::vec(scalar(), 1..3)
            .prop_map(|v| CellState::Value(CellValue::Many(v))),
    ]
}

/// A coherent record: root cells; a `many` group `g1` with items and
/// cells; a nested `many` group `g2` under each `g1` item (depth 2 — the
/// wire is depth-N ready even though the policy is depth 1). Item lists
/// are never empty and every cell sits on an existing item (§2.4).
fn record_values() -> impl Strategy<Value = RecordValues> {
    let ids = || proptest::collection::btree_set("[a-z0-9]{1,3}", 1..3);
    (
        proptest::collection::btree_map("[a-z]{1,4}", state(), 0..4),
        proptest::option::of((
            ids(),
            proptest::collection::btree_map(("[a-z]{1,3}", "[a-z0-9]{1,3}"), state(), 0..4),
            proptest::option::of((ids(), proptest::collection::btree_map("[a-z]{1,3}", state(), 0..3))),
        )),
    )
        .prop_map(|(root, g1)| {
            let mut v = RecordValues::new();
            for (column, state) in root {
                v.cells.insert(CellAddr { column: ColumnId::new(column), path: RowPath::root() }, state);
            }
            let Some((items, item_cells, nested)) = g1 else { return v };
            let g1 = GroupId::new("g1");
            v.items.insert(
                ItemsAddr { group: g1.clone(), parent: RowPath::root() },
                items.iter().map(ItemId::new).collect(),
            );
            for ((column, item), state) in item_cells {
                // Only cells on items that exist.
                if !items.contains(&item) {
                    continue;
                }
                let path = RowPath::root().child(PathSeg { group: g1.clone(), item: ItemId::new(item) });
                v.cells.insert(CellAddr { column: ColumnId::new(column), path }, state);
            }
            if let Some((sub_ids, sub_cells)) = nested {
                let first = ItemId::new(items.iter().next().expect("non-empty"));
                let parent = RowPath::root().child(PathSeg { group: g1.clone(), item: first });
                let g2 = GroupId::new("g2");
                v.items.insert(
                    ItemsAddr { group: g2.clone(), parent: parent.clone() },
                    sub_ids.iter().map(ItemId::new).collect(),
                );
                let sub = ItemId::new(sub_ids.iter().next().expect("non-empty"));
                let path = parent.child(PathSeg { group: g2, item: sub });
                for (column, state) in sub_cells {
                    v.cells.insert(CellAddr { column: ColumnId::new(column), path: path.clone() }, state);
                }
            }
            v
        })
}

fn snapshot_stream() -> impl Strategy<Value = (Vec<SnapshotRecord>, Vec<Line>)> {
    proptest::collection::vec(("[a-z0-9]{1,6}", record_values()), 0..4).prop_map(|records| {
        // Distinct record ids: dedup by id.
        // The lens travels in-stream as a revision line (§5), and its
        // id is its content hash.
        let schema = varve_schema::Schema::default();
        let lens = varve_schema::revision_id(&schema);
        let mut seen = std::collections::BTreeSet::new();
        let mut logical = Vec::new();
        let mut lines = vec![Line::Revision { id: lens.clone(), schema }];
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
        let header = Line::Header(Manifest {
            format_version: varve_wire::FORMAT_VERSION,
            source_instance: "gen".into(),
            mode: Mode::Snapshot,
            intent: Intent::Upsert,
            revisions: vec![lens],
            record_count: seen.len() as u64,
            attachments_bundled: false,
        });
        (logical, std::iter::once(header).chain(lines).collect())
    })
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
