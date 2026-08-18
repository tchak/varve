//! Round-trip law for the schema canonical form (§2.13, §5): over
//! generated schemas covering every scalar type, both arities, `one`
//! and `many` groups nested two deep, block provenance and resolver
//! declarations, `from ∘ to` and `to ∘ from` are identities, and the
//! revision id is a function of the schema and nothing else.

use proptest::prelude::*;
use varve_core::canonical::{CanonicalValue, canonical_bytes};
use varve_core::{BlockId, ColumnId, GroupId, NomenclatureId, OptionId, ResolverId};
use varve_schema::{
    Arity, AttachmentConstraints, Block, BlockRef, Cardinality, Column, Element, Group, Mapping,
    NomenclatureRef, OptionRow, ResolverDeclaration, ResultField, ScalarType, Schema, Unit,
    block_canonical, block_from_canonical, option_row_canonical, option_row_from_canonical,
    revision_id, scalar_type_from_canonical, schema_canonical, schema_from_canonical,
};

fn ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,7}"
}

fn label() -> impl Strategy<Value = String> {
    "\\PC{0,12}"
}

fn unit() -> impl Strategy<Value = Option<Unit>> {
    proptest::option::of(prop_oneof![
        Just(Unit::Millimetre),
        Just(Unit::Metre),
        Just(Unit::Kilometre),
        Just(Unit::Gram),
        Just(Unit::Tonne),
        Just(Unit::Minute),
        Just(Unit::Day),
        Just(Unit::Week),
        Just(Unit::Month),
        Just(Unit::Year),
        Just(Unit::Hectare),
        Just(Unit::Litre),
        Just(Unit::CubicMetre),
        Just(Unit::Percent),
    ])
}

fn option_row() -> impl Strategy<Value = OptionRow> {
    (
        ident(),
        label(),
        proptest::collection::vec((ident(), label()), 0..3),
    )
        .prop_map(|(id, label, fields)| OptionRow { id: OptionId::new(id), label, fields })
}

fn nomenclature_ref() -> impl Strategy<Value = NomenclatureRef> {
    prop_oneof![
        proptest::collection::vec(option_row(), 0..4).prop_map(NomenclatureRef::Inline),
        (ident(), 0u32..1000).prop_map(|(id, version)| NomenclatureRef::Published {
            id: NomenclatureId::new(id),
            version,
        }),
    ]
}

/// Accept lists in **normalized** form (lowercase, sorted, deduplicated,
/// no `*/*`): the canonical form commits to the normalized set, so this
/// is the domain on which decoding is the exact inverse. The
/// unnormalized case is its own law below.
fn normalized_constraints() -> impl Strategy<Value = AttachmentConstraints> {
    (
        proptest::sample::subsequence(
            vec!["application/pdf", "image/*", "image/jpeg", "image/png", "text/csv"],
            0..=5,
        ),
        proptest::option::of(0u64..(1 << 53)),
    )
        .prop_map(|(accept, max_bytes)| AttachmentConstraints {
            accept: accept.into_iter().map(str::to_string).collect(),
            max_bytes,
        })
}

fn scalar_type() -> impl Strategy<Value = ScalarType> {
    prop_oneof![
        Just(ScalarType::Text),
        Just(ScalarType::Boolean),
        unit().prop_map(ScalarType::Integer),
        unit().prop_map(ScalarType::Decimal),
        Just(ScalarType::Date),
        Just(ScalarType::Datetime),
        nomenclature_ref().prop_map(ScalarType::Enum),
        normalized_constraints().prop_map(ScalarType::Attachment),
        Just(ScalarType::Geometry),
    ]
}

fn arity() -> impl Strategy<Value = Arity> {
    prop_oneof![Just(Arity::One), Just(Arity::Many)]
}

fn column() -> impl Strategy<Value = Element> {
    (ident(), label(), scalar_type(), arity()).prop_map(|(id, label, ty, arity)| {
        Element::Column(Column { id: ColumnId::new(id), label, ty, arity })
    })
}

