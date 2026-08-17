use varve_core::primitives::Decimal;
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RowPath};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, NomenclatureRef, OptionRow,
    ScalarType, Schema,
};
use varve_value::{
    AttachmentRef, CellAddr, CellState, CellValue, ConformanceError, ItemsAddr,
    Op, RecordValues, Scalar, apply, cell_delta, check, diff,
};

fn column(id: &str, ty: ScalarType, arity: Arity) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity,
    })
}

/// name: text, amount: decimal, tags: enum many, files: attachment many,
/// contacts (many group) { email: text }.
fn schema() -> Schema {
    let tags = NomenclatureRef::Inline(vec![
        OptionRow {
            id: OptionId::new("o1"),
            label: "Urgent".into(),
            fields: vec![],
        },
        OptionRow {
            id: OptionId::new("o2"),
            label: "Complet".into(),
            fields: vec![],
        },
    ]);
    Schema {
        root: vec![
            column("name", ScalarType::Text, Arity::One),
            column("amount", ScalarType::Decimal(None), Arity::One),
            column("tags", ScalarType::Enum(tags), Arity::Many),
            column("files", ScalarType::Attachment, Arity::Many),
            Element::Group(Group {
                id: GroupId::new("contacts"),
                label: "contacts".into(),
                cardinality: Cardinality::Many,
                children: vec![column("email", ScalarType::Text, Arity::One)],
            }),
        ],
        resolvers: vec![],
    }
}

fn root_cell(column: &str) -> CellAddr {
    CellAddr {
        column: ColumnId::new(column),
        path: RowPath::root(),
    }
}

fn contact_path(item: &str) -> RowPath {
    RowPath::root().child(PathSeg {
        group: GroupId::new("contacts"),
        item: ItemId::new(item),
    })
}

fn one(scalar: Scalar) -> CellState {
    CellState::Value(CellValue::One(scalar))
}

fn attachment(id: &str, sha: &str) -> Scalar {
    Scalar::Attachment(AttachmentRef {
        id: id.into(),
        sha256: sha.into(),
        filename: format!("{id}.pdf"),
    })
}

fn sample() -> RecordValues {
    let mut v = RecordValues::new();
    v.cells
        .insert(root_cell("name"), one(Scalar::Text("Dupont".into())));
    v.cells.insert(
        root_cell("amount"),
        one(Scalar::Decimal(Decimal::parse("120.50").unwrap())),
    );
    v.cells.insert(
        root_cell("tags"),
        CellState::Value(CellValue::Many(vec![
            Scalar::Enum(OptionId::new("o1")),
            Scalar::Enum(OptionId::new("o2")),
        ])),
    );
    v.cells.insert(
        root_cell("files"),
        CellState::Value(CellValue::Many(vec![
            attachment("f1", "aaa"),
            attachment("f2", "bbb"),
        ])),
    );
    v.items.insert(
        ItemsAddr {
            group: GroupId::new("contacts"),
            parent: RowPath::root(),
        },
        vec![ItemId::new("i1"), ItemId::new("i2")],
    );
    for item in ["i1", "i2"] {
        v.cells.insert(
            CellAddr {
                column: ColumnId::new("email"),
                path: contact_path(item),
            },
            one(Scalar::Text(format!("{item}@example.org"))),
        );
    }
    v
}

#[test]
fn sample_conforms() {
    let errors = check(&sample(), &schema(), &Default::default());
    assert_eq!(errors, vec![]);
}

#[test]
fn empty_cell_conforms_everywhere() {
    let mut v = sample();
    v.cells.insert(root_cell("amount"), CellState::Empty);
    assert_eq!(check(&v, &schema(), &Default::default()), vec![]);
}

#[test]
fn conformance_catches_mistakes() {
    let mut v = sample();
    // Wrong scalar type.
    v.cells
        .insert(root_cell("amount"), one(Scalar::Text("12,5".into())));
    // Unknown enum option.
    v.cells.insert(
        root_cell("tags"),
        CellState::Value(CellValue::Many(vec![Scalar::Enum(OptionId::new("nope"))])),
    );
    // Item-scoped column addressed at root.
    v.cells
        .insert(root_cell("email"), one(Scalar::Text("x@y.z".into())));
    // Cell naming an item that is not in the list.
    v.cells.insert(
        CellAddr {
            column: ColumnId::new("email"),
            path: contact_path("ghost"),
        },
        one(Scalar::Text("ghost@example.org".into())),
    );
    let errors = check(&v, &schema(), &Default::default());
    assert!(errors.contains(&ConformanceError::TypeMismatch(ColumnId::new("amount"))));
    assert!(errors.contains(&ConformanceError::UnknownOption(
        ColumnId::new("tags"),
        OptionId::new("nope")
    )));
    assert!(errors.contains(&ConformanceError::ScopeMismatch(ColumnId::new("email"))));
    assert!(errors.contains(&ConformanceError::UnknownItem(
        ColumnId::new("email"),
        GroupId::new("contacts")
    )));
    assert_eq!(errors.len(), 4);
}

