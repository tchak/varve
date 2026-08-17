use varve_core::primitives::{Decimal, Instant};
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RowPath};
use varve_projection::{ColumnStatus, project};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, NomenclatureRef, OptionRow,
    ScalarType, Schema,
};
use varve_value::{CellAddr, CellState, CellValue, ItemsAddr, RecordValues, Scalar};

fn column(id: &str, ty: ScalarType, arity: Arity) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity,
    })
}

fn schema(elements: Vec<Element>) -> Schema {
    Schema {
        root: elements,
        resolvers: vec![],
    }
}

fn options(pairs: &[(&str, &str)]) -> NomenclatureRef {
    NomenclatureRef::Inline(
        pairs
            .iter()
            .map(|(id, label)| OptionRow {
                id: OptionId::new(*id),
                label: (*label).to_string(),
                fields: vec![],
            })
            .collect(),
    )
}

fn addr(column: &str) -> CellAddr {
    CellAddr {
        column: ColumnId::new(column),
        path: RowPath::root(),
    }
}

fn one(scalar: Scalar) -> CellState {
    CellState::Value(CellValue::One(scalar))
}

#[test]
fn identity_projection_is_a_no_op_with_a_clean_report() {
    let s = schema(vec![column("name", ScalarType::Text, Arity::One)]);
    let mut v = RecordValues::new();
    v.cells
        .insert(addr("name"), one(Scalar::Text("Dupont".into())));

    let p = project(&v, &s, &s, &Default::default()).unwrap();
    assert_eq!(p.values, v);
    assert!(p.report.is_clean());
    assert_eq!(
        p.report.columns[&ColumnId::new("name")].status,
        ColumnStatus::Identity
    );
}

#[test]
fn added_and_removed_columns_are_free() {
    let writer = schema(vec![
        column("a", ScalarType::Text, Arity::One),
        column("gone", ScalarType::Text, Arity::One),
    ]);
    let reader = schema(vec![
        column("a", ScalarType::Text, Arity::One),
        column("new", ScalarType::Text, Arity::One),
    ]);
    let mut v = RecordValues::new();
    v.cells.insert(addr("a"), one(Scalar::Text("x".into())));
    v.cells.insert(addr("gone"), one(Scalar::Text("y".into())));

    let p = project(&v, &writer, &reader, &Default::default()).unwrap();
    assert!(!p.values.cells.contains_key(&addr("gone")));
    assert_eq!(
        p.report.columns[&ColumnId::new("new")].status,
        ColumnStatus::AddedAbsent
    );
    assert_eq!(p.report.ignored_writer_columns, vec![ColumnId::new("gone")]);
    assert!(p.report.is_clean());
}

#[test]
fn widening_casts_convert_values() {
    let writer = schema(vec![
        column("n", ScalarType::Integer(None), Arity::One),
        column("d", ScalarType::Date, Arity::One),
        column("choice", ScalarType::Enum(options(&[("o1", "Oui")])), Arity::One),
    ]);
    let reader = schema(vec![
        column("n", ScalarType::Decimal(None), Arity::One),
        column("d", ScalarType::Datetime, Arity::One),
        column("choice", ScalarType::Text, Arity::One),
    ]);
    let mut v = RecordValues::new();
    v.cells.insert(addr("n"), one(Scalar::Integer(42)));
    v.cells.insert(
        addr("d"),
        one(Scalar::Date(
            varve_core::primitives::Date::parse("2026-08-16").unwrap(),
        )),
    );
    v.cells
        .insert(addr("choice"), one(Scalar::Enum(OptionId::new("o1"))));

    let p = project(&v, &writer, &reader, &Default::default()).unwrap();
    assert_eq!(
        p.values.cells[&addr("n")],
        one(Scalar::Decimal(Decimal::parse("42").unwrap()))
    );
    assert_eq!(
        p.values.cells[&addr("d")],
        one(Scalar::Datetime(
            Instant::parse("2026-08-16T00:00:00Z").unwrap()
        ))
    );
    // Enum→text goes through the writer's lens: the label, not the id.
    assert_eq!(p.values.cells[&addr("choice")], one(Scalar::Text("Oui".into())));
    assert!(p.report.is_clean());
}

