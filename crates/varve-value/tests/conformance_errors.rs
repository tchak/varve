//! Every `ConformanceError` variant `roundtrip.rs` does not already pin,
//! one deliberate mistake at a time (§2.4, §2.6, §2.15).

use varve_core::canonical::MAX_SAFE_INTEGER;
use varve_core::primitives::Date;
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RowPath};
use varve_schema::{
    Arity, AttachmentConstraints, Cardinality, Column, Element, Group, NomenclatureRef, OptionRow,
    ScalarType, Schema,
};
use varve_value::{
    AttachmentRef, CellAddr, CellState, CellValue, ConformanceError, Feature, ItemsAddr,
    RecordValues, Scalar, check,
};

fn column(id: &str, ty: ScalarType, arity: Arity) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity,
    })
}

fn group(id: &str, cardinality: Cardinality, children: Vec<Element>) -> Element {
    Element::Group(Group {
        included_from: None,
        id: GroupId::new(id),
        label: id.into(),
        cardinality,
        children,
    })
}

fn tags() -> NomenclatureRef {
    NomenclatureRef::Inline(
        ["o1", "o2"]
            .into_iter()
            .map(|id| OptionRow {
                id: OptionId::new(id),
                label: id.into(),
                fields: vec![],
            })
            .collect(),
    )
}

/// name: text one; words: text many; tags: enum many; files: attachment
/// many (≤ 1000 bytes); geo: geometry one; rib (one group) { iban:
/// text }; contacts (many group) { email: text; phones (many group) {
/// number: text } }.
fn schema() -> Schema {
    Schema {
        root: vec![
            column("name", ScalarType::Text, Arity::One),
            column("words", ScalarType::Text, Arity::Many),
            column("tags", ScalarType::Enum(tags()), Arity::Many),
            column(
                "files",
                ScalarType::Attachment(AttachmentConstraints {
                    accept: vec![],
                    max_bytes: Some(1_000),
                }),
                Arity::Many,
            ),
            column(
                "free",
                ScalarType::Attachment(Default::default()),
                Arity::One,
            ),
            column("geo", ScalarType::Geometry, Arity::One),
            group(
                "rib",
                Cardinality::One,
                vec![column("iban", ScalarType::Text, Arity::One)],
            ),
            group(
                "contacts",
                Cardinality::Many,
                vec![
                    column("email", ScalarType::Text, Arity::One),
                    group(
                        "phones",
                        Cardinality::Many,
                        vec![column("number", ScalarType::Text, Arity::One)],
                    ),
                ],
            ),
        ],
        resolvers: vec![],
    }
}

fn root(column: &str) -> CellAddr {
    CellAddr {
        column: ColumnId::new(column),
        path: RowPath::root(),
    }
}

fn one(scalar: Scalar) -> CellState {
    CellState::Value(CellValue::One(scalar))
}

fn many(scalars: Vec<Scalar>) -> CellState {
    CellState::Value(CellValue::Many(scalars))
}

fn attachment(id: &str, byte_size: u64) -> Scalar {
    use varve_core::canonical::{CanonicalValue, hash_plain};
    Scalar::Attachment(Box::new(AttachmentRef {
        id: id.into(),
        hash: hash_plain(&CanonicalValue::String(id.into())).unwrap(),
        filename: format!("{id}.pdf"),
        content_type: "application/pdf".into(),
        byte_size,
    }))
}

fn feature() -> Scalar {
    Scalar::Geometry(Box::new(
        Feature::parse(
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[1,2]},"properties":null}"#,
        )
        .unwrap(),
    ))
}

fn errors(v: &RecordValues) -> Vec<ConformanceError> {
    check(v, &schema(), &Default::default())
}

#[test]
fn unknown_column() {
    let mut v = RecordValues::new();
    v.cells.insert(root("nope"), one(Scalar::Text("x".into())));
    assert_eq!(
        errors(&v),
        vec![ConformanceError::UnknownColumn(ColumnId::new("nope"))]
    );
    // Empty in an unknown column is still an unknown column.
    let mut v = RecordValues::new();
    v.cells.insert(root("nope"), CellState::Empty);
    assert_eq!(
        errors(&v),
        vec![ConformanceError::UnknownColumn(ColumnId::new("nope"))]
    );
}

#[test]
fn arity_mismatch_both_ways() {
    let mut v = RecordValues::new();
    // A list in a `one` column.
    v.cells
        .insert(root("name"), many(vec![Scalar::Text("a".into())]));
    // A single value in a `many` column.
    v.cells.insert(root("words"), one(Scalar::Text("a".into())));
    let errs = errors(&v);
    assert!(errs.contains(&ConformanceError::ArityMismatch(ColumnId::new("name"))));
    assert!(errs.contains(&ConformanceError::ArityMismatch(ColumnId::new("words"))));
    assert_eq!(errs.len(), 2);
    // Arity is checked before type: a wrongly-typed list in a `one`
    // column is one arity error, not a type error per element.
    let mut v = RecordValues::new();
    v.cells.insert(root("name"), many(vec![Scalar::Integer(1)]));
    assert_eq!(
        errors(&v),
        vec![ConformanceError::ArityMismatch(ColumnId::new("name"))]
    );
}

#[test]
fn unknown_group() {
    let mut v = RecordValues::new();
    v.items.insert(
        ItemsAddr {
            group: GroupId::new("ghosts"),
            parent: RowPath::root(),
        },
        vec![ItemId::new("i1")],
    );
    assert_eq!(
        errors(&v),
        vec![ConformanceError::UnknownGroup(GroupId::new("ghosts"))]
    );
}