#[test]
fn diff_then_apply_reproduces_target() {
    let from = sample();
    let mut to = sample();

    // Scalar edit.
    to.cells
        .insert(root_cell("name"), one(Scalar::Text("Durand".into())));
    // Unset a cell entirely.
    to.cells.remove(&root_cell("amount"));
    // Replace file f2's content, drop f1, add f3.
    to.cells.insert(
        root_cell("files"),
        CellState::Value(CellValue::Many(vec![
            attachment("f2", "b-new"),
            attachment("f3", "ccc"),
        ])),
    );
    // Remove contact i1 (with its cell), add i3, reorder to [i3, i2].
    let contacts = ItemsAddr {
        group: GroupId::new("contacts"),
        parent: RowPath::root(),
    };
    to.items
        .insert(contacts.clone(), vec![ItemId::new("i3"), ItemId::new("i2")]);
    to.cells.remove(&CellAddr {
        column: ColumnId::new("email"),
        path: contact_path("i1"),
    });
    to.cells.insert(
        CellAddr {
            column: ColumnId::new("email"),
            path: contact_path("i3"),
        },
        one(Scalar::Text("i3@example.org".into())),
    );

    let ops = diff(&from, &to);
    let mut replay = from.clone();
    for op in &ops {
        apply(&mut replay, op).expect("op must apply");
    }
    assert_eq!(replay, to);
    // The removed item's cell rides the cascade: no explicit Unset for it.
    assert!(!ops.iter().any(|op| matches!(
        op,
        Op::Unset { column, path }
            if column == &ColumnId::new("email") && path == &contact_path("i1")
    )));

    // Conformance holds on both ends.
    assert_eq!(check(&to, &schema(), &Default::default()), vec![]);
}

#[test]
fn diff_of_identical_values_is_empty() {
    assert_eq!(diff(&sample(), &sample()), vec![]);
}

#[test]
fn full_export_is_a_patch_against_empty() {
    // §5: a snapshot export is a patch against the empty state.
    let target = sample();
    let ops = diff(&RecordValues::new(), &target);
    assert!(ops.iter().all(|op| matches!(
        op,
        Op::Set { .. } | Op::AddItem { .. } | Op::Reorder { .. }
    )));
    let mut replay = RecordValues::new();
    for op in &ops {
        apply(&mut replay, op).expect("op must apply");
    }
    assert_eq!(replay, target);
}

#[test]
fn remove_item_cascades() {
    let mut v = sample();
    apply(
        &mut v,
        &Op::RemoveItem {
            group: GroupId::new("contacts"),
            parent: RowPath::root(),
            item: ItemId::new("i1"),
        },
    )
    .unwrap();
    assert!(!v.cells.contains_key(&CellAddr {
        column: ColumnId::new("email"),
        path: contact_path("i1"),
    }));
    assert!(v.cells.contains_key(&CellAddr {
        column: ColumnId::new("email"),
        path: contact_path("i2"),
    }));
}

#[test]
fn geometry_is_validated_geojson() {
    use varve_value::{Feature, GeometryError};
    let point = r#"{"type":"Feature","id":7,"geometry":{"type":"Point","coordinates":[2.35,48.85]},"properties":null}"#;
    let feature = Feature::parse(point).unwrap();
    // Numeric GeoJSON ids normalize to text element identity.
    assert_eq!(feature.id(), Some("7"));
    // Equality is semantic, not textual.
    let spaced = point.replace(":", ": ");
    assert_eq!(feature, Feature::parse(&spaced).unwrap());
    // A geometry cell holds Features only.
    assert_eq!(
        Feature::parse(r#"{"type":"Point","coordinates":[0,0]}"#),
        Err(GeometryError::NotAFeature)
    );
    assert_eq!(
        Feature::parse(r#"{"type":"FeatureCollection","features":[]}"#),
        Err(GeometryError::NotAFeature)
    );
    assert_eq!(Feature::parse("not json"), Err(GeometryError::Malformed));
}

#[test]
fn file_replacement_is_visible_element_wise() {
    let old = CellValue::Many(vec![attachment("f1", "aaa"), attachment("f2", "bbb")]);
    let new = CellValue::Many(vec![attachment("f2", "b-new"), attachment("f3", "ccc")]);
    let delta = cell_delta(&old, &new).unwrap();
    assert_eq!(delta.removed, vec!["f1"]);
    assert_eq!(delta.changed, vec!["f2"]);
    assert_eq!(delta.added, vec!["f3"]);
    assert!(!delta.unidentified);
}
