use std::collections::BTreeSet;

use varve_core::primitives::Decimal;
use varve_core::{ColumnId, GroupId, OptionId, ResolverId, RowPath};
use varve_impact::{
    BreakKind, ChangeClass, ColumnChange, RecordUnderAssessment, ResolverChange, RuleRef, assess,
    broken_rules, classify, classify_with_rules,
};
use varve_logic::{Atom, ColumnRef, Const, Expr, Operand};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, Mapping, NomenclatureRef, OptionRow,
    ResolverDeclaration, ResultField, ScalarType, Schema,
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
        column("widened", ScalarType::Integer(None), Arity::One),
        column("narrowed", ScalarType::Decimal(None), Arity::One),
        column("truncated", ScalarType::Datetime, Arity::One),
        column(
            "broken",
            ScalarType::Attachment(Default::default()),
            Arity::Many,
        ),
        column("moved", ScalarType::Text, Arity::One),
    ]);
    let to = schema(vec![
        column("kept", ScalarType::Text, Arity::One),
        column("new", ScalarType::Boolean, Arity::One),
        column("widened", ScalarType::Decimal(None), Arity::One),
        column("narrowed", ScalarType::Integer(None), Arity::One),
        column("truncated", ScalarType::Date, Arity::One),
        column("broken", ScalarType::Integer(None), Arity::One),
        Element::Group(Group {
            included_from: None,
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
    assert!(
        report.columns[&ColumnId::new("c")]
            .removed_options
            .is_empty()
    );
}

#[test]
fn unit_changes_are_named_in_the_report() {
    use varve_impact::UnitChange;
    use varve_schema::Unit;
    let int = |u: Option<Unit>| schema(vec![column("n", ScalarType::Integer(u), Arity::One)]);

    // Unit added: cast is free, but the report names the semantic change.
    let report = classify(&int(None), &int(Some(Unit::Day)), &Default::default()).unwrap();
    let impact = &report.columns[&ColumnId::new("n")];
    assert_eq!(impact.class, ChangeClass::Safe);
    assert_eq!(
        impact.unit_change,
        Some(UnitChange {
            from: None,
            to: Some(Unit::Day)
        })
    );

    // Unit removed: lossy (meaning dropped — §2.14/§5.5), named.
    let report = classify(&int(Some(Unit::Day)), &int(None), &Default::default()).unwrap();
    let impact = &report.columns[&ColumnId::new("n")];
    assert_eq!(impact.class, ChangeClass::Lossy);
    assert_eq!(
        impact.unit_change,
        Some(UnitChange {
            from: Some(Unit::Day),
            to: None
        })
    );

    // Within a dimension: checked, named.
    let report = classify(
        &int(Some(Unit::Metre)),
        &int(Some(Unit::Kilometre)),
        &Default::default(),
    )
    .unwrap();
    let impact = &report.columns[&ColumnId::new("n")];
    assert_eq!(impact.class, ChangeClass::Checked);
    assert!(impact.unit_change.is_some());

    // Across dimensions: breaking, named.
    let report = classify(
        &int(Some(Unit::Day)),
        &int(Some(Unit::Month)),
        &Default::default(),
    )
    .unwrap();
    let impact = &report.columns[&ColumnId::new("n")];
    assert_eq!(impact.class, ChangeClass::Breaking);
    assert!(impact.unit_change.is_some());

    // No unit anywhere: no noise.
    let report = classify(&int(None), &int(None), &Default::default()).unwrap();
    assert_eq!(report.columns[&ColumnId::new("n")].unit_change, None);
}

#[test]
fn attachment_constraint_changes_are_named() {
    use varve_impact::ConstraintChange;
    use varve_schema::AttachmentConstraints;
    let files = |constraints: AttachmentConstraints| {
        schema(vec![column(
            "piece",
            ScalarType::Attachment(constraints),
            Arity::Many,
        )])
    };
    let open = AttachmentConstraints::default();
    let pdf_only = AttachmentConstraints {
        accept: vec!["application/pdf".into()],
        max_bytes: Some(5_000_000),
    };

    // Narrowing: checked, and the report names both sides.
    let report = classify(
        &files(open.clone()),
        &files(pdf_only.clone()),
        &Default::default(),
    )
    .unwrap();
    let impact = &report.columns[&ColumnId::new("piece")];
    assert_eq!(impact.class, ChangeClass::Checked);
    assert_eq!(
        impact.constraint_change,
        Some(ConstraintChange {
            from: open.clone(),
            to: pdf_only.clone()
        })
    );

    // Broadening: safe, still named.
    let report = classify(&files(pdf_only), &files(open.clone()), &Default::default()).unwrap();
    let impact = &report.columns[&ColumnId::new("piece")];
    assert_eq!(impact.class, ChangeClass::Safe);
    assert!(impact.constraint_change.is_some());

    // Unchanged: no noise.
    let report = classify(&files(open.clone()), &files(open), &Default::default()).unwrap();
    assert_eq!(
        report.columns[&ColumnId::new("piece")].constraint_change,
        None
    );
}

#[test]
fn resolver_impact_questions() {
    let decl = |id: &str, version: u32, target: &str| ResolverDeclaration {
        id: ResolverId::new(id),
        version,
        anchor: GroupId::new("g"),
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
    let mut to = schema(columns.clone());
    to.resolvers = vec![decl("remapped", 1, "other"), decl("brand-new", 1, "fed")];

    let report = classify(&from, &to, &Default::default()).unwrap();
    assert!(report.resolvers.contains(&ResolverChange::Removed {
        anchor: GroupId::new("g"),
        id: ResolverId::new("removed"),
        orphaned_columns: vec![ColumnId::new("fed")],
    }));
    // Remapped: `other` is now fed (stale, re-derivable from retained
    // snapshots) and `fed` is no longer fed by it (orphaned by the remap).
    assert!(report.resolvers.contains(&ResolverChange::MappingChanged {
        anchor: GroupId::new("g"),
        id: ResolverId::new("remapped"),
        stale_columns: vec![ColumnId::new("other")],
        orphaned_columns: vec![ColumnId::new("fed")],
    }));
    assert!(report.resolvers.contains(&ResolverChange::Added {
        anchor: GroupId::new("g"),
        id: ResolverId::new("brand-new"),
    }));

    // Independent questions get independent answers: one declaration
    // that bumps its version, retypes its result and changes its input.
    let mut retyped = decl("r", 2, "fed");
    retyped.result_type = vec![ResultField {
        name: "value".into(),
        ty: ScalarType::Integer(None),
    }];
    retyped.input = vec![(ColumnId::new("other"), ScalarType::Text)];
    let mut from = schema(columns.clone());
    from.resolvers = vec![decl("r", 1, "fed")];
    let mut to = schema(columns);
    to.resolvers = vec![retyped];
    let report = classify(&from, &to, &Default::default()).unwrap();
    // The result field `value` is now an integer landing in text `fed`:
    // that mapping breaks.
    assert!(
        report
            .resolvers
            .contains(&ResolverChange::ResultTypeChanged {
                anchor: GroupId::new("g"),
                id: ResolverId::new("r"),
                broken_mappings: vec![Mapping {
                    result_field: "value".into(),
                    target: ColumnId::new("fed")
                }],
            })
    );
    assert!(report.resolvers.contains(&ResolverChange::InputChanged {
        anchor: GroupId::new("g"),
        id: ResolverId::new("r"),
    }));
    assert!(report.resolvers.contains(&ResolverChange::VersionChanged {
        anchor: GroupId::new("g"),
        id: ResolverId::new("r"),
        from: 1,
        to: 2,
    }));
    assert!(
        !report
            .resolvers
            .iter()
            .any(|c| matches!(c, ResolverChange::MappingChanged { .. }))
    );
}

#[test]
fn block_bumps_are_named() {
    // §2.1/Q5: inclusion pastes with provenance, so the impact report
    // can say "RIB block v1 → v2" and group the per-column casts under
    // it — instead of an anonymous retype inside some group.
    use varve_core::BlockId;
    use varve_schema::{Block, BlockRef, DepthPolicy};
    let rib = |version: u32, iban_ty: ScalarType| Block {
        id: BlockId::new("rib"),
        version,
        group: Group {
            id: GroupId::new("rib"),
            label: "RIB".into(),
            cardinality: Cardinality::One,
            children: vec![column("iban", iban_ty, Arity::One)],
            included_from: None,
        },
        resolvers: vec![],
    };
    let v1 = rib(1, ScalarType::Text);
    let v2 = rib(2, ScalarType::Integer(None));
    assert_eq!(v1.validate(DepthPolicy::default()), vec![]);
    let mut from = schema(vec![]);
    v1.include_into(&mut from, None).unwrap();
    let mut to = schema(vec![]);
    v2.include_into(&mut to, None).unwrap();

    let report = classify(&from, &to, &Default::default()).unwrap();
    assert_eq!(
        report.blocks,
        vec![varve_impact::BlockChange::Bumped {
            group: GroupId::new("rib"),
            from: BlockRef {
                id: BlockId::new("rib"),
                version: 1
            },
            to: BlockRef {
                id: BlockId::new("rib"),
                version: 2
            },
        }]
    );
    // The block's columns still get their §3 rows.
    assert_eq!(
        report.columns[&ColumnId::new("iban")].class,
        ChangeClass::Checked
    );

    // Hand-editing the included group drops the pin: detached.
    let mut detached = to.clone();
    if let Element::Group(g) = &mut detached.root[0] {
        g.included_from = None;
    }
    let report = classify(&to, &detached, &Default::default()).unwrap();
    assert!(matches!(
        report.blocks.as_slice(),
        [varve_impact::BlockChange::Detached { .. }]
    ));
    // Nothing block-related between two revisions without blocks.
    assert!(
        classify(&schema(vec![]), &schema(vec![]), &Default::default())
            .unwrap()
            .blocks
            .is_empty()
    );
}

#[test]
fn broken_rule_references_follow_the_section_4_1_taxonomy() {
    // A rule breaks when a source is removed (`not_available`), retyped
    // so the atom no longer typechecks (`incompatible`), an enum constant
    // names a removed option (`not_included`), or a projected field
    // disappears — the typechecker's verdicts against the new revision,
    // classified. Rules are the caller's to name: surfaces are above.
    let yes_no = || options(&[("oui", "Oui"), ("non", "Non")]);
    let from = schema(vec![
        column("gone", ScalarType::Boolean, Arity::One),
        column("retyped", ScalarType::Integer(None), Arity::One),
        column("choice", ScalarType::Enum(yes_no()), Arity::One),
        column("stable", ScalarType::Boolean, Arity::One),
    ]);
    let to = schema(vec![
        column("retyped", ScalarType::Text, Arity::One),
        column(
            "choice",
            ScalarType::Enum(options(&[("oui", "Oui")])),
            Arity::One,
        ),
        column("stable", ScalarType::Boolean, Arity::One),
    ]);
    let source = |id: &str| ColumnRef {
        column: ColumnId::new(id),
        field: None,
    };
    let eq = |id: &str, c: Const| {
        Expr::Atom(Atom::Eq {
            source: source(id),
            right: Operand::Const(c),
        })
    };
    let rule = |name: &str, expr: Expr| RuleRef {
        name: name.into(),
        scope: vec![],
        expr,
    };
    let rules = [
        rule("uses-gone", eq("gone", Const::Boolean(true))),
        rule(
            "uses-retyped",
            Expr::Atom(Atom::Gt {
                source: source("retyped"),
                right: Operand::Const(Const::Number {
                    value: Decimal::from_i64(3),
                    unit: None,
                }),
            }),
        ),
        rule(
            "uses-removed-option",
            eq("choice", Const::Option(OptionId::new("non"))),
        ),
        rule(
            "still-fine",
            Expr::And(vec![
                eq("stable", Const::Boolean(true)),
                eq("choice", Const::Option(OptionId::new("oui"))),
            ]),
        ),
        // Broken before and after: not the transition's doing.
        rule(
            "was-already-broken",
            eq("never-existed", Const::Boolean(true)),
        ),
    ];
    let broken = broken_rules(&rules, &from, &to, &Default::default());
    let by_name = |n: &str| broken.iter().find(|b| b.name == n).unwrap();
    assert_eq!(
        by_name("uses-gone").kinds,
        vec![BreakKind::SourceRemoved(ColumnId::new("gone"))]
    );
    assert!(!by_name("uses-gone").already_broken);
    assert_eq!(
        by_name("uses-retyped").kinds,
        vec![BreakKind::SourceRetyped(ColumnId::new("retyped"))]
    );
    assert_eq!(
        by_name("uses-removed-option").kinds,
        vec![BreakKind::OptionRemoved(
            ColumnId::new("choice"),
            OptionId::new("non")
        )]
    );
    assert!(broken.iter().all(|b| b.name != "still-fine"));
    assert!(by_name("was-already-broken").already_broken);
    assert!(matches!(
        by_name("was-already-broken").kinds.as_slice(),
        [BreakKind::Other(_)]
    ));

    // A newly broken rule makes the transition breaking; an already
    // broken one does not.
    let report = classify_with_rules(&from, &to, &Default::default(), &rules).unwrap();
    assert_eq!(report.rules.len(), 4);
    assert_eq!(report.worst(), ChangeClass::Breaking);
    let report = classify_with_rules(&from, &to, &Default::default(), &rules[4..]).unwrap();
    assert_ne!(report.worst(), ChangeClass::Breaking);
}

#[test]
fn assessment_turns_checked_into_exact_counts() {
    let from = schema(vec![column("d", ScalarType::Decimal(None), Arity::One)]);
    let to = schema(vec![column("d", ScalarType::Integer(None), Arity::One)]);

    let record = |s: &str| {
        let mut v = RecordValues::new();
        v.cells.insert(
            addr("d"),
            CellState::Value(CellValue::One(Scalar::Decimal(Decimal::parse(s).unwrap()))),
        );
        v
    };
    let records = [record("42"), record("1.5"), record("7"), record("0.25")];
    let none = BTreeSet::new();
    let under: Vec<RecordUnderAssessment<'_>> = records
        .iter()
        .map(|v| RecordUnderAssessment {
            values: v,
            pending: &none,
        })
        .collect();

    let report = assess(&from, &to, &Default::default(), &[], under).unwrap();
    let assessment = report.records.as_ref().unwrap();
    assert_eq!(assessment.records, 4);
    // Exactly the fractional ones fail — the §7 count, per column.
    assert_eq!(assessment.records_with_failures, 2);
    assert_eq!(assessment.cells_failed, 2);
    assert_eq!(assessment.failed_by_column[&ColumnId::new("d")], 2);
    // Static class said Checked; the data decided.
    assert_eq!(report.worst(), ChangeClass::Checked);
}

#[test]
fn assessment_counts_uncastable_cells_and_pending_on_removed_resolvers() {
    // A breaking column change comes with its blast radius: the records
    // whose cells have nowhere to go. And a removed resolver comes with
    // the records whose pending resolutions can never land (§2.8).
    let decl = ResolverDeclaration {
        id: ResolverId::new("insee"),
        version: 1,
        anchor: GroupId::new("g"),
        input: vec![(ColumnId::new("siret"), ScalarType::Text)],
        result_type: vec![ResultField {
            name: "name".into(),
            ty: ScalarType::Text,
        }],
        mapping: vec![Mapping {
            result_field: "name".into(),
            target: ColumnId::new("name"),
        }],
    };
    let mut from = schema(vec![
        column("siret", ScalarType::Text, Arity::One),
        column("name", ScalarType::Text, Arity::One),
        column("g", ScalarType::Geometry, Arity::One),
    ]);
    from.resolvers = vec![decl];
    let to = schema(vec![
        column("siret", ScalarType::Text, Arity::One),
        column("name", ScalarType::Text, Arity::One),
        column("g", ScalarType::Integer(None), Arity::One), // no cast exists
    ]);
    let feature = varve_value::Feature::parse(
        r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[1,2]},"properties":null}"#,
    )
    .unwrap();
    let with_geometry = {
        let mut v = RecordValues::new();
        v.cells.insert(
            addr("g"),
            CellState::Value(CellValue::One(Scalar::Geometry(Box::new(feature)))),
        );
        v
    };
    let without = RecordValues::new();
    let pending_insee: BTreeSet<(GroupId, ResolverId)> =
        [(GroupId::new("g"), ResolverId::new("insee"))]
            .into_iter()
            .collect();
    let none = BTreeSet::new();
    let records = [
        RecordUnderAssessment {
            values: &with_geometry,
            pending: &pending_insee,
        },
        RecordUnderAssessment {
            values: &without,
            pending: &pending_insee,
        },
        RecordUnderAssessment {
            values: &with_geometry,
            pending: &none,
        },
    ];
    let report = assess(&from, &to, &Default::default(), &[], records).unwrap();
    assert_eq!(
        report.columns[&ColumnId::new("g")].change,
        ColumnChange::Forbidden
    );
    let a = report.records.as_ref().unwrap();
    // The projection drops those cells (nothing failed to cast — there
    // is no cast), so they are counted as uncastable, not failed.
    assert_eq!(a.cells_failed, 0);
    assert_eq!(a.records_with_uncastable, 2);
    assert_eq!(a.uncastable_by_column[&ColumnId::new("g")], 2);
    assert_eq!(
        a.pending_on_removed_resolvers[&(GroupId::new("g"), ResolverId::new("insee"))],
        2
    );
    assert_eq!(report.worst(), ChangeClass::Breaking);
}
