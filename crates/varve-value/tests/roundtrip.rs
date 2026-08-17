use varve_core::primitives::Decimal;
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RowPath};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, NomenclatureRef, OptionRow,
    ScalarType, Schema,
};
use varve_value::{
    ApplyError, AttachmentRef, CellAddr, CellState, CellValue, ConformanceError, ItemsAddr,
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
            column("files", ScalarType::Attachment(Default::default()), Arity::Many),
            Element::Group(Group {
                included_from: None,
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
fn attachment_claims_are_checked_against_constraints() {
    use varve_schema::AttachmentConstraints;
    let constrained = Schema {
        root: vec![column(
            "piece",
            ScalarType::Attachment(AttachmentConstraints {
                accept: vec!["application/pdf".into(), "image/*".into()],
                max_bytes: Some(2_000),
            }),
            Arity::Many,
        )],
        resolvers: vec![],
    };
    let cell = |ct: &str, size: u64| {
        let mut v = RecordValues::new();
        let mut a = match attachment("f1", "content") {
            Scalar::Attachment(a) => a,
            _ => unreachable!(),
        };
        a.content_type = ct.into();
        a.byte_size = size;
        v.cells.insert(
            CellAddr { column: ColumnId::new("piece"), path: RowPath::root() },
            CellState::Value(CellValue::Many(vec![Scalar::Attachment(a)])),
        );
        v
    };
    // Accepted exactly and by wildcard; within size.
    assert_eq!(check(&cell("application/pdf", 1_000), &constrained, &Default::default()), vec![]);
    assert_eq!(check(&cell("image/png", 1_000), &constrained, &Default::default()), vec![]);
    // Wrong type and oversized: both claims checked, zero IO.
    let errors = check(&cell("video/mp4", 9_000), &constrained, &Default::default());
    assert!(errors.contains(&ConformanceError::AttachmentTypeNotAccepted(
        ColumnId::new("piece"),
        "video/mp4".into()
    )));
    assert!(errors.contains(&ConformanceError::AttachmentTooLarge(ColumnId::new("piece"))));
}

#[test]
fn enum_membership_is_checked_against_the_bound_nomenclature_version() {
    // §2.12: a column typed "id from N@v" has a closed id set — v's rows,
    // not the latest. Same cell, two bindings, two verdicts.
    use varve_core::NomenclatureId;
    use varve_schema::{NomenclatureRef, NomenclatureTable, OptionRow};
    let row = |id: &str| OptionRow { id: OptionId::new(id), label: id.into(), fields: vec![] };
    let cog = NomenclatureId::new("cog");
    let mut table = NomenclatureTable::new();
    table.insert(cog.clone(), 1, vec![row("01")]);
    table.insert(cog.clone(), 2, vec![row("01"), row("02")]);
    let bound_to = |version: u32| Schema {
        root: vec![column(
            "commune",
            ScalarType::Enum(NomenclatureRef::Published { id: cog.clone(), version }),
            Arity::One,
        )],
        resolvers: vec![],
    };
    let mut v = RecordValues::new();
    v.cells.insert(
        CellAddr { column: ColumnId::new("commune"), path: RowPath::root() },
        one(Scalar::Enum(OptionId::new("02"))),
    );
    assert_eq!(check(&v, &bound_to(2), &table), vec![]);
    assert_eq!(
        check(&v, &bound_to(1), &table),
        vec![ConformanceError::UnknownOption(ColumnId::new("commune"), OptionId::new("02"))]
    );
    assert_eq!(
        check(&v, &bound_to(3), &table),
        vec![ConformanceError::UnknownNomenclature(ColumnId::new("commune"), cog, 3)]
    );
}

#[test]
fn one_state_one_encoding_no_empty_lists() {
    // §2.4: a blank `many` cell is `Empty`, never a zero-length list; a
    // `many` group with no items has no item list. Apply refuses to
    // produce either, and conformance flags them if built by hand.
    let mut v = RecordValues::new();
    let tags = Op::Set {
        column: ColumnId::new("tags"),
        path: RowPath::root(),
        state: CellState::Value(CellValue::Many(vec![])),
    };
    assert_eq!(apply(&mut v, &tags), Err(ApplyError::EmptyList(ColumnId::new("tags"))));
    assert!(v.cells.is_empty());

    // A failing AddItem leaves nothing behind — no `[]` under the group.
    let bad = Op::AddItem {
        group: GroupId::new("contacts"),
        parent: RowPath::root(),
        item: ItemId::new("c1"),
        at: 5,
    };
    assert_eq!(apply(&mut v, &bad), Err(ApplyError::BadIndex(GroupId::new("contacts"), 5)));
    assert!(v.items.is_empty());
    assert_eq!(v, RecordValues::new());

    // Hand-built alternatives are non-conforming.
    let mut v = RecordValues::new();
    v.cells.insert(root_cell("tags"), CellState::Value(CellValue::Many(vec![])));
    v.items.insert(
        ItemsAddr { group: GroupId::new("contacts"), parent: RowPath::root() },
        vec![],
    );
    let errors = check(&v, &schema(), &Default::default());
    assert!(errors.contains(&ConformanceError::EmptyList(ColumnId::new("tags"))));
    assert!(errors.contains(&ConformanceError::EmptyItemList(GroupId::new("contacts"))));
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
fn geometry_canonical_form_is_jcs() {
    // §2.13 decision 3: a feature's canonical form is its JSON value
    // with ES6 number rendering — the same bytes any JCS implementation
    // produces — and equality is equality of that form.
    use varve_core::canonical::{CanonicalValue, canonical_bytes};
    use varve_value::Feature;
    let a = Feature::parse(
        r#"{"type":"Feature","id":7,"geometry":{"type":"Point","coordinates":[-0.0,1.0,1e21]},"properties":{"n":100000000000000000000000,"s":"x"}}"#,
    )
    .unwrap();
    let b = Feature::parse(
        r#"{"properties":{"s":"x","n":1e23},"geometry":{"coordinates":[0,1,1000000000000000000000],"type":"Point"},"id":7.0,"type":"Feature"}"#,
    )
    .unwrap();
    // `-0.0`/`0`, `1.0`/`1`, `1e21`/`1000…`, `7`/`7.0`: one value each.
    assert_eq!(a, b);
    assert_eq!(a.id(), Some("7"));
    // Display *is* the JCS text: sorted keys, ES6 numbers.
    let jcs = r#"{"geometry":{"coordinates":[0,1,1e+21],"type":"Point"},"id":7,"properties":{"n":1e+23,"s":"x"},"type":"Feature"}"#;
    assert_eq!(a.to_string(), jcs);
    assert_eq!(canonical_bytes(a.to_canonical()).unwrap(), jcs.as_bytes());
    // The canonical value is a JSON object, not a stringified blob, and
    // decodes back to the same feature.
    assert!(matches!(a.to_canonical(), CanonicalValue::Object(_)));
    assert_eq!(Feature::from_canonical(a.to_canonical()).unwrap(), a);
    // A canonical value that is not a Feature is refused.
    assert!(Feature::from_canonical(&CanonicalValue::String("nope".into())).is_err());
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
