//! Property layer for diff/patch: `apply(a, diff(a, b)) == b` over
//! generated record states — including **depth 2** (nested `many`
//! groups), which backs the "depth-1 is a policy, not a type" claim
//! (§2.3) with evidence instead of prose.

use proptest::prelude::*;
use proptest::sample::subsequence;
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RowPath};
use varve_value::{
    AttachmentRef, CellAddr, CellState, CellValue, ItemsAddr, Op, RecordValues, Scalar, apply,
    cell_delta, diff,
};

const G1: &str = "g1";
const G2: &str = "g2";

fn seg(group: &str, item: &str) -> PathSeg {
    PathSeg {
        group: GroupId::new(group),
        item: ItemId::new(item),
    }
}

fn items(pool: &'static [&'static str]) -> impl Strategy<Value = Vec<ItemId>> {
    subsequence(pool.to_vec(), 0..=pool.len())
        .prop_shuffle()
        .prop_map(|v| v.into_iter().map(ItemId::new).collect())
}

fn attachment(id: &str, content: &str) -> Scalar {
    use varve_core::canonical::{CanonicalValue, hash_plain};
    Scalar::Attachment(Box::new(AttachmentRef {
        id: id.into(),
        hash: hash_plain(&CanonicalValue::String(content.into())).unwrap(),
        filename: format!("{id}.pdf"),
        content_type: "application/pdf".into(),
        byte_size: 1_000,
    }))
}

/// Elements that carry identity (§2.4): enums and attachments; a small
/// id alphabet so lists share, repeat and replace elements.
fn identified() -> impl Strategy<Value = Scalar> {
    prop_oneof![
        "[o][1-3]".prop_map(|s| Scalar::Enum(OptionId::new(s))),
        ("[f][1-3]", "[a-c]").prop_map(|(id, content)| attachment(&id, &content)),
    ]
}

/// One scalar of every kind: the value side of the nine `ScalarType`s
/// minus geometry (exercised in the wire and roundtrip suites).
fn scalar() -> impl Strategy<Value = Scalar> {
    prop_oneof![
        "[a-c]{0,2}".prop_map(Scalar::Text),
        any::<bool>().prop_map(Scalar::Boolean),
        any::<i16>().prop_map(|i| Scalar::Integer(i.into())),
        "-?[0-9]{1,4}(\\.[0-9]{1,3})?".prop_map(|s| Scalar::Decimal(Decimal::parse(&s).unwrap())),
        (2000i32..2030, 1u8..=12, 1u8..=28).prop_map(|(y, m, d)| Scalar::Date(
            Date::parse(&format!("{y:04}-{m:02}-{d:02}")).unwrap()
        )),
        (0u8..24, 0u8..60).prop_map(|(h, m)| Scalar::Datetime(
            Instant::parse(&format!("2026-08-18T{h:02}:{m:02}:00Z")).unwrap()
        )),
        identified(),
    ]
}

fn cell() -> impl Strategy<Value = CellState> {
    prop_oneof![
        Just(CellState::Empty),
        scalar().prop_map(|s| CellState::Value(CellValue::One(s))),
        // Never empty: a blank `many` cell is `Empty` (§2.4).
        proptest::collection::vec(scalar(), 1..=3)
            .prop_map(|v| CellState::Value(CellValue::Many(v))),
    ]
}