#[test]
fn checked_casts_fail_loudly_per_cell() {
    let writer = schema(vec![
        column("d", ScalarType::Decimal(None), Arity::One),
        column("choice", ScalarType::Enum(options(&[("o1", "Oui"), ("o2", "Non")])), Arity::One),
    ]);
    let reader = schema(vec![
        column("d", ScalarType::Integer(None), Arity::One),
        column("choice", ScalarType::Enum(options(&[("o1", "Oui")])), Arity::One),
    ]);
    let mut v = RecordValues::new();
    // Fractional: fails the exact-or-nothing decimal→integer cast.
    v.cells
        .insert(addr("d"), one(Scalar::Decimal(Decimal::parse("1.5").unwrap())));
    // Option o2 was removed from the reader's nomenclature.
    v.cells
        .insert(addr("choice"), one(Scalar::Enum(OptionId::new("o2"))));

    let p = project(&v, &writer, &reader, &Default::default()).unwrap();
    assert!(p.values.cells.is_empty());
    assert_eq!(p.report.total_failed(), 2);
    assert!(!p.report.is_clean());
}

#[test]
fn many_to_one_truncates_and_reports() {
    let writer = schema(vec![column(
        "tags",
        ScalarType::Enum(options(&[("o1", "A"), ("o2", "B")])),
        Arity::Many,
    )]);
    let reader = schema(vec![column(
        "tags",
        ScalarType::Enum(options(&[("o1", "A"), ("o2", "B")])),
        Arity::One,
    )]);
    let mut v = RecordValues::new();
    v.cells.insert(
        addr("tags"),
        CellState::Value(CellValue::Many(vec![
            Scalar::Enum(OptionId::new("o1")),
            Scalar::Enum(OptionId::new("o2")),
        ])),
    );

    let p = project(&v, &writer, &reader, &Default::default()).unwrap();
    assert_eq!(p.values.cells[&addr("tags")], one(Scalar::Enum(OptionId::new("o1"))));
    assert_eq!(p.report.total_lossy(), 1);

    // A singleton narrows without loss.
    let mut v2 = RecordValues::new();
    v2.cells.insert(
        addr("tags"),
        CellState::Value(CellValue::Many(vec![Scalar::Enum(OptionId::new("o2"))])),
    );
    let p2 = project(&v2, &writer, &reader, &Default::default()).unwrap();
    assert_eq!(p2.report.total_lossy(), 0);
}

#[test]
fn datetime_to_date_loss_is_per_cell() {
    let writer = schema(vec![column("t", ScalarType::Datetime, Arity::One)]);
    let reader = schema(vec![column("t", ScalarType::Date, Arity::One)]);

    let mut noon = RecordValues::new();
    noon.cells.insert(
        addr("t"),
        one(Scalar::Datetime(Instant::parse("2026-08-16T12:30:00Z").unwrap())),
    );
    let p = project(&noon, &writer, &reader, &Default::default()).unwrap();
    assert_eq!(p.report.total_lossy(), 1);

    let mut midnight = RecordValues::new();
    midnight.cells.insert(
        addr("t"),
        one(Scalar::Datetime(Instant::parse("2026-08-16T00:00:00Z").unwrap())),
    );
    let p = project(&midnight, &writer, &reader, &Default::default()).unwrap();
    assert_eq!(p.report.total_lossy(), 0);
}

#[test]
fn scope_move_is_breaking() {
    // §3 correction: a column moved into a many group cannot be
    // projected — its row-path arity changed.
    let writer = schema(vec![column("x", ScalarType::Text, Arity::One)]);
    let reader = schema(vec![Element::Group(Group {
        id: GroupId::new("g"),
        label: "g".into(),
        cardinality: Cardinality::Many,
        children: vec![column("x", ScalarType::Text, Arity::One)],
    })]);
    let mut v = RecordValues::new();
    v.cells.insert(addr("x"), one(Scalar::Text("root".into())));

    let p = project(&v, &writer, &reader, &Default::default()).unwrap();
    assert_eq!(
        p.report.columns[&ColumnId::new("x")].status,
        ColumnStatus::ScopeMoved
    );
    assert!(p.values.cells.is_empty());
    assert!(!p.report.is_clean());
}

