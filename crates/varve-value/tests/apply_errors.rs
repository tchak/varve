//! The `apply` contract, edge by edge: every `ApplyError` variant, each
//! leaving the values untouched ("errors leave `values` unchanged"),
//! and the positive edges of the five ops (§5, §2.4).

use varve_core::{ColumnId, GroupId, ItemId, PathSeg, RowPath};
use varve_value::{
    ApplyError, CellAddr, CellState, CellValue, ItemsAddr, Op, RecordValues, Scalar, apply,
};

fn g(id: &str) -> GroupId {
    GroupId::new(id)
}

fn i(id: &str) -> ItemId {
    ItemId::new(id)
}

fn seg(group: &str, item: &str) -> PathSeg {
    PathSeg {
        group: g(group),
        item: i(item),
    }
}

fn text(s: &str) -> CellState {
    CellState::Value(CellValue::One(Scalar::Text(s.into())))
}

fn set(column: &str, path: RowPath, state: CellState) -> Op {
    Op::Set {
        column: ColumnId::new(column),
        path,
        state,
    }
}

fn add(group: &str, parent: RowPath, item: &str, at: usize) -> Op {
    Op::AddItem {
        group: g(group),
        parent,
        item: i(item),
        at,
    }
}

fn remove(group: &str, parent: RowPath, item: &str) -> Op {
    Op::RemoveItem {
        group: g(group),
        parent,
        item: i(item),
    }
}

fn reorder(group: &str, parent: RowPath, order: &[&str]) -> Op {
    Op::Reorder {
        group: g(group),
        parent,
        order: order.iter().map(|s| i(s)).collect(),
    }
}

fn items(group: &str, parent: RowPath) -> ItemsAddr {
    ItemsAddr {
        group: g(group),
        parent,
    }
}

fn cell(column: &str, path: RowPath) -> CellAddr {
    CellAddr {
        column: ColumnId::new(column),
        path,
    }
}

/// Root cell `name`; `g1` with items i1, i2 (cell `c1` on each); `g2`
/// nested under g1/i1 with item j1 (cell `c2` on it).
fn sample() -> RecordValues {
    let mut v = RecordValues::new();
    v.cells
        .insert(cell("name", RowPath::root()), text("Dupont"));
    v.items
        .insert(items("g1", RowPath::root()), vec![i("i1"), i("i2")]);
    for item in ["i1", "i2"] {
        v.cells.insert(
            cell("c1", RowPath::root().child(seg("g1", item))),
            text(item),
        );
    }
    let i1 = RowPath::root().child(seg("g1", "i1"));
    v.items.insert(items("g2", i1.clone()), vec![i("j1")]);
    v.cells
        .insert(cell("c2", i1.child(seg("g2", "j1"))), text("nested"));
    v
}

/// Apply `op` expecting `err`; the values must be exactly as before.
fn refuse(op: Op, err: ApplyError) {
    let before = sample();
    let mut v = before.clone();
    assert_eq!(apply(&mut v, &op), Err(err), "{op:?}");
    assert_eq!(
        v, before,
        "a refused op must leave the values unchanged: {op:?}"
    );
}

#[test]
fn item_exists() {
    for at in [0, 1, 2] {
        refuse(
            add("g1", RowPath::root(), "i1", at),
            ApplyError::ItemExists(g("g1"), i("i1")),
        );
        refuse(
            add("g1", RowPath::root(), "i2", at),
            ApplyError::ItemExists(g("g1"), i("i2")),
        );
    }
}

#[test]
fn unknown_item() {
    // No list at all for the group at that scope.
    refuse(
        remove("g9", RowPath::root(), "i1"),
        ApplyError::UnknownItem(g("g9"), i("i1")),
    );
    refuse(
        remove("g2", RowPath::root().child(seg("g1", "i2")), "j1"),
        ApplyError::UnknownItem(g("g2"), i("j1")),
    );
    // A list exists, the item is not in it.
    refuse(
        remove("g1", RowPath::root(), "ghost"),
        ApplyError::UnknownItem(g("g1"), i("ghost")),
    );
}

#[test]
fn bad_index() {
    // Two items: 0, 1 and 2 (append) are fine; 3 is out of bounds.
    refuse(
        add("g1", RowPath::root(), "i3", 3),
        ApplyError::BadIndex(g("g1"), 3),
    );
    refuse(
        add("g1", RowPath::root(), "i3", usize::MAX),
        ApplyError::BadIndex(g("g1"), usize::MAX),
    );
    // A group with no list yet: only 0 is a valid index.
    refuse(
        add("g9", RowPath::root(), "x", 1),
        ApplyError::BadIndex(g("g9"), 1),
    );
}