/// A random op over the fixed universe — most of them do not apply to
/// a given state, which is the point.
fn op() -> impl Strategy<Value = Op> {
    let column = || prop_oneof![Just("c_root"), Just("c_g1"), Just("c_g2"), Just("c_x")];
    let g1_item = || prop_oneof![Just("i1"), Just("i2"), Just("i3"), Just("i9")];
    let g2_item = || prop_oneof![Just("j1"), Just("j2"), Just("j9")];
    let path = || {
        prop_oneof![
            Just(RowPath::root()),
            g1_item().prop_map(|i| RowPath::root().child(seg(G1, i))),
            (g1_item(), g2_item())
                .prop_map(|(i, j)| RowPath::root().child(seg(G1, i)).child(seg(G2, j))),
        ]
    };
    let list_addr = || {
        prop_oneof![
            Just((GroupId::new(G1), RowPath::root())),
            g1_item().prop_map(|i| (GroupId::new(G2), RowPath::root().child(seg(G1, i)))),
        ]
    };
    let any_item = || prop_oneof![g1_item(), g2_item()];
    prop_oneof![
        (column(), path(), cell()).prop_map(|(c, path, state)| Op::Set {
            column: ColumnId::new(c),
            path,
            state,
        }),
        (column(), path()).prop_map(|(c, path)| Op::Unset {
            column: ColumnId::new(c),
            path
        }),
        (list_addr(), any_item(), 0usize..4).prop_map(|((group, parent), item, at)| Op::AddItem {
            group,
            parent,
            item: ItemId::new(item),
            at,
        }),
        (list_addr(), any_item()).prop_map(|((group, parent), item)| Op::RemoveItem {
            group,
            parent,
            item: ItemId::new(item),
        }),
        (list_addr(), proptest::collection::vec(any_item(), 0..4)).prop_map(
            |((group, parent), order)| Op::Reorder {
                group,
                parent,
                order: order.into_iter().map(ItemId::new).collect(),
            }
        ),
    ]
}

prop_compose! {
    /// A coherent record state over a fixed universe: root column,
    /// `g1` (many, root), `g2` (many, nested in g1), with cells at all
    /// three depths.
    fn record_values()(
        g1 in items(&["i1", "i2", "i3"]),
        nested in proptest::collection::btree_map(
            prop_oneof![Just("i1"), Just("i2"), Just("i3")],
            items(&["j1", "j2"]),
            0..=3,
        ),
        root_cell in proptest::option::of(cell()),
        g1_cells in proptest::collection::btree_map(
            prop_oneof![Just("i1"), Just("i2"), Just("i3")],
            cell(),
            0..=3,
        ),
        g2_cells in proptest::collection::btree_map(
            (
                prop_oneof![Just("i1"), Just("i2"), Just("i3")],
                prop_oneof![Just("j1"), Just("j2")],
            ),
            cell(),
            0..=6,
        ),
    ) -> RecordValues {
        let mut v = RecordValues::new();
        if !g1.is_empty() {
            v.items.insert(
                ItemsAddr { group: GroupId::new(G1), parent: RowPath::root() },
                g1.clone(),
            );
        }
        for i in &g1 {
            if let Some(js) = nested.get(i.as_str())
                && !js.is_empty()
            {
                v.items.insert(
                    ItemsAddr {
                        group: GroupId::new(G2),
                        parent: RowPath::root().child(seg(G1, i.as_str())),
                    },
                    js.clone(),
                );
            }
        }
        if let Some(state) = root_cell {
            v.cells.insert(
                CellAddr { column: ColumnId::new("c_root"), path: RowPath::root() },
                state,
            );
        }
        for (i, state) in g1_cells {
            if g1.contains(&ItemId::new(i)) {
                v.cells.insert(
                    CellAddr {
                        column: ColumnId::new("c_g1"),
                        path: RowPath::root().child(seg(G1, i)),
                    },
                    state,
                );
            }
        }
        for ((i, j), state) in g2_cells {
            let in_g1 = g1.contains(&ItemId::new(i));
            let in_g2 = nested
                .get(i)
                .is_some_and(|js| js.contains(&ItemId::new(j)));
            if in_g1 && in_g2 {
                v.cells.insert(
                    CellAddr {
                        column: ColumnId::new("c_g2"),
                        path: RowPath::root()
                            .child(seg(G1, i))
                            .child(seg(G2, j)),
                    },
                    state,
                );
            }
        }
        v
    }
}

