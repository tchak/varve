use varve_core::primitives::Decimal;
use varve_core::{ColumnId, GroupId, OptionId, ResolverId, RowPath};
use varve_impact::{ChangeClass, ColumnChange, ResolverChange, assess, classify};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, Mapping, NomenclatureRef,
    OptionRow, ResolverDeclaration, ResultField, ScalarType, Schema,
};
use varve_value::{CellAddr, CellState, CellValue, RecordValues, Scalar};

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

#[test]
fn classification_covers_the_section_3_table() {
    let from = schema(vec![
        column("kept", ScalarType::Text, Arity::One),
        column("gone", ScalarType::Text, Arity::One),
        column("widened", ScalarType::Integer, Arity::One),
        column("narrowed", ScalarType::Decimal, Arity::One),
        column("truncated", ScalarType::Datetime, Arity::One),
        column("broken", ScalarType::Attachment, Arity::Many),
        column("moved", ScalarType::Text, Arity::One),
    ]);
    let to = schema(vec![
        column("kept", ScalarType::Text, Arity::One),
        column("new", ScalarType::Boolean, Arity::One),
        column("widened", ScalarType::Decimal, Arity::One),
        column("narrowed", ScalarType::Integer, Arity::One),
        column("truncated", ScalarType::Date, Arity::One),
        column("broken", ScalarType::Integer, Arity::One),
        Element::Group(Group {
            id: GroupId::new("g"),
            label: "g".into(),
            cardinality: Cardinality::Many,
            children: vec![column("moved", ScalarType::Text, Arity::One)],
        }),
    ]);

    let report = classify(&from, &to, &Default::default()).unwrap();
    let class = |id: &str| report.columns[&ColumnId::new(id)].class;
    let change = |id: &str| &report.columns[&ColumnId::new(id)].change;

    assert_eq!(class("kept"), ChangeClass::Safe);
    assert!(matches!(change("kept"), ColumnChange::Identical));
    assert_eq!(class("gone"), ChangeClass::Safe);
    assert!(matches!(change("gone"), ColumnChange::Removed));
    assert_eq!(class("new"), ChangeClass::Safe);
    assert!(matches!(change("new"), ColumnChange::Added));
    assert_eq!(class("widened"), ChangeClass::Safe);
    assert_eq!(class("narrowed"), ChangeClass::Checked);
    assert_eq!(class("truncated"), ChangeClass::Lossy);
    assert_eq!(class("broken"), ChangeClass::Breaking);
    assert!(matches!(change("broken"), ColumnChange::Forbidden));
    assert_eq!(class("moved"), ChangeClass::Breaking);
    assert!(matches!(change("moved"), ColumnChange::ScopeMoved));

    assert_eq!(report.worst(), ChangeClass::Breaking);
}

#[test]
fn enum_option_removal_is_named_precisely() {
    let from = schema(vec![column(
        "c",
        ScalarType::Enum(options(&[("o1", "Oui"), ("o2", "Non")])),
        Arity::One,
    )]);
    let to = schema(vec![column(
        "c",
        ScalarType::Enum(options(&[("o1", "Oui certes")])),
        Arity::One,
    )]);
    let report = classify(&from, &to, &Default::default()).unwrap();
    let impact = &report.columns[&ColumnId::new("c")];
    assert_eq!(impact.class, ChangeClass::Checked);
    assert_eq!(impact.removed_options, vec![OptionId::new("o2")]);

    // Relabel-only: safe, nothing removed (§2.11).
    let relabeled = schema(vec![column(
        "c",
        ScalarType::Enum(options(&[("o1", "Oui certes"), ("o2", "Non")])),
        Arity::One,
    )]);
    let report = classify(&from, &relabeled, &Default::default()).unwrap();
    assert_eq!(report.columns[&ColumnId::new("c")].class, ChangeClass::Safe);
    assert!(report.columns[&ColumnId::new("c")].removed_options.is_empty());
}

#[test]
fn resolver_impact_questions() {
    let decl = |id: &str, version: u32, target: &str| ResolverDeclaration {
        id: ResolverId::new(id),
        version,
        input: vec![(ColumnId::new("key"), ScalarType::Text)],
        result_type: vec![ResultField {
            name: "value".into(),
            ty: ScalarType::Text,
        }],
        mapping: vec![Mapping {
            result_field: "value".into(),
            target: ColumnId::new(target),
        }],
    };
    let columns = vec![
        column("key", ScalarType::Text, Arity::One),
        column("fed", ScalarType::Text, Arity::One),
        column("other", ScalarType::Text, Arity::One),
    ];
    let mut from = schema(columns.clone());
    from.resolvers = vec![decl("removed", 1, "fed"), decl("remapped", 1, "fed")];
    let mut to = schema(columns);
    to.resolvers = vec![decl("remapped", 1, "other"), decl("brand-new", 1, "fed")];

    let report = classify(&from, &to, &Default::default()).unwrap();
    assert!(report.resolvers.contains(&ResolverChange::Removed {
        id: ResolverId::new("removed"),
        orphaned_columns: vec![ColumnId::new("fed")],
    }));
    assert!(report.resolvers.contains(&ResolverChange::MappingChanged {
        id: ResolverId::new("remapped"),
        stale_columns: vec![ColumnId::new("other")],
    }));
    assert!(report.resolvers.contains(&ResolverChange::Added {
        id: ResolverId::new("brand-new"),
    }));
}

#[test]
fn assessment_turns_checked_into_exact_counts() {
    let from = schema(vec![column("d", ScalarType::Decimal, Arity::One)]);
    let to = schema(vec![column("d", ScalarType::Integer, Arity::One)]);

    let record = |s: &str| {
        let mut v = RecordValues::new();
        v.cells.insert(
            addr("d"),
            CellState::Value(CellValue::One(Scalar::Decimal(
                Decimal::parse(s).unwrap(),
            ))),
        );
        v
    };
    let records = [record("42"), record("1.5"), record("7"), record("0.25")];

    let report = assess(&from, &to, &Default::default(), records.iter()).unwrap();
    let assessment = report.records.as_ref().unwrap();
    assert_eq!(assessment.records, 4);
    // Exactly the fractional ones fail — the §7 count, per column.
    assert_eq!(assessment.records_with_failures, 2);
    assert_eq!(assessment.cells_failed, 2);
    assert_eq!(assessment.failed_by_column[&ColumnId::new("d")], 2);
    // Static class said Checked; the data decided.
    assert_eq!(report.worst(), ChangeClass::Checked);
}