#[test]
fn bad_reorder() {
    // No list to reorder.
    refuse(
        reorder("g9", RowPath::root(), &["a"]),
        ApplyError::BadReorder(g("g9")),
    );
    refuse(
        reorder("g9", RowPath::root(), &[]),
        ApplyError::BadReorder(g("g9")),
    );
    // Different length: shorter and longer.
    refuse(
        reorder("g1", RowPath::root(), &["i1"]),
        ApplyError::BadReorder(g("g1")),
    );
    refuse(
        reorder("g1", RowPath::root(), &["i2", "i1", "i3"]),
        ApplyError::BadReorder(g("g1")),
    );
    // Same length, different elements.
    refuse(
        reorder("g1", RowPath::root(), &["i1", "i3"]),
        ApplyError::BadReorder(g("g1")),
    );
    // Repeated element.
    refuse(
        reorder("g1", RowPath::root(), &["i1", "i1"]),
        ApplyError::BadReorder(g("g1")),
    );
}

#[test]
fn empty_list() {
    // §2.4: a blank `many` cell is `Empty`, never a zero-length list.
    refuse(
        set(
            "tags",
            RowPath::root(),
            CellState::Value(CellValue::Many(vec![])),
        ),
        ApplyError::EmptyList(ColumnId::new("tags")),
    );
    // Even over an existing cell: the previous value survives.
    refuse(
        set(
            "name",
            RowPath::root(),
            CellState::Value(CellValue::Many(vec![])),
        ),
        ApplyError::EmptyList(ColumnId::new("name")),
    );
}

#[test]
fn add_item_at_len_appends() {
    let mut v = sample();
    apply(&mut v, &add("g1", RowPath::root(), "i3", 2)).unwrap();
    assert_eq!(
        v.items[&items("g1", RowPath::root())],
        vec![i("i1"), i("i2"), i("i3")]
    );
    apply(&mut v, &add("g1", RowPath::root(), "i0", 0)).unwrap();
    assert_eq!(
        v.items[&items("g1", RowPath::root())],
        vec![i("i0"), i("i1"), i("i2"), i("i3")]
    );
    // First item of a new group at index 0 creates the list.
    apply(&mut v, &add("g9", RowPath::root(), "x", 0)).unwrap();
    assert_eq!(v.items[&items("g9", RowPath::root())], vec![i("x")]);
}

#[test]
fn removing_the_last_item_removes_the_list() {
    // §2.4 one state, one encoding: no `[]` is ever stored.
    let mut v = sample();
    let i1 = RowPath::root().child(seg("g1", "i1"));
    apply(&mut v, &remove("g2", i1.clone(), "j1")).unwrap();
    assert!(!v.items.contains_key(&items("g2", i1.clone())));
    assert!(!v.cells.contains_key(&cell("c2", i1.child(seg("g2", "j1")))));
    // g1 itself is untouched.
    assert_eq!(
        v.items[&items("g1", RowPath::root())],
        vec![i("i1"), i("i2")]
    );
}

#[test]
fn remove_item_cascades_through_nested_lists() {
    // Removing g1/i1 takes its cells, the nested g2 list and g2's cells.
    let mut v = sample();
    apply(&mut v, &remove("g1", RowPath::root(), "i1")).unwrap();
    let i1 = RowPath::root().child(seg("g1", "i1"));
    assert_eq!(v.items[&items("g1", RowPath::root())], vec![i("i2")]);
    assert!(!v.items.contains_key(&items("g2", i1.clone())));
    assert!(!v.cells.contains_key(&cell("c1", i1.clone())));
    assert!(!v.cells.contains_key(&cell("c2", i1.child(seg("g2", "j1")))));
    // Sibling and root cells survive.
    assert!(
        v.cells
            .contains_key(&cell("c1", RowPath::root().child(seg("g1", "i2"))))
    );
    assert!(v.cells.contains_key(&cell("name", RowPath::root())));
    // Nothing else remains: exactly root + i2.
    assert_eq!(v.cells.len(), 2);
    assert_eq!(v.items.len(), 1);
}

#[test]
fn unset_of_an_absent_cell_is_a_no_op() {
    let before = sample();
    let mut v = before.clone();
    apply(
        &mut v,
        &Op::Unset {
            column: ColumnId::new("nope"),
            path: RowPath::root(),
        },
    )
    .unwrap();
    assert_eq!(v, before);
    // And unset of a present cell removes it (back to absent, not Empty).
    apply(
        &mut v,
        &Op::Unset {
            column: ColumnId::new("name"),
            path: RowPath::root(),
        },
    )
    .unwrap();
    assert!(!v.cells.contains_key(&cell("name", RowPath::root())));
}

#[test]
fn set_overwrites_and_reorder_to_the_same_order_is_ok() {
    let mut v = sample();
    apply(&mut v, &set("name", RowPath::root(), text("Durand"))).unwrap();
    assert_eq!(v.cells[&cell("name", RowPath::root())], text("Durand"));
    apply(&mut v, &set("name", RowPath::root(), CellState::Empty)).unwrap();
    assert_eq!(v.cells[&cell("name", RowPath::root())], CellState::Empty);

    let before = v.clone();
    apply(&mut v, &reorder("g1", RowPath::root(), &["i1", "i2"])).unwrap();
    assert_eq!(v, before);
    apply(&mut v, &reorder("g1", RowPath::root(), &["i2", "i1"])).unwrap();
    assert_eq!(
        v.items[&items("g1", RowPath::root())],
        vec![i("i2"), i("i1")]
    );
    // Reorder does not touch cells.
    assert_eq!(v.cells, before.cells);
}
