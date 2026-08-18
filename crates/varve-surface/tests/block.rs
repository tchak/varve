//! Blocks, both halves: the schema-side `varve_schema::Block` (shell +
//! declarations) and the surface-side `BlockDefaults` (rules, prompts,
//! formats) — the "RIB" example from §2.15 — self-contained, content-
//! addressed, pasted on inclusion so nothing downstream knows, and
//! pasted *with provenance* so the revision knows.

use std::collections::BTreeSet;

use varve_core::{BlockId, ColumnId, GroupId, RevisionId, SurfaceId};
use varve_logic::{Atom, ColumnRef, Expr};
use varve_schema::{
    Arity, AttachmentConstraints, Block, BlockRef, Cardinality, Column, DepthPolicy, Element,
    Group, Mapping, ResolverDeclaration, ResultField, ScalarType, Schema, included_blocks,
    revision_id,
};
use varve_surface::{
    BlockDefaults, BlockDefaultsError, ColumnNode, Format, GroupNode, Node, Surface, WritePolicy,
    admissibility, validate,
};
use varve_value::RecordValues;

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

/// The RIB block's schema half: an IBAN text column plus a justificatif
/// attachment restricted to PDF/images ≤ 5 MB.
fn rib_block() -> Block {
    Block {
        id: BlockId::new("rib"),
        version: 1,
        group: Group {
            id: GroupId::new("rib"),
            label: "RIB".into(),
            cardinality: Cardinality::One,
            children: vec![
                Element::Column(Column {
                    id: ColumnId::new("iban"),
                    label: "IBAN".into(),
                    ty: ScalarType::Text,
                    arity: Arity::One,
                }),
                Element::Column(Column {
                    id: ColumnId::new("justificatif"),
                    label: "Justificatif".into(),
                    ty: ScalarType::Attachment(AttachmentConstraints {
                        accept: vec!["application/pdf".into(), "image/*".into()],
                        max_bytes: Some(5_000_000),
                    }),
                    arity: Arity::Many,
                }),
            ],
            included_from: None,
        },
        resolvers: vec![],
    }
}

/// Its surface half: prompts, an IBAN format, "always required" IBAN and
/// a justificatif required once the IBAN is filled.
fn rib_defaults() -> BlockDefaults {
    let mut iban = col_node("iban");
    iban.prompt = Some("IBAN".into());
    iban.format = Some(Format::Iban);
    iban.required = Some(Expr::And(vec![]));
    let mut justificatif = col_node("justificatif");
    justificatif.prompt = Some("Justificatif de RIB".into());
    justificatif.required = Some(Expr::Atom(Atom::IsFilled {
        source: ColumnRef { column: ColumnId::new("iban"), field: None },
    }));
    BlockDefaults {
        block: BlockRef { id: BlockId::new("rib"), version: 1 },
        node: GroupNode {
            group: GroupId::new("rib"),
            prompt: Some("Coordonnées bancaires".into()),
            visibility: None,
            children: vec![Node::Column(iban), Node::Column(justificatif)],
        },
    }
}

#[test]
fn both_halves_validate_and_are_content_addressed() {
    let block = rib_block();
    let defaults = rib_defaults();
    assert_eq!(block.validate(DepthPolicy::default()), vec![]);
    assert_eq!(defaults.validate(&block, &Default::default()), vec![]);
    // Same content → same hash; a changed default rule → new hash (rules
    // pin to the block version — Q5); the shell hash is untouched by
    // the surface half.
    assert_eq!(rib_block().content_hash(), block.content_hash());
    let mut edited = rib_defaults();
    if let Node::Column(c) = &mut edited.node.children[1] {
        c.required = None;
    }
    assert_ne!(edited.content_hash(), defaults.content_hash());
    // A format contributes its own canonical shape (never Debug).
    let mut formatted = rib_defaults();
    if let Node::Column(c) = &mut formatted.node.children[0] {
        c.format = Some(Format::Regex("[A-Z]{2}\\d+".into()));
    }
    assert_ne!(formatted.content_hash(), defaults.content_hash());
}

