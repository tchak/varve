use std::collections::BTreeSet;

use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RevisionId, RowPath, SurfaceId};
use varve_logic::{Atom, ColumnRef, Const, Expr, Operand};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, NomenclatureRef, OptionRow, ScalarType, Schema,
    revision_id,
};
use varve_surface::{
    ColumnNode, Finding, Format, GroupNode, Ineligibility, Node, Section, Surface, SurfaceError,
    WritePolicy, admissibility, reachability, validate,
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

fn yes_no() -> NomenclatureRef {
    NomenclatureRef::Inline(vec![
        OptionRow {
            id: OptionId::new("oui"),
            label: "Oui".into(),
            fields: vec![],
        },
        OptionRow {
            id: OptionId::new("non"),
            label: "Non".into(),
            fields: vec![],
        },
    ])
}

fn schema() -> Schema {
    Schema {
        root: vec![
            column("situation", ScalarType::Enum(yes_no()), Arity::One),
            column("detail", ScalarType::Text, Arity::One),
            column("email", ScalarType::Text, Arity::One),
            Element::Group(Group {
                included_from: None,
                id: GroupId::new("contacts"),
                label: "contacts".into(),
                cardinality: Cardinality::Many,
                children: vec![
                    column("role", ScalarType::Enum(yes_no()), Arity::One),
                    column("precision", ScalarType::Text, Arity::One),
                ],
            }),
        ],
        resolvers: vec![],
    }
}

fn col_node(id: &str) -> ColumnNode {
    ColumnNode {
        column: ColumnId::new(id),
        prompt: None,
        help: None,
        visibility: None,
        required: None,
        write: WritePolicy::default(),
        format: None,
    }
}

fn when_oui(source_id: &str) -> Expr {
    Expr::Atom(Atom::Eq {
        source: ColumnRef {
            column: ColumnId::new(source_id),
            field: None,
        },
        right: Operand::Const(Const::Option(OptionId::new("oui"))),
    })
}

fn always() -> Expr {
    Expr::And(vec![])
}

fn surface(nodes: Vec<Node>) -> Surface {
    Surface {
        id: SurfaceId::new("public"),
        revision: revision_id(&schema()),
        nodes,
        ineligibility: None,
    }
}

fn set(values: &mut RecordValues, id: &str, scalar: Scalar) {
    values.cells.insert(
        CellAddr {
            column: ColumnId::new(id),
            path: RowPath::root(),
        },
        CellState::Value(CellValue::One(scalar)),
    );
}

#[test]
fn validation_catches_structure_and_rules() {
    let s = schema();
    // A surface authored against another revision.
    let mut stale = surface(vec![]);
    stale.revision = RevisionId::new("rev-0");
    assert!(matches!(
        validate(&stale, &s, &Default::default()).as_slice(),
        [SurfaceError::RevisionMismatch { .. }]
    ));
    let noms = Default::default();

    // Item column placed at root: misplaced.
    let bad = surface(vec![Node::Column(col_node("role"))]);
    assert!(matches!(
        validate(&bad, &s, &noms).as_slice(),
        [SurfaceError::MisplacedColumn(_)]
    ));

    // Format on a non-text column.
    let mut node = col_node("situation");
    node.format = Some(Format::Email);
    let bad = surface(vec![Node::Column(node)]);
    assert!(matches!(
        validate(&bad, &s, &noms).as_slice(),
        [SurfaceError::FormatOnNonText(_)]
    ));

    // A visibility cycle across two columns.
    let mut a = col_node("situation");
    a.visibility = Some(Expr::Atom(Atom::IsFilled {
        source: ColumnRef {
            column: ColumnId::new("detail"),
            field: None,
        },
    }));
    let mut b = col_node("detail");
    b.visibility = Some(Expr::Atom(Atom::IsFilled {
        source: ColumnRef {
            column: ColumnId::new("situation"),
            field: None,
        },
    }));
    let bad = surface(vec![Node::Column(a), Node::Column(b)]);
    assert!(matches!(
        validate(&bad, &s, &noms).as_slice(),
        [SurfaceError::Cycle(_)]
    ));

    // A correct surface, with a group in place: clean.
    let good = surface(vec![
        Node::Column(col_node("situation")),
        Node::Group(GroupNode {
            group: GroupId::new("contacts"),
            prompt: None,
            visibility: None,
            children: vec![
                Node::Column(col_node("role")),
                Node::Column(col_node("precision")),
            ],
        }),
    ]);
    assert_eq!(validate(&good, &s, &noms), vec![]);
}

#[test]
fn reachability_cascades_and_sections_hide_children() {
    let s = schema();
    let noms = Default::default();
    // detail visible when situation = oui; email inside a section shown
    // under the same condition.
    let mut detail = col_node("detail");
    detail.visibility = Some(when_oui("situation"));
    let surf = surface(vec![
        Node::Column(col_node("situation")),
        Node::Column(detail),
        Node::Section(Section {
            title: "Contact".into(),
            help: None,
            visibility: Some(when_oui("situation")),
            children: vec![Node::Column(col_node("email"))],
        }),
    ]);

    let mut values = RecordValues::new();
    let pending = BTreeSet::new();
    let reach = reachability(&surf, &s, &noms, &values, &pending).unwrap();
    // Unanswered: both conditioned columns hidden (progressive
    // disclosure falls out of absence-loses).
    assert!(!reach.is_visible(&ColumnId::new("detail"), &RowPath::root()));
    assert!(!reach.is_visible(&ColumnId::new("email"), &RowPath::root()));
    assert!(reach.is_visible(&ColumnId::new("situation"), &RowPath::root()));

    set(&mut values, "situation", Scalar::Enum(OptionId::new("oui")));
    let reach = reachability(&surf, &s, &noms, &values, &pending).unwrap();
    assert!(reach.is_visible(&ColumnId::new("detail"), &RowPath::root()));
    assert!(reach.is_visible(&ColumnId::new("email"), &RowPath::root()));
}

#[test]
fn per_item_reachability() {
    let s = schema();
    let noms = Default::default();
    // precision visible when the item's role = oui.
    let mut precision = col_node("precision");
    precision.visibility = Some(when_oui("role"));
    let surf = surface(vec![Node::Group(GroupNode {
        group: GroupId::new("contacts"),
        prompt: None,
        visibility: None,
        children: vec![Node::Column(col_node("role")), Node::Column(precision)],
    })]);

    let mut values = RecordValues::new();
    values.items.insert(
        ItemsAddr {
            group: GroupId::new("contacts"),
            parent: RowPath::root(),
        },
        vec![ItemId::new("i1"), ItemId::new("i2")],
    );
    let path = |item: &str| {
        RowPath::root().child(PathSeg {
            group: GroupId::new("contacts"),
            item: ItemId::new(item),
        })
    };
    values.cells.insert(
        CellAddr {
            column: ColumnId::new("role"),
            path: path("i1"),
        },
        CellState::Value(CellValue::One(Scalar::Enum(OptionId::new("oui")))),
    );
    values.cells.insert(
        CellAddr {
            column: ColumnId::new("role"),
            path: path("i2"),
        },
        CellState::Value(CellValue::One(Scalar::Enum(OptionId::new("non")))),
    );

    let reach = reachability(&surf, &s, &noms, &values, &BTreeSet::new()).unwrap();
    // Same column, different items, different visibility.
    assert!(reach.is_visible(&ColumnId::new("precision"), &path("i1")));
    assert!(!reach.is_visible(&ColumnId::new("precision"), &path("i2")));
}

#[test]
fn two_surfaces_one_record_disagree_on_admissibility() {
    // §2.6's headline: the same record can be complete for the public
    // surface and incomplete for the back-office one — neither lies.
    let s = schema();
    let noms = Default::default();
    let pending = BTreeSet::new();

    let mut public_detail = col_node("detail");
    public_detail.required = None; // not required publicly
    let public = surface(vec![
        Node::Column(col_node("situation")),
        Node::Column(public_detail),
    ]);

    let mut back_office_detail = col_node("detail");
    back_office_detail.required = Some(always());
    let back_office = Surface {
        id: SurfaceId::new("back-office"),
        ..surface(vec![
            Node::Column(col_node("situation")),
            Node::Column(back_office_detail),
        ])
    };

    let mut values = RecordValues::new();
    set(&mut values, "situation", Scalar::Enum(OptionId::new("oui")));

    let on_public = admissibility(&public, &s, &noms, &values, &pending).unwrap();
    assert!(on_public.is_admissible());
    let on_back_office = admissibility(&back_office, &s, &noms, &values, &pending).unwrap();
    assert!(!on_back_office.is_admissible());
    assert!(matches!(
        on_back_office.findings.as_slice(),
        [Finding::MissingRequired { column, .. }] if column == &ColumnId::new("detail")
    ));
}

#[test]
fn required_only_when_reachable_and_formats_only_when_filled() {
    let s = schema();
    let noms = Default::default();
    let pending = BTreeSet::new();

    // detail: visible & required only when situation = oui; email has a
    // format constraint.
    let mut detail = col_node("detail");
    detail.visibility = Some(when_oui("situation"));
    detail.required = Some(always());
    let mut email = col_node("email");
    email.format = Some(Format::Email);
    let surf = surface(vec![
        Node::Column(col_node("situation")),
        Node::Column(detail),
        Node::Column(email),
    ]);

    // Unanswered situation: detail is hidden, so not missing-required.
    let values = RecordValues::new();
    let report = admissibility(&surf, &s, &noms, &values, &pending).unwrap();
    assert!(report.is_admissible());

    // situation = oui: detail becomes reachable and required.
    let mut values = RecordValues::new();
    set(&mut values, "situation", Scalar::Enum(OptionId::new("oui")));
    set(&mut values, "email", Scalar::Text("not-an-email".into()));
    let report = admissibility(&surf, &s, &noms, &values, &pending).unwrap();
    assert_eq!(report.findings.len(), 2);
    assert!(report.findings.iter().any(|f| matches!(
        f,
        Finding::MissingRequired { column, .. } if column == &ColumnId::new("detail")
    )));
    assert!(report.findings.iter().any(|f| matches!(
        f,
        Finding::FormatViolation { column, .. } if column == &ColumnId::new("email")
    )));
}

#[test]
fn reachability_refuses_a_duplicated_column_node() {
    // validate reports duplicates; reachability must not silently keep
    // the last node's rule if handed an unvalidated surface.
    let s = schema();
    let mut shown = col_node("detail");
    shown.visibility = Some(always());
    let surf = surface(vec![Node::Column(col_node("detail")), Node::Column(shown)]);
    assert!(matches!(
        reachability(&surf, &s, &Default::default(), &RecordValues::new(), &BTreeSet::new()),
        Err(SurfaceError::DuplicateColumn(c)) if c == ColumnId::new("detail")
    ));
}

#[test]
fn formats_apply_to_every_element_of_a_many_text_cell() {
    // A multi-valued email column: the format holds for each element;
    // one bad address is a violation, blanks are the required rule's.
    let s = Schema {
        root: vec![column("emails", ScalarType::Text, Arity::Many)],
        resolvers: vec![],
    };
    let mut emails = col_node("emails");
    emails.format = Some(Format::Email);
    let mut surf = surface(vec![Node::Column(emails)]);
    surf.revision = revision_id(&s);
    assert_eq!(validate(&surf, &s, &Default::default()), vec![]);
    let cell = |list: &[&str]| {
        let mut v = RecordValues::new();
        v.cells.insert(
            CellAddr {
                column: ColumnId::new("emails"),
                path: RowPath::root(),
            },
            CellState::Value(CellValue::Many(
                list.iter().map(|t| Scalar::Text((*t).into())).collect(),
            )),
        );
        v
    };
    let ok = admissibility(
        &surf,
        &s,
        &Default::default(),
        &cell(&["a@b.fr", "c@d.fr"]),
        &BTreeSet::new(),
    )
    .unwrap();
    assert!(ok.findings.is_empty());
    let bad = admissibility(
        &surf,
        &s,
        &Default::default(),
        &cell(&["a@b.fr", "nope"]),
        &BTreeSet::new(),
    )
    .unwrap();
    assert!(
        matches!(bad.findings.as_slice(), [Finding::FormatViolation { column, .. }] if column == &ColumnId::new("emails"))
    );
}

#[test]
fn ineligibility_and_columns() {
    let s = schema();
    let noms = Default::default();
    let mut surf = surface(vec![Node::Column(col_node("situation"))]);
    surf.ineligibility = Some(Ineligibility {
        rule: when_oui("situation"),
        message: "Non éligible.".into(),
    });
    assert_eq!(validate(&surf, &s, &noms), vec![]);

    let mut values = RecordValues::new();
    set(&mut values, "situation", Scalar::Enum(OptionId::new("oui")));
    let report = admissibility(&surf, &s, &noms, &values, &BTreeSet::new()).unwrap();
    assert!(matches!(
        report.findings.as_slice(),
        [Finding::Ineligible { message }] if message == "Non éligible."
    ));

    // The §2.9 entry-visibility filter's static column set.
    assert_eq!(surf.columns(), BTreeSet::from([ColumnId::new("situation")]));
}

#[test]
fn writable_set_is_what_a_checkpoint_freezes() {
    // §2.8: a checkpoint taken through a surface freezes the columns
    // writable there and the groups holding them — read-only columns
    // and groups with no writable column stay out of the frozen set.
    let mut read_only = col_node("detail");
    read_only.write = WritePolicy {
        writable: false,
        override_derived: false,
    };
    let mut ro_role = col_node("role");
    ro_role.write = WritePolicy {
        writable: false,
        override_derived: false,
    };
    let surf = surface(vec![
        Node::Column(col_node("situation")),
        Node::Column(read_only),
        Node::Group(GroupNode {
            group: GroupId::new("contacts"),
            prompt: None,
            visibility: None,
            children: vec![Node::Column(ro_role), Node::Column(col_node("precision"))],
        }),
    ]);
    assert_eq!(
        surf.writable_columns(),
        BTreeSet::from([ColumnId::new("situation"), ColumnId::new("precision")])
    );
    assert_eq!(
        surf.writable_groups(),
        BTreeSet::from([GroupId::new("contacts")])
    );

    // Every column read-only in the group → the group is not writable.
    let mut ro_precision = col_node("precision");
    ro_precision.write = WritePolicy {
        writable: false,
        override_derived: false,
    };
    let mut ro_role = col_node("role");
    ro_role.write = WritePolicy {
        writable: false,
        override_derived: false,
    };
    let frozen_form = surface(vec![Node::Group(GroupNode {
        group: GroupId::new("contacts"),
        prompt: None,
        visibility: None,
        children: vec![Node::Column(ro_role), Node::Column(ro_precision)],
    })]);
    assert!(frozen_form.writable_columns().is_empty());
    assert!(frozen_form.writable_groups().is_empty());
}
