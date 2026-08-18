//! Generators shared by the wire property suites: every scalar kind the
//! canonical form carries (§2.13), coherent record states at depth 2,
//! and the manifest/lens boilerplate a stream needs (§5).

#![allow(dead_code)]

use proptest::prelude::*;
use proptest::sample::subsequence;
use varve_core::canonical::{CanonicalValue, hash_plain};
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RevisionId, RowPath};
use varve_schema::{Schema, revision_id};
use varve_value::{
    AttachmentRef, CellAddr, CellState, CellValue, Feature, ItemsAddr, RecordValues, Scalar,
};
use varve_wire::{Intent, Line, Manifest, Mode};

/// Geometry with the numbers JCS rendering has to get right: negative
/// zero, integral doubles, large and tiny exponents, numeric ids,
/// property numbers.
pub fn geometry() -> impl Strategy<Value = Scalar> {
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

/// A calendar date anywhere in the four-digit year range (§2.13).
pub fn date() -> impl Strategy<Value = Date> {
    (0i32..=9999, 1u8..=12, 1u8..=28)
        .prop_map(|(y, m, d)| Date::parse(&format!("{y:04}-{m:02}-{d:02}")).unwrap())
}

/// An instant spelled with a fraction and an offset — the stored value
/// normalizes to the `Z` form (§5). Years stay one off each edge so a
/// large offset never pushes the UTC date out of range.
pub fn instant() -> impl Strategy<Value = Instant> {
    (
        1i32..=9998,
        1u8..=12,
        1u8..=28,
        0u8..24,
        0u8..60,
        0u8..60,
        proptest::option::of("[0-9]{1,9}"),
        prop_oneof![
            Just("Z".to_string()),
            (any::<bool>(), 0u8..24, 0u8..60)
                .prop_map(|(neg, h, m)| format!("{}{h:02}:{m:02}", if neg { '-' } else { '+' })),
        ],
    )
        .prop_map(|(y, mo, d, h, mi, s, frac, offset)| {
            let frac = frac.map(|f| format!(".{f}")).unwrap_or_default();
            Instant::parse(&format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}{frac}{offset}"))
                .unwrap()
        })
}

/// Fractional and negative decimals, normalized by `parse`.
pub fn decimal() -> impl Strategy<Value = Decimal> {
    "-?[0-9]{1,18}(\\.[0-9]{1,6})?".prop_map(|s| Decimal::parse(&s).unwrap())
}

/// A content-addressed attachment claim (§2.15): unicode filename, a
/// byte size anywhere up to the JCS-safe bound.
pub fn attachment() -> impl Strategy<Value = Scalar> {
    (
        "[a-z0-9]{1,6}",
        "\\PC{0,12}",
        "\\PC{0,12}",
        prop_oneof![
            Just("application/pdf".to_string()),
            Just("image/png".to_string()),
            "[a-z]{1,8}/[a-z.+-]{1,10}",
        ],
        0u64..=varve_core::canonical::MAX_SAFE_INTEGER as u64,
    )
        .prop_map(|(id, content, filename, content_type, byte_size)| {
            Scalar::Attachment(Box::new(AttachmentRef {
                id,
                hash: hash_plain(&CanonicalValue::String(content)).unwrap(),
                filename,
                content_type,
                byte_size,
            }))
        })
}

/// Every scalar kind (§2.13 decision 3): exact numbers and instants as
/// normalized strings, geometry as its canonical JSON value.
pub fn scalar() -> impl Strategy<Value = Scalar> {
    prop_oneof![
        "\\PC{0,8}".prop_map(Scalar::Text),
        any::<bool>().prop_map(Scalar::Boolean),
        // Full i64 range: exact integers travel as strings (§2.13).
        any::<i64>().prop_map(Scalar::Integer),
        decimal().prop_map(Scalar::Decimal),
        date().prop_map(Scalar::Date),
        instant().prop_map(Scalar::Datetime),
        "[a-z0-9]{1,5}".prop_map(|s| Scalar::Enum(OptionId::new(s))),
        attachment(),
        geometry(),
    ]
}

pub fn state() -> impl Strategy<Value = CellState> {
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
pub fn record_values() -> impl Strategy<Value = RecordValues> {
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

fn seg(group: &str, item: &str) -> PathSeg {
    PathSeg { group: GroupId::new(group), item: ItemId::new(item) }
}

fn items(pool: &'static [&'static str]) -> impl Strategy<Value = Vec<ItemId>> {
    subsequence(pool.to_vec(), 0..=pool.len())
        .prop_shuffle()
        .prop_map(|v| v.into_iter().map(ItemId::new).collect())
}

/// A record state over a **fixed, small universe** — items drawn from
/// `{i1,i2,i3}` (shuffled) with `{j1,j2}` nested under each, columns
/// from `{a,b,c}` at every depth — so that two successive states share
/// ids and `diff` between them emits every op kind: `Unset`,
/// `RemoveItem` and `Reorder` as well as `Set` and `AddItem`.
pub fn shared_universe_values() -> impl Strategy<Value = RecordValues> {
    (
        items(&["i1", "i2", "i3"]),
        proptest::collection::btree_map(
            prop_oneof![Just("i1"), Just("i2"), Just("i3")],
            items(&["j1", "j2"]),
            0..=3,
        ),
        proptest::collection::btree_map(prop_oneof![Just("a"), Just("b"), Just("c")], state(), 0..=3),
        proptest::collection::btree_map(
            (prop_oneof![Just("a"), Just("b")], prop_oneof![Just("i1"), Just("i2"), Just("i3")]),
            state(),
            0..=4,
        ),
        proptest::collection::btree_map(
            (
                prop_oneof![Just("a"), Just("b")],
                prop_oneof![Just("i1"), Just("i2"), Just("i3")],
                prop_oneof![Just("j1"), Just("j2")],
            ),
            state(),
            0..=4,
        ),
    )
        .prop_map(|(g1, nested, root_cells, g1_cells, g2_cells)| {
            let mut v = RecordValues::new();
            if !g1.is_empty() {
                v.items.insert(ItemsAddr { group: GroupId::new("g1"), parent: RowPath::root() }, g1.clone());
            }
            for i in &g1 {
                if let Some(js) = nested.get(i.as_str())
                    && !js.is_empty()
                {
                    v.items.insert(
                        ItemsAddr {
                            group: GroupId::new("g2"),
                            parent: RowPath::root().child(seg("g1", i.as_str())),
                        },
                        js.clone(),
                    );
                }
            }
            for (c, state) in root_cells {
                v.cells.insert(CellAddr { column: ColumnId::new(c), path: RowPath::root() }, state);
            }
            for ((c, i), state) in g1_cells {
                if g1.contains(&ItemId::new(i)) {
                    v.cells.insert(
                        CellAddr { column: ColumnId::new(c), path: RowPath::root().child(seg("g1", i)) },
                        state,
                    );
                }
            }
            for ((c, i, j), state) in g2_cells {
                let in_g1 = g1.contains(&ItemId::new(i));
                let in_g2 = nested.get(i).is_some_and(|js| js.contains(&ItemId::new(j)));
                if in_g1 && in_g2 {
                    v.cells.insert(
                        CellAddr {
                            column: ColumnId::new(c),
                            path: RowPath::root().child(seg("g1", i)).child(seg("g2", j)),
                        },
                        state,
                    );
                }
            }
            v
        })
}

/// The lens every generated stream reads through: the empty schema,
/// whose id is its content hash (§2.13).
pub fn lens() -> RevisionId {
    revision_id(&Schema::default())
}

pub fn revision_line() -> Line {
    Line::Revision { id: lens(), schema: Schema::default() }
}

pub fn manifest(mode: Mode, intent: Intent, record_count: u64) -> Manifest {
    Manifest {
        format_version: varve_wire::FORMAT_VERSION,
        source_instance: "gen".into(),
        mode,
        intent,
        revisions: vec![lens()],
        record_count,
        attachments_bundled: false,
    }
}
