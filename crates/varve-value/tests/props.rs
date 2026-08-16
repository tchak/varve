//! Property layer for diff/patch: `apply(a, diff(a, b)) == b` over
//! generated record states — including **depth 2** (nested `many`
//! groups), which backs the "depth-1 is a policy, not a type" claim
//! (§2.3) with evidence instead of prose.

use proptest::prelude::*;
use proptest::sample::subsequence;
use varve_core::{ColumnId, GroupId, ItemId, PathSeg, RowPath};
use varve_value::{
    CellAddr, CellState, CellValue, ItemsAddr, Op, RecordValues, Scalar, apply,
    diff,
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

fn cell() -> impl Strategy<Value = CellState> {
    prop_oneof![
        Just(CellState::Empty),
        "[a-c]{0,2}".prop_map(|s| CellState::Value(CellValue::One(Scalar::Text(s)))),
        any::<i16>().prop_map(|i| CellState::Value(CellValue::One(
            Scalar::Integer(i.into())
        ))),
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
}