#[test]
fn both_halves_must_be_self_contained() {
    let noms = Default::default();
    // Defaults naming a column the shell does not own.
    let mut defaults = rib_defaults();
    defaults.node.children.push(Node::Column(col_node("outsider")));
    assert!(defaults
        .validate(&rib_block(), &noms)
        .iter()
        .any(|e| matches!(e, BlockDefaultsError::ForeignColumn(c) if c == &ColumnId::new("outsider"))));

    // A rule reading a column outside the block: it would mean
    // different things in different inclusions.
    let mut defaults = rib_defaults();
    if let Node::Column(c) = &mut defaults.node.children[0] {
        c.visibility = Some(Expr::Atom(Atom::IsFilled {
            source: ColumnRef { column: ColumnId::new("elsewhere"), field: None },
        }));
    }
    assert!(defaults
        .validate(&rib_block(), &noms)
        .iter()
        .any(|e| matches!(e, BlockDefaultsError::ForeignRuleSource(..))));

    // Defaults for another block, or another version.
    let mut defaults = rib_defaults();
    defaults.block.version = 2;
    assert!(defaults
        .validate(&rib_block(), &noms)
        .iter()
        .any(|e| matches!(e, BlockDefaultsError::WrongBlock(..))));

    // Halves disagreeing on the group id.
    let mut defaults = rib_defaults();
    defaults.node.group = GroupId::new("other");
    assert!(defaults
        .validate(&rib_block(), &noms)
        .iter()
        .any(|e| matches!(e, BlockDefaultsError::GroupMismatch(..))));

    // Schema side: a resolver mapping into a foreign column.
    let mut block = rib_block();
    block.resolvers.push(ResolverDeclaration {
        id: varve_core::ResolverId::new("x"),
        version: 1,
        anchor: GroupId::new("rib"),
        input: vec![(ColumnId::new("iban"), ScalarType::Text)],
        result_type: vec![ResultField { name: "bic".into(), ty: ScalarType::Text }],
        mapping: vec![Mapping { result_field: "bic".into(), target: ColumnId::new("elsewhere") }],
    });
    assert!(block
        .validate(DepthPolicy::default())
        .iter()
        .any(|e| matches!(e, varve_schema::BlockError::ForeignResolverColumn(_))));
}

#[test]
fn inclusion_pastes_with_provenance_and_nothing_downstream_knows() {
    let block = rib_block();
    let defaults = rib_defaults();
    let mut schema = Schema {
        root: vec![Element::Column(Column {
            id: ColumnId::new("nom"),
            label: "Nom".into(),
            ty: ScalarType::Text,
            arity: Arity::One,
        })],
        resolvers: vec![],
    };
    let mut surface = Surface {
        id: SurfaceId::new("public"),
        revision: RevisionId::new("pending"),
        nodes: vec![Node::Column(col_node("nom"))],
        ineligibility: None,
    };
    block.include_into(&mut schema, None).unwrap();
    defaults.include_into(&mut surface, None).unwrap();
    surface.revision = revision_id(&schema);

    // The included schema and surface validate as ordinary ones.
    assert_eq!(varve_schema::validate(&schema, DepthPolicy::default()), vec![]);
    assert_eq!(validate(&surface, &schema, &Default::default()), vec![]);
    // And admissibility runs the block's default rules: on a pristine
    // record the IBAN (always required) is missing; the justificatif
    // (required only once IBAN is filled) is not.
    let report =
        admissibility(&surface, &schema, &Default::default(), &RecordValues::new(), &BTreeSet::new())
            .unwrap();
    assert_eq!(report.findings.len(), 1);
    assert!(matches!(
        &report.findings[0],
        varve_surface::Finding::MissingRequired { column, .. } if column == &ColumnId::new("iban")
    ));
    // The revision knows what it included — by block *and version* —
    // without a registry (that is what lets rules pin to a version and
    // the impact report name a bump).
    assert_eq!(
        included_blocks(&schema),
        vec![(GroupId::new("rib"), BlockRef { id: BlockId::new("rib"), version: 1 })]
    );
    // Provenance is identity-bearing: the same structure typed by hand
    // is a different revision.
    let mut by_hand = schema.clone();
    if let Element::Group(g) = &mut by_hand.root[1] {
        g.included_from = None;
    }
    assert_ne!(revision_id(&by_hand), revision_id(&schema));
    // Round-trips through the canonical form.
    assert_eq!(
        varve_schema::schema_from_canonical(&varve_schema::schema_canonical(&schema)).unwrap(),
        schema
    );

    // Including twice, or into a missing container, is refused before
    // anything is touched.
    let before = schema.clone();
    assert_eq!(
        block.include_into(&mut schema, None),
        Err(varve_schema::IncludeError::DuplicateGroup(GroupId::new("rib")))
    );
    assert_eq!(schema, before);
    let mut fresh = Schema::default();
    assert_eq!(
        block.include_into(&mut fresh, Some(&GroupId::new("nope"))),
        Err(varve_schema::IncludeError::UnknownContainer(GroupId::new("nope")))
    );
    assert_eq!(fresh, Schema::default());
    let before = surface.clone();
    assert_eq!(
        defaults.include_into(&mut surface, None),
        Err(varve_surface::IncludeError::DuplicateGroup(GroupId::new("rib")))
    );
    assert_eq!(surface, before);
}