#[test]
fn items_survive_or_drop_with_their_group() {
    let group = |children| {
        Element::Group(Group {
            id: GroupId::new("g"),
            label: "g".into(),
            cardinality: Cardinality::Many,
            children,
        })
    };
    let writer = schema(vec![group(vec![column("x", ScalarType::Text, Arity::One)])]);
    let reader_keeps = writer.clone();
    let reader_drops = schema(vec![]);

    let mut v = RecordValues::new();
    v.items.insert(
        ItemsAddr {
            group: GroupId::new("g"),
            parent: RowPath::root(),
        },
        vec![ItemId::new("i1")],
    );
    v.cells.insert(
        CellAddr {
            column: ColumnId::new("x"),
            path: RowPath::root().child(PathSeg {
                group: GroupId::new("g"),
                item: ItemId::new("i1"),
            }),
        },
        one(Scalar::Text("v".into())),
    );

    let kept = project(&v, &writer, &reader_keeps, &Default::default()).unwrap();
    assert_eq!(kept.values, v);
    let dropped = project(&v, &writer, &reader_drops, &Default::default()).unwrap();
    assert!(dropped.values.items.is_empty());
    assert_eq!(dropped.report.dropped_item_lists, 1);
}

#[test]
fn unit_conversions_are_exact_or_nothing() {
    use varve_schema::Unit;
    let metres_int = schema(vec![column(
        "d",
        ScalarType::Integer(Some(Unit::Metre)),
        Arity::One,
    )]);
    let km_int = schema(vec![column(
        "d",
        ScalarType::Integer(Some(Unit::Kilometre)),
        Arity::One,
    )]);
    let km_dec = schema(vec![column(
        "d",
        ScalarType::Decimal(Some(Unit::Kilometre)),
        Arity::One,
    )]);

    let mut v = RecordValues::new();
    v.cells.insert(addr("d"), one(Scalar::Integer(1500)));

    // 1500 m → integer km: 1.5 is not an integer — fails, counted.
    let p = project(&v, &metres_int, &km_int, &Default::default()).unwrap();
    assert_eq!(p.report.total_failed(), 1);
    // 1500 m → decimal km: exactly 1.5.
    let p = project(&v, &metres_int, &km_dec, &Default::default()).unwrap();
    assert_eq!(
        p.values.cells[&addr("d")],
        one(Scalar::Decimal(Decimal::parse("1.5").unwrap()))
    );
    assert!(p.report.is_clean());

    // 2000 m → integer km: exactly 2 — succeeds.
    let mut v2 = RecordValues::new();
    v2.cells.insert(addr("d"), one(Scalar::Integer(2000)));
    let p = project(&v2, &metres_int, &km_int, &Default::default()).unwrap();
    assert_eq!(p.values.cells[&addr("d")], one(Scalar::Integer(2)));
}

#[test]
fn unit_added_or_removed_is_pure_reinterpretation() {
    use varve_schema::Unit;
    let plain = schema(vec![column("n", ScalarType::Integer(None), Arity::One)]);
    let days = schema(vec![column(
        "n",
        ScalarType::Integer(Some(Unit::Day)),
        Arity::One,
    )]);
    let mut v = RecordValues::new();
    v.cells.insert(addr("n"), one(Scalar::Integer(12)));

    for (from, to) in [(&plain, &days), (&days, &plain)] {
        let p = project(&v, from, to, &Default::default()).unwrap();
        assert_eq!(p.values.cells[&addr("n")], one(Scalar::Integer(12)));
        assert!(p.report.is_clean());
        // Not identity: the report shows the reinterpretation.
        assert_eq!(
            p.report.columns[&ColumnId::new("n")].status,
            ColumnStatus::Cast
        );
    }
}

#[test]
fn text_to_enum_matches_labels() {
    let writer = schema(vec![column("c", ScalarType::Text, Arity::One)]);
    let reader = schema(vec![column(
        "c",
        ScalarType::Enum(options(&[("o1", "Oui"), ("o2", "Non")])),
        Arity::One,
    )]);
    let mut v = RecordValues::new();
    v.cells.insert(addr("c"), one(Scalar::Text("Non".into())));
    let p = project(&v, &writer, &reader, &Default::default()).unwrap();
    assert_eq!(p.values.cells[&addr("c")], one(Scalar::Enum(OptionId::new("o2"))));

    let mut bad = RecordValues::new();
    bad.cells.insert(addr("c"), one(Scalar::Text("Peut-être".into())));
    let p = project(&bad, &writer, &reader, &Default::default()).unwrap();
    assert_eq!(p.report.total_failed(), 1);
}