fn block_ref() -> impl Strategy<Value = BlockRef> {
    (ident(), 0u32..100).prop_map(|(id, version)| BlockRef { id: BlockId::new(id), version })
}

fn group(children: impl Strategy<Value = Vec<Element>>) -> impl Strategy<Value = Group> {
    (
        ident(),
        label(),
        prop_oneof![Just(Cardinality::One), Just(Cardinality::Many)],
        children,
        proptest::option::of(block_ref()),
    )
        .prop_map(|(id, label, cardinality, children, included_from)| Group {
            id: GroupId::new(id),
            label,
            cardinality,
            children,
            included_from,
        })
}

/// Elements nested up to two group levels deep.
fn element() -> impl Strategy<Value = Element> {
    let leaf = column();
    let depth1 = prop_oneof![
        column(),
        group(proptest::collection::vec(leaf, 0..3)).prop_map(Element::Group),
    ];
    prop_oneof![
        column(),
        group(proptest::collection::vec(depth1, 0..3)).prop_map(Element::Group),
    ]
}

fn resolver() -> impl Strategy<Value = ResolverDeclaration> {
    (
        ident(),
        0u32..100,
        ident(),
        proptest::collection::vec((ident(), scalar_type()), 0..3),
        proptest::collection::vec((ident(), scalar_type()), 0..3),
        proptest::collection::vec((ident(), ident()), 0..3),
    )
        .prop_map(|(id, version, anchor, input, result, mapping)| ResolverDeclaration {
            id: ResolverId::new(id),
            version,
            anchor: GroupId::new(anchor),
            input: input.into_iter().map(|(c, ty)| (ColumnId::new(c), ty)).collect(),
            result_type: result
                .into_iter()
                .map(|(name, ty)| ResultField { name, ty })
                .collect(),
            mapping: mapping
                .into_iter()
                .map(|(field, target)| Mapping { result_field: field, target: ColumnId::new(target) })
                .collect(),
        })
}

fn schema() -> impl Strategy<Value = Schema> {
    (
        proptest::collection::vec(element(), 0..5),
        proptest::collection::vec(resolver(), 0..3),
    )
        .prop_map(|(root, resolvers)| Schema { root, resolvers })
}

fn block() -> impl Strategy<Value = Block> {
    (
        ident(),
        0u32..100,
        group(proptest::collection::vec(element(), 0..3)),
        proptest::collection::vec(resolver(), 0..3),
    )
        .prop_map(|(id, version, group, resolvers)| Block {
            id: BlockId::new(id),
            version,
            group,
            resolvers,
        })
}

fn bytes(v: &CanonicalValue) -> Vec<u8> {
    canonical_bytes(v).expect("schemas contain no floats")
}

