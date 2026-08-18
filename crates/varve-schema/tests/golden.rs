//! Golden content addresses (§2.13). The revision id and the block
//! hash *are* the canonical bytes; these literals pin the canonical
//! shape so that any drift — a renamed key, a reordered field, a
//! changed number rendering — fails here instead of silently
//! re-identifying every published revision. They may only change with a
//! deliberate format change (and then every stored id changes with them).

use varve_core::{BlockId, ColumnId, GroupId, NomenclatureId, OptionId, ResolverId};
use varve_schema::{
    Arity, AttachmentConstraints, Block, BlockRef, Cardinality, Column, Element, Group, Mapping,
    NomenclatureRef, OptionRow, ResolverDeclaration, ResultField, ScalarType, Schema, Unit,
    revision_id,
};

fn column(id: &str, ty: ScalarType, arity: Arity) -> Element {
    Element::Column(Column { id: ColumnId::new(id), label: format!("Label {id}"), ty, arity })
}

fn row(id: &str, label: &str, fields: &[(&str, &str)]) -> OptionRow {
    OptionRow {
        id: OptionId::new(id),
        label: label.into(),
        fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

/// Every scalar type, both arities, a `one` group holding a nested
/// `many` group with block provenance, and a resolver declaration.
fn kitchen_sink() -> Schema {
    Schema {
        root: vec![
            column("nom", ScalarType::Text, Arity::One),
            column("accord", ScalarType::Boolean, Arity::One),
            column("nombre", ScalarType::Integer(None), Arity::One),
            column("duree", ScalarType::Integer(Some(Unit::Month)), Arity::One),
            column("montant", ScalarType::Decimal(None), Arity::One),
            column("surface", ScalarType::Decimal(Some(Unit::Hectare)), Arity::Many),
            column("date", ScalarType::Date, Arity::One),
            column("instant", ScalarType::Datetime, Arity::One),
            column(
                "situation",
                ScalarType::Enum(NomenclatureRef::Inline(vec![
                    row("oui", "Oui", &[]),
                    row("non", "Non", &[("code", "N")]),
                ])),
                Arity::One,
            ),
            column(
                "commune",
                ScalarType::Enum(NomenclatureRef::Published { id: NomenclatureId::new("cog"), version: 3 }),
                Arity::Many,
            ),
            column("piece", ScalarType::Attachment(AttachmentConstraints::default()), Arity::One),
            column(
                "photos",
                ScalarType::Attachment(AttachmentConstraints {
                    accept: vec!["image/*".into(), "application/pdf".into()],
                    max_bytes: Some(10_000_000),
                }),
                Arity::Many,
            ),
            column("parcelle", ScalarType::Geometry, Arity::Many),
            Element::Group(Group {
                id: GroupId::new("identite"),
                label: "Identité".into(),
                cardinality: Cardinality::One,
                included_from: None,
                children: vec![
                    column("prenom", ScalarType::Text, Arity::One),
                    Element::Group(Group {
                        id: GroupId::new("rib"),
                        label: "RIB".into(),
                        cardinality: Cardinality::Many,
                        included_from: Some(BlockRef { id: BlockId::new("rib"), version: 2 }),
                        children: vec![
                            column("iban", ScalarType::Text, Arity::One),
                            column("bic", ScalarType::Text, Arity::One),
                        ],
                    }),
                ],
            }),
        ],
        resolvers: vec![ResolverDeclaration {
            id: ResolverId::new("insee-sirene"),
            version: 4,
            anchor: GroupId::new("identite"),
            input: vec![(ColumnId::new("nom"), ScalarType::Text)],
            result_type: vec![
                ResultField { name: "raison_sociale".into(), ty: ScalarType::Text },
                ResultField { name: "effectif".into(), ty: ScalarType::Integer(None) },
            ],
            mapping: vec![Mapping { result_field: "raison_sociale".into(), target: ColumnId::new("nom") }],
        }],
    }
}

fn rib_block() -> Block {
    Block {
        id: BlockId::new("rib"),
        version: 2,
        group: Group {
            id: GroupId::new("rib"),
            label: "RIB".into(),
            cardinality: Cardinality::One,
            included_from: None,
            children: vec![
                column("iban", ScalarType::Text, Arity::One),
                column("bic", ScalarType::Text, Arity::One),
            ],
        },
        resolvers: vec![ResolverDeclaration {
            id: ResolverId::new("bic-lookup"),
            version: 1,
            anchor: GroupId::new("rib"),
            input: vec![(ColumnId::new("iban"), ScalarType::Text)],
            result_type: vec![ResultField { name: "bic".into(), ty: ScalarType::Text }],
            mapping: vec![Mapping { result_field: "bic".into(), target: ColumnId::new("bic") }],
        }],
    }
}

#[test]
fn revision_id_of_the_empty_schema_is_pinned() {
    assert_eq!(
        revision_id(&Schema::default()).as_str(),
        "sha256:3aa2c62dc77fb053f50d9418cd5017de14939648aa02563d47c39972e288040d",
    );
}

#[test]
fn revision_id_of_the_kitchen_sink_schema_is_pinned() {
    assert_eq!(
        revision_id(&kitchen_sink()).as_str(),
        // Repinned 2026-08-18: resolver declarations gained the identity-
        // bearing `anchor` field (§10 Q17) — a deliberate canonical
        // change, made while no record hashes exist.
        "sha256:ca7d6cf075a594f73bc6cf512408c9eb6690aa771ec73c15d6a0fb893a1e4b15",
    );
}

#[test]
fn block_content_hash_is_pinned() {
    assert_eq!(
        rib_block().content_hash().to_string(),
        // Repinned 2026-08-18, same cause as the revision id above.
        "sha256:e8c388aba5453e82d04baa870774309ebe4deba4ce15dcaf715a6c7a1214cdc7",
    );
}