#[test]
fn misplaced_items() {
    // An item list for a cardinality-`one` group: `one` groups have no
    // items (§2.5).
    let mut v = RecordValues::new();
    v.items.insert(
        ItemsAddr {
            group: GroupId::new("rib"),
            parent: RowPath::root(),
        },
        vec![ItemId::new("i1")],
    );
    assert_eq!(
        errors(&v),
        vec![ConformanceError::MisplacedItems(GroupId::new("rib"))]
    );

    // A `many` group's list at the wrong scope: `phones` lives under a
    // contact, not at the root …
    let mut v = RecordValues::new();
    v.items.insert(
        ItemsAddr {
            group: GroupId::new("phones"),
            parent: RowPath::root(),
        },
        vec![ItemId::new("p1")],
    );
    assert_eq!(
        errors(&v),
        vec![ConformanceError::MisplacedItems(GroupId::new("phones"))]
    );
    // … and `contacts` lives at the root, not under a contact. The
    // parent path also names an item that exists, so this is purely a
    // scope error.
    let mut v = RecordValues::new();
    v.items.insert(
        ItemsAddr {
            group: GroupId::new("contacts"),
            parent: RowPath::root(),
        },
        vec![ItemId::new("c1")],
    );
    let under_c1 = RowPath::root().child(PathSeg {
        group: GroupId::new("contacts"),
        item: ItemId::new("c1"),
    });
    v.items.insert(
        ItemsAddr {
            group: GroupId::new("contacts"),
            parent: under_c1,
        },
        vec![ItemId::new("c2")],
    );
    assert_eq!(
        errors(&v),
        vec![ConformanceError::MisplacedItems(GroupId::new("contacts"))]
    );
}

#[test]
fn duplicate_item() {
    let mut v = RecordValues::new();
    v.items.insert(
        ItemsAddr {
            group: GroupId::new("contacts"),
            parent: RowPath::root(),
        },
        vec![ItemId::new("c1"), ItemId::new("c2"), ItemId::new("c1")],
    );
    assert_eq!(
        errors(&v),
        vec![ConformanceError::DuplicateItem(GroupId::new("contacts"))]
    );
}

#[test]
fn duplicate_element_identity() {
    // Repeated enum id in one `many` cell (§2.4: options are
    // self-identifying).
    let mut v = RecordValues::new();
    v.cells.insert(
        root("tags"),
        many(vec![
            Scalar::Enum(OptionId::new("o1")),
            Scalar::Enum(OptionId::new("o1")),
        ]),
    );
    assert_eq!(
        errors(&v),
        vec![ConformanceError::DuplicateElement(ColumnId::new("tags"))]
    );

    // Repeated attachment id — even with different content.
    let mut v = RecordValues::new();
    let mut second = attachment("f1", 20);
    if let Scalar::Attachment(a) = &mut second {
        a.filename = "other.pdf".into();
    }
    v.cells
        .insert(root("files"), many(vec![attachment("f1", 10), second]));
    assert_eq!(
        errors(&v),
        vec![ConformanceError::DuplicateElement(ColumnId::new("files"))]
    );

    // Text elements carry no identity: repeats are not flagged.
    let mut v = RecordValues::new();
    v.cells.insert(
        root("words"),
        many(vec![Scalar::Text("a".into()), Scalar::Text("a".into())]),
    );
    assert_eq!(errors(&v), vec![]);
}

#[test]
fn attachment_size_unrepresentable() {
    let huge = MAX_SAFE_INTEGER as u64 + 1;
    // With a size limit: too large *and* unrepresentable — both named.
    let mut v = RecordValues::new();
    v.cells
        .insert(root("files"), many(vec![attachment("f1", huge)]));
    assert_eq!(
        errors(&v),
        vec![
            ConformanceError::AttachmentTooLarge(ColumnId::new("files")),
            ConformanceError::AttachmentSizeUnrepresentable(ColumnId::new("files")),
        ]
    );
    // Without a limit: only the JCS-range error.
    let mut v = RecordValues::new();
    v.cells.insert(root("free"), one(attachment("f1", huge)));
    assert_eq!(
        errors(&v),
        vec![ConformanceError::AttachmentSizeUnrepresentable(
            ColumnId::new("free")
        )]
    );
    // Exactly the bound is representable.
    let mut v = RecordValues::new();
    v.cells
        .insert(root("free"), one(attachment("f1", MAX_SAFE_INTEGER as u64)));
    assert_eq!(errors(&v), vec![]);
}

#[test]
fn type_mismatch_off_the_diagonal() {
    let cases: Vec<(&str, Scalar)> = vec![
        ("name", feature()),                        // geometry in a text column
        ("name", attachment("f1", 10)),             // attachment in a text column
        ("geo", Scalar::Text("POINT(1 2)".into())), // text in a geometry column
        ("free", Scalar::Text("file.pdf".into())),  // text in an attachment column
        ("name", Scalar::Date(Date::parse("2026-08-18").unwrap())),
        ("name", Scalar::Boolean(true)),
        ("geo", attachment("f1", 10)),
        ("free", feature()),
    ];
    for (column, scalar) in cases {
        let mut v = RecordValues::new();
        v.cells.insert(root(column), one(scalar.clone()));
        assert_eq!(
            errors(&v),
            vec![ConformanceError::TypeMismatch(ColumnId::new(column))],
            "{column} <- {scalar:?}"
        );
    }
    // Inside a `many` cell every wrong element is reported.
    let mut v = RecordValues::new();
    v.cells.insert(
        root("words"),
        many(vec![
            Scalar::Integer(1),
            Scalar::Text("ok".into()),
            Scalar::Boolean(false),
        ]),
    );
    assert_eq!(
        errors(&v),
        vec![
            ConformanceError::TypeMismatch(ColumnId::new("words")),
            ConformanceError::TypeMismatch(ColumnId::new("words")),
        ]
    );
}
