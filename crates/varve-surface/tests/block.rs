//! Blocks: the "RIB" example from §2.15 and the SIRET pattern from
//! §2.7, assembled — self-contained, content-addressed, pasted on
//! inclusion so nothing downstream knows.

use std::collections::BTreeSet;

use varve_core::{BlockId, ColumnId, GroupId, RevisionId, SurfaceId};
use varve_logic::{Atom, ColumnRef, Expr};
use varve_schema::{
    Arity, AttachmentConstraints, Cardinality, Column, Element, Group, Mapping,
    ResolverDeclaration, ResultField, ScalarType, Schema, revision_id,
};
use varve_surface::{
    Block, BlockError, ColumnNode, Format, GroupNode, Node, Surface, WritePolicy,
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

/// The RIB block: an IBAN text column with a format, plus a
/// justificatif attachment restricted to PDF/images ≤ 5 MB.
fn rib_block() -> Block {
    let mut iban = col_node("iban");
    iban.prompt = Some("IBAN".into());
    iban.format = Some(Format::Iban);
    iban.required = Some(Expr::And(vec![]));
    let mut justificatif = col_node("justificatif");
    justificatif.prompt = Some("Justificatif de RIB".into());
    // Required once the IBAN is filled — a rule over block columns only.
    justificatif.required = Some(Expr::Atom(Atom::IsFilled {
        source: ColumnRef { column: ColumnId::new("iban"), field: None },
    }));
    Block {
        id: BlockId::new("rib"),
        version: 1,
        shell: Group {
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
        },
        resolvers: vec![],
        defaults: GroupNode {
            group: GroupId::new("rib"),
            prompt: Some("Coordonnées bancaires".into()),
            visibility: None,
            children: vec![Node::Column(iban), Node::Column(justificatif)],
        },
    }
}

#[test]
fn rib_block_validates_and_is_content_addressed() {
    let block = rib_block();
    assert_eq!(block.validate(&Default::default()), vec![]);
    let id = block.content_id();
    // Same content → same id; a changed default rule → new id (rules
    // pin to the block version — Q5).
    assert_eq!(rib_block().content_id(), id);
    let mut edited = rib_block();
    if let Node::Column(c) = &mut edited.defaults.children[1] {
        c.required = None;
    }
    assert_ne!(edited.content_id(), id);
}

#[test]
fn block_must_be_self_contained() {
    let noms = Default::default();
    // Defaults naming a column the shell does not own.
    let mut block = rib_block();
    block.defaults.children.push(Node::Column(col_node("outsider")));
    assert!(block
        .validate(&noms)
        .iter()
        .any(|e| matches!(e, BlockError::ForeignColumn(c) if c == &ColumnId::new("outsider"))));

    // A rule reading a column outside the block: it would mean
    // different things in different inclusions.
    let mut block = rib_block();
    if let Node::Column(c) = &mut block.defaults.children[0] {
        c.visibility = Some(Expr::Atom(Atom::IsFilled {
            source: ColumnRef { column: ColumnId::new("elsewhere"), field: None },
        }));
    }
    assert!(block
        .validate(&noms)
        .iter()
        .any(|e| matches!(e, BlockError::ForeignRuleSource(..))));

    // A resolver mapping into a foreign column.
    let mut block = rib_block();
    block.resolvers.push(ResolverDeclaration {
        id: varve_core::ResolverId::new("x"),
        version: 1,
        input: vec![(ColumnId::new("iban"), ScalarType::Text)],
        result_type: vec![ResultField { name: "bic".into(), ty: ScalarType::Text }],
        mapping: vec![Mapping { result_field: "bic".into(), target: ColumnId::new("elsewhere") }],
    });
    assert!(block
        .validate(&noms)
        .iter()
        .any(|e| matches!(e, BlockError::ForeignResolverColumn(_))));

    // Halves disagreeing on the group id.
    let mut block = rib_block();
    block.defaults.group = GroupId::new("other");
    assert!(block
        .validate(&noms)
        .iter()
        .any(|e| matches!(e, BlockError::GroupMismatch(..))));
}

#[test]
fn inclusion_pastes_and_nothing_downstream_knows() {
    let block = rib_block();
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
    block.include(&mut schema, &mut surface, None).unwrap();
    surface.revision = revision_id(&schema);

    // The included schema and surface validate as ordinary ones.
    assert_eq!(
        varve_schema::validate(&schema, varve_schema::DepthPolicy::default()),
        vec![]
    );
    assert_eq!(validate(&surface, &schema, &Default::default()), vec![]);
    // And admissibility runs the block's default rules: on a pristine
    // record the IBAN (always required) is missing; the justificatif
    // (required only once IBAN is filled) is not.
    let report = admissibility(&surface, &schema, &Default::default(), &RecordValues::new(), &BTreeSet::new())
        .unwrap();
    assert_eq!(report.findings.len(), 1);
    assert!(matches!(
        &report.findings[0],
        varve_surface::Finding::MissingRequired { column, .. } if column == &ColumnId::new("iban")
    ));
    // The block is visible to tooling that asks, invisible otherwise.
    let registry = [rib_block()];
    let included = varve_surface::included_blocks(&schema, &registry);
    assert_eq!(included.len(), 1);
    assert_eq!(included[0].1, GroupId::new("rib"));
}