proptest! {
    /// THE invariant of this crate.
    #[test]
    fn diff_then_apply_reproduces_target(
        a in record_values(),
        b in record_values(),
    ) {
        let ops = diff(&a, &b);
        let mut replay = a.clone();
        for op in &ops {
            prop_assert!(
                apply(&mut replay, op).is_ok(),
                "op failed to apply: {op:?}"
            );
        }
        prop_assert_eq!(replay, b);
    }

    #[test]
    fn diff_with_self_is_empty(a in record_values()) {
        prop_assert!(diff(&a, &a).is_empty());
    }

    /// §5: a snapshot export is a patch against the empty state and
    /// never needs destructive ops.
    #[test]
    fn export_from_empty_is_constructive(b in record_values()) {
        let ops = diff(&RecordValues::new(), &b);
        for op in &ops {
            prop_assert!(
                !matches!(op, Op::Unset { .. } | Op::RemoveItem { .. }),
                "destructive op in a from-empty export: {op:?}"
            );
        }
        let mut replay = RecordValues::new();
        for op in &ops {
            apply(&mut replay, op).unwrap();
        }
        prop_assert_eq!(replay, b);
    }

    /// A refused op leaves the values exactly as they were — the
    /// documented `apply` contract, over every op kind and error path.
    #[test]
    fn a_refused_op_changes_nothing(a in record_values(), op in op()) {
        let mut v = a.clone();
        if apply(&mut v, &op).is_err() {
            prop_assert_eq!(v, a, "refused op mutated the values: {:?}", op);
        }
    }

    /// Going there and back again: `diff(a, b)` then `diff(b, a)`
    /// returns to `a` exactly.
    #[test]
    fn diff_round_trip_returns_to_the_origin(
        a in record_values(),
        b in record_values(),
    ) {
        let mut v = a.clone();
        for op in diff(&a, &b) {
            apply(&mut v, &op).unwrap();
        }
        prop_assert_eq!(&v, &b);
        for op in diff(&b, &a) {
            apply(&mut v, &op).unwrap();
        }
        prop_assert_eq!(v, a);
    }

    /// §2.4 element-wise reporting: over identified elements the delta
    /// partitions the new ids into added / changed / unchanged and the
    /// removed ids are exactly old \ new; nothing is unidentified.
    #[test]
    fn cell_delta_partitions_identified_elements(
        old in proptest::collection::vec(identified(), 1..=4),
        new in proptest::collection::vec(identified(), 1..=4),
    ) {
        use std::collections::BTreeSet;
        let ids = |list: &[Scalar]| -> BTreeSet<String> {
            list.iter().map(|s| s.element_id().unwrap().to_string()).collect()
        };
        let (old_ids, new_ids) = (ids(&old), ids(&new));
        let delta = cell_delta(&CellValue::Many(old.clone()), &CellValue::Many(new.clone())).unwrap();
        prop_assert!(!delta.unidentified);
        let added: BTreeSet<String> = delta.added.iter().cloned().collect();
        let changed: BTreeSet<String> = delta.changed.iter().cloned().collect();
        let removed: BTreeSet<String> = delta.removed.iter().cloned().collect();
        prop_assert!(added.is_subset(&new_ids));
        prop_assert!(added.is_disjoint(&old_ids));
        prop_assert!(changed.is_subset(&new_ids));
        prop_assert!(changed.is_subset(&old_ids));
        prop_assert_eq!(&removed, &old_ids.difference(&new_ids).cloned().collect::<BTreeSet<_>>());
        // Ids in both lists are either changed or unchanged — and
        // changed iff some new element carrying that id differs from
        // the old element of that id (the last one, if repeated).
        for id in old_ids.intersection(&new_ids) {
            let previous = old.iter().rev().find(|s| s.element_id() == Some(id)).unwrap();
            let differs = new.iter().filter(|s| s.element_id() == Some(id)).any(|n| n != previous);
            prop_assert_eq!(changed.contains(id), differs, "id {}", id);
        }
        // Delta with itself is empty — for a well-formed cell, i.e. one
        // without repeated identities (conformance rejects those as
        // `DuplicateElement`; the report is not defined over them).
        if new_ids.len() == new.len() {
            let same = cell_delta(&CellValue::Many(new.clone()), &CellValue::Many(new.clone())).unwrap();
            prop_assert!(same.added.is_empty() && same.changed.is_empty() && same.removed.is_empty());
        }
    }
}
