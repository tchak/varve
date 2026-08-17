//! Canonical form and content hash of a schema: the revision identity
//! (§2.13 decision 7, plain regime — identical schemas converge on
//! identical ids on every instance). Identity-bearing: types, arity,
//! cardinality, element order, labels, inline nomenclature rows
//! (a relabel is a new revision, §2.11), units, resolver declarations.
//! Not identity-bearing: surfaces (separate objects).

use varve_core::RevisionId;
use varve_core::canonical::{CanonicalValue, hash_plain};

use crate::{
    Arity, Cardinality, Element, NomenclatureRef, OptionRow, ResolverDeclaration,
    ScalarType, Schema,
};

fn obj(pairs: Vec<(&str, CanonicalValue)>) -> CanonicalValue {
    CanonicalValue::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn string(s: impl ToString) -> CanonicalValue {
    CanonicalValue::String(s.to_string())
}

fn array<T>(items: &[T], f: impl Fn(&T) -> CanonicalValue) -> CanonicalValue {
    CanonicalValue::Array(items.iter().map(f).collect())
}

pub fn schema_canonical(schema: &Schema) -> CanonicalValue {
    obj(vec![
        ("elements", array(&schema.root, element)),
        ("resolvers", array(&schema.resolvers, resolver)),
    ])
}

/// The revision id: hash of the canonical bytes (§2.13). Deterministic
/// and instance-independent by construction.
pub fn revision_id(schema: &Schema) -> RevisionId {
    let hash = hash_plain(&schema_canonical(schema)).expect("schemas contain no floats");
    RevisionId::new(hash.to_string())
}

fn element(el: &Element) -> CanonicalValue {
    match el {
        Element::Column(c) => obj(vec![(
            "column",
            obj(vec![
                ("id", string(&c.id)),
                ("label", string(&c.label)),
                ("type", scalar_type(&c.ty)),
                (
                    "arity",
                    string(match c.arity {
                        Arity::One => "one",
                        Arity::Many => "many",
                    }),
                ),
            ]),
        )]),
        Element::Group(g) => obj(vec![(
            "group",
            obj(vec![
                ("id", string(&g.id)),
                ("label", string(&g.label)),
                (
                    "cardinality",
                    string(match g.cardinality {
                        Cardinality::One => "one",
                        Cardinality::Many => "many",
                    }),
                ),
                ("children", array(&g.children, element)),
            ]),
        )]),
    }
}

fn scalar_type(ty: &ScalarType) -> CanonicalValue {
    let simple = |kind: &str| obj(vec![("kind", string(kind))]);
    match ty {
        ScalarType::Text => simple("text"),
        ScalarType::Boolean => simple("boolean"),
        ScalarType::Date => simple("date"),
        ScalarType::Datetime => simple("datetime"),
        // §2.15: constraints are identity-bearing — narrowing the
        // accept set is a revision.
        ScalarType::Attachment(constraints) => {
            let mut pairs = vec![("kind", string("attachment"))];
            if !constraints.accept.is_empty() {
                pairs.push((
                    "accept",
                    CanonicalValue::Array(
                        constraints.accept.iter().map(string).collect(),
                    ),
                ));
            }
            if let Some(max) = constraints.max_bytes {
                pairs.push(("max_bytes", CanonicalValue::Int(max as i64)));
            }
            obj(pairs)
        }
        ScalarType::Geometry => simple("geometry"),
        ScalarType::Integer(unit) | ScalarType::Decimal(unit) => {
            let kind = if matches!(ty, ScalarType::Integer(_)) {
                "integer"
            } else {
                "decimal"
            };
            let mut pairs = vec![("kind", string(kind))];
            if let Some(unit) = unit {
                pairs.push(("unit", string(unit.name())));
            }
            obj(pairs)
        }
        ScalarType::Enum(nref) => obj(vec![
            ("kind", string("enum")),
            (
                "nomenclature",
                match nref {
                    NomenclatureRef::Inline(rows) => {
                        obj(vec![("inline", array(rows, option_row))])
                    }
                    NomenclatureRef::Published { id, version } => obj(vec![(
                        "published",
                        obj(vec![
                            ("id", string(id)),
                            ("version", CanonicalValue::Int(i64::from(*version))),
                        ]),
                    )]),
                },
            ),
        ]),
    }
}

fn option_row(row: &OptionRow) -> CanonicalValue {
    obj(vec![
        ("id", string(&row.id)),
        ("label", string(&row.label)),
        (
            "fields",
            CanonicalValue::Array(
                row.fields
                    .iter()
                    .map(|(k, v)| CanonicalValue::Array(vec![string(k), string(v)]))
                    .collect(),
            ),
        ),
    ])
}

fn resolver(decl: &ResolverDeclaration) -> CanonicalValue {
    obj(vec![
        ("id", string(&decl.id)),
        ("version", CanonicalValue::Int(i64::from(decl.version))),
        (
            "input",
            CanonicalValue::Array(
                decl.input
                    .iter()
                    .map(|(column, ty)| {
                        CanonicalValue::Array(vec![string(column), scalar_type(ty)])
                    })
                    .collect(),
            ),
        ),
        (
            "result",
            CanonicalValue::Array(
                decl.result_type
                    .iter()
                    .map(|f| obj(vec![("name", string(&f.name)), ("type", scalar_type(&f.ty))]))
                    .collect(),
            ),
        ),
        (
            "mapping",
            CanonicalValue::Array(
                decl.mapping
                    .iter()
                    .map(|m| {
                        obj(vec![
                            ("field", string(&m.result_field)),
                            ("target", string(&m.target)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}