proptest! {
    /// THE law: `schema_from_canonical ∘ schema_canonical` is the
    /// identity, and the canonical form is a fixed point of a decode
    /// pass.
    #[test]
    fn schema_round_trips(s in schema()) {
        let canonical = schema_canonical(&s);
        let decoded = schema_from_canonical(&canonical).unwrap();
        prop_assert_eq!(&decoded, &s);
        prop_assert_eq!(schema_canonical(&decoded), canonical);
    }

    /// The revision id is a pure function of the schema (§2.13:
    /// deterministic, instance-independent) and injective on it: two
    /// schemas share an id iff they are the same schema.
    #[test]
    fn revision_id_is_deterministic_and_injective(a in schema(), b in schema()) {
        prop_assert_eq!(revision_id(&a), revision_id(&a));
        prop_assert_eq!(revision_id(&a), revision_id(&a.clone()));
        prop_assert_eq!(revision_id(&a) == revision_id(&b), a == b);
        // …and it is exactly the hash of the canonical bytes: same
        // bytes, same id.
        prop_assert_eq!(
            bytes(&schema_canonical(&a)) == bytes(&schema_canonical(&b)),
            a == b
        );
    }

    /// Scalar types and option rows round-trip on their own — the
    /// pieces nomenclature and block lines carry (§5).
    #[test]
    fn scalar_types_round_trip(ty in scalar_type()) {
        // The type's canonical form is what a column embeds: reach it
        // through a one-column schema.
        let s = Schema {
            root: vec![Element::Column(Column {
                id: ColumnId::new("c"), label: String::new(), ty: ty.clone(), arity: Arity::One,
            })],
            resolvers: vec![],
        };
        let CanonicalValue::Object(m) = schema_canonical(&s) else { panic!() };
        let CanonicalValue::Array(elements) = &m["elements"] else { panic!() };
        let CanonicalValue::Object(el) = &elements[0] else { panic!() };
        let CanonicalValue::Object(col) = &el["column"] else { panic!() };
        prop_assert_eq!(scalar_type_from_canonical(&col["type"]).unwrap(), ty);
    }

    #[test]
    fn option_rows_round_trip(row in option_row()) {
        let canonical = option_row_canonical(&row);
        prop_assert_eq!(option_row_from_canonical(&canonical).unwrap(), row.clone());
        prop_assert_eq!(option_row_canonical(&option_row_from_canonical(&canonical).unwrap()), canonical);
    }

    /// Blocks: the schema-side half travels like a nomenclature (§2.1)
    /// and round-trips the same way; its content hash is a function of
    /// the block.
    #[test]
    fn blocks_round_trip(b in block(), other in block()) {
        let canonical = block_canonical(&b);
        let decoded = block_from_canonical(&canonical).unwrap();
        prop_assert_eq!(&decoded, &b);
        prop_assert_eq!(block_canonical(&decoded), canonical);
        prop_assert_eq!(b.content_hash(), decoded.content_hash());
        prop_assert_eq!(b.content_hash() == other.content_hash(), b == other);
    }

    /// Attachment constraints are a *set* (§2.15): case, order,
    /// duplicates and `*/*` are not identity-bearing. Every spelling
    /// canonicalizes to the same bytes and decodes to the normalized
    /// form.
    #[test]
    fn attachment_constraints_canonicalize_as_a_set(
        accept in proptest::collection::vec(
            prop_oneof![
                Just("application/pdf"), Just("Application/PDF"), Just("image/*"),
                Just("IMAGE/*"), Just("image/png"), Just("*/*"), Just(" text/csv ; charset=utf-8"),
            ],
            0..5,
        ),
        max_bytes in proptest::option::of(0u64..(1 << 53)),
        shuffle in any::<u64>(),
    ) {
        let raw = AttachmentConstraints {
            accept: accept.iter().map(|s| s.to_string()).collect(),
            max_bytes,
        };
        let mut permuted = raw.clone();
        permuted.accept.rotate_left((shuffle as usize) % (raw.accept.len().max(1)));
        let ty = |c: AttachmentConstraints| ScalarType::Attachment(c);
        let single = |t: ScalarType| Schema {
            root: vec![Element::Column(Column {
                id: ColumnId::new("f"), label: String::new(), ty: t, arity: Arity::One,
            })],
            resolvers: vec![],
        };
        let normalized = single(ty(raw.normalized()));
        prop_assert_eq!(revision_id(&single(ty(raw.clone()))), revision_id(&normalized));
        prop_assert_eq!(revision_id(&single(ty(permuted))), revision_id(&normalized));
        // Decoding lands on the normalized form, which is a fixed point.
        let decoded = schema_from_canonical(&schema_canonical(&single(ty(raw)))).unwrap();
        prop_assert_eq!(&decoded, &normalized);
        prop_assert_eq!(schema_from_canonical(&schema_canonical(&decoded)).unwrap(), normalized);
    }
}

#[test]
fn malformed_schemas_error_cleanly() {
    let obj = |pairs: &[(&str, CanonicalValue)]| {
        CanonicalValue::Object(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    };
    let s = |t: &str| CanonicalValue::String(t.into());
    let arr = |items: Vec<CanonicalValue>| CanonicalValue::Array(items);
    let column = |ty: CanonicalValue, arity: &str| {
        obj(&[(
            "column",
            obj(&[("id", s("c")), ("label", s("c")), ("type", ty), ("arity", s(arity))]),
        )])
    };
    let schema_of = |elements: Vec<CanonicalValue>| {
        obj(&[("elements", arr(elements)), ("resolvers", arr(vec![]))])
    };
    let kind = |k: &str| obj(&[("kind", s(k))]);

    // Well-formed baseline, so the negatives below fail for the reason
    // they claim.
    assert!(schema_from_canonical(&schema_of(vec![column(kind("text"), "one")])).is_ok());

    let bad = [
        // Not an object / missing top-level arrays.
        CanonicalValue::Null,
        arr(vec![]),
        obj(&[("elements", arr(vec![]))]),
        obj(&[("resolvers", arr(vec![]))]),
        obj(&[("elements", CanonicalValue::Null), ("resolvers", arr(vec![]))]),
        // An element is a column or a group, nothing else.
        schema_of(vec![obj(&[])]),
        schema_of(vec![obj(&[("section", obj(&[]))])]),
        schema_of(vec![s("column")]),
        // Column: missing / mistyped fields, bad arity.
        schema_of(vec![obj(&[("column", obj(&[("id", s("c")), ("label", s("c")), ("type", kind("text"))]))])]),
        schema_of(vec![obj(&[("column", obj(&[("id", s("c")), ("type", kind("text")), ("arity", s("one"))]))])]),
        schema_of(vec![obj(&[("column", obj(&[("id", CanonicalValue::Int(1)), ("label", s("c")), ("type", kind("text")), ("arity", s("one"))]))])]),
        schema_of(vec![column(kind("text"), "two")]),
        schema_of(vec![column(kind("text"), "One")]),
        // Scalar kinds: unknown, missing, non-string.
        schema_of(vec![column(kind("string"), "one")]),
        schema_of(vec![column(obj(&[]), "one")]),
        schema_of(vec![column(obj(&[("kind", CanonicalValue::Int(1))]), "one")]),
        schema_of(vec![column(s("text"), "one")]),
        // Units: unknown name, non-string.
        schema_of(vec![column(obj(&[("kind", s("integer")), ("unit", s("furlong"))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("decimal")), ("unit", s("KM"))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("integer")), ("unit", CanonicalValue::Int(1))]), "one")]),
        // Attachments: accept must be strings, max_bytes a non-negative int.
        schema_of(vec![column(obj(&[("kind", s("attachment")), ("accept", s("image/*"))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("attachment")), ("accept", arr(vec![CanonicalValue::Int(1)]))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("attachment")), ("max_bytes", CanonicalValue::Int(-1))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("attachment")), ("max_bytes", s("10"))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("attachment")), ("max_bytes", CanonicalValue::Float(1.5))]), "one")]),
        // Enums: nomenclature missing, neither inline nor published,
        // malformed rows / published refs.
        schema_of(vec![column(kind("enum"), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", obj(&[]))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", s("cog"))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", obj(&[("inline", obj(&[]))]))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", obj(&[("inline", arr(vec![obj(&[("id", s("o1")), ("label", s("x"))])]))]))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", obj(&[("inline", arr(vec![obj(&[("id", s("o1")), ("label", s("x")), ("fields", arr(vec![s("k")]))])]))]))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", obj(&[("inline", arr(vec![obj(&[("id", s("o1")), ("label", s("x")), ("fields", arr(vec![arr(vec![s("k")])]))])]))]))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", obj(&[("published", obj(&[("id", s("cog"))]))]))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", obj(&[("published", obj(&[("id", s("cog")), ("version", CanonicalValue::Int(-1))]))]))]), "one")]),
        schema_of(vec![column(obj(&[("kind", s("enum")), ("nomenclature", obj(&[("published", obj(&[("id", s("cog")), ("version", s("1"))]))]))]), "one")]),
        // Groups: bad cardinality, missing children, malformed provenance.
        schema_of(vec![obj(&[("group", obj(&[("id", s("g")), ("label", s("g")), ("cardinality", s("some")), ("children", arr(vec![]))]))])]),
        schema_of(vec![obj(&[("group", obj(&[("id", s("g")), ("label", s("g")), ("cardinality", s("one"))]))])]),
        schema_of(vec![obj(&[("group", obj(&[("id", s("g")), ("label", s("g")), ("cardinality", s("one")), ("children", arr(vec![])), ("included_from", s("rib"))]))])]),
        schema_of(vec![obj(&[("group", obj(&[("id", s("g")), ("label", s("g")), ("cardinality", s("one")), ("children", arr(vec![])), ("included_from", obj(&[("id", s("rib"))]))]))])]),
        schema_of(vec![obj(&[("group", obj(&[("id", s("g")), ("label", s("g")), ("cardinality", s("one")), ("children", arr(vec![])), ("included_from", obj(&[("id", s("rib")), ("version", CanonicalValue::Int(-3))]))]))])]),
        // A malformed child deep inside a group is still a refusal.
        schema_of(vec![obj(&[("group", obj(&[("id", s("g")), ("label", s("g")), ("cardinality", s("many")), ("children", arr(vec![column(kind("nope"), "one")]))]))])]),
        // Resolvers: missing arrays, bad input pairs, bad version.
        obj(&[("elements", arr(vec![])), ("resolvers", arr(vec![obj(&[("id", s("r")), ("version", CanonicalValue::Int(1))])]))]),
        obj(&[("elements", arr(vec![])), ("resolvers", arr(vec![obj(&[("id", s("r")), ("version", CanonicalValue::Int(1)), ("input", arr(vec![s("c")])), ("result", arr(vec![])), ("mapping", arr(vec![]))])]))]),
        obj(&[("elements", arr(vec![])), ("resolvers", arr(vec![obj(&[("id", s("r")), ("version", CanonicalValue::Int(1)), ("input", arr(vec![arr(vec![s("c"), kind("nope")])])), ("result", arr(vec![])), ("mapping", arr(vec![]))])]))]),
        obj(&[("elements", arr(vec![])), ("resolvers", arr(vec![obj(&[("id", s("r")), ("version", CanonicalValue::Int(-1)), ("input", arr(vec![])), ("result", arr(vec![])), ("mapping", arr(vec![]))])]))]),
        obj(&[("elements", arr(vec![])), ("resolvers", arr(vec![obj(&[("id", s("r")), ("version", CanonicalValue::Int(1)), ("input", arr(vec![])), ("result", arr(vec![obj(&[("name", s("x"))])])), ("mapping", arr(vec![]))])]))]),
        obj(&[("elements", arr(vec![])), ("resolvers", arr(vec![obj(&[("id", s("r")), ("version", CanonicalValue::Int(1)), ("input", arr(vec![])), ("result", arr(vec![])), ("mapping", arr(vec![obj(&[("field", s("x"))])]))])]))]),
    ];
    for value in &bad {
        assert!(schema_from_canonical(value).is_err(), "{value:?} should be refused");
    }

    // The standalone decoders refuse the same way.
    assert!(scalar_type_from_canonical(&kind("string")).is_err());
    assert!(scalar_type_from_canonical(&CanonicalValue::Null).is_err());
    assert!(option_row_from_canonical(&obj(&[("id", s("o1"))])).is_err());
    assert!(option_row_from_canonical(&obj(&[("id", s("o1")), ("label", s("x")), ("fields", s("k=v"))])).is_err());
    assert!(block_from_canonical(&obj(&[("id", s("b")), ("version", CanonicalValue::Int(1)), ("group", column(kind("text"), "one")), ("resolvers", arr(vec![]))])).is_err());
    assert!(block_from_canonical(&obj(&[("id", s("b")), ("version", CanonicalValue::Int(1)), ("resolvers", arr(vec![]))])).is_err());
    assert!(block_from_canonical(&obj(&[("id", s("b")), ("group", obj(&[("group", obj(&[("id", s("g")), ("label", s("g")), ("cardinality", s("one")), ("children", arr(vec![]))]))])), ("resolvers", arr(vec![]))])).is_err());
}
