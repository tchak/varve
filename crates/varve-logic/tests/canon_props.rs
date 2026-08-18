//! Round-trip law for the canonical form: `from(to(expr)) == expr` over
//! generated expressions of every shape.

use proptest::prelude::*;
use std::collections::BTreeSet;
use varve_core::canonical::{CanonicalValue, canonical_bytes};
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, OptionId, ResolverId};
use varve_logic::{
    Atom, ColumnRef, Const, Expr, Operand, from_canonical, resolver_sources, sources,
    to_canonical,
};
use varve_schema::Unit;

/// Every unit variant (§2.14): the canonical form names each one.
const UNITS: &[Unit] = &[
    Unit::Millimetre,
    Unit::Centimetre,
    Unit::Metre,
    Unit::Kilometre,
    Unit::Gram,
    Unit::Kilogram,
    Unit::Tonne,
    Unit::Minute,
    Unit::Hour,
    Unit::Day,
    Unit::Week,
    Unit::Month,
    Unit::Year,
    Unit::SquareMetre,
    Unit::Hectare,
    Unit::SquareKilometre,
    Unit::Litre,
    Unit::CubicMetre,
    Unit::Percent,
];

fn column_ref() -> impl Strategy<Value = ColumnRef> {
    ("[a-z][a-z0-9_]{0,6}", proptest::option::of("[a-z]{1,8}")).prop_map(
        |(column, field)| ColumnRef {
            column: ColumnId::new(column),
            field,
        },
    )
}

/// A calendar date anywhere in the canonical year range (§2.13).
fn date() -> impl Strategy<Value = Date> {
    (0i32..=9998, 1u8..=12, 1u8..=28)
        .prop_map(|(y, m, d)| Date::parse(&format!("{y:04}-{m:02}-{d:02}")).unwrap())
}

/// An instant with a fraction and an offset — the stored value
/// normalizes to UTC, so a decoded constant compares equal.
fn datetime() -> impl Strategy<Value = Instant> {
    (
        1i32..=9998,
        1u8..=12,
        1u8..=28,
        0u8..24,
        0u8..60,
        0u8..60,
        proptest::option::of("[0-9]{1,9}"),
        prop_oneof![Just("Z".to_string()), (0u8..14, 0u8..60, any::<bool>()).prop_map(
            |(h, m, neg)| format!("{}{h:02}:{m:02}", if neg { '-' } else { '+' })
        )],
    )
        .prop_map(|(y, mo, d, h, mi, s, frac, off)| {
            let frac = frac.map(|f| format!(".{f}")).unwrap_or_default();
            Instant::parse(&format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}{frac}{off}"))
                .unwrap()
        })
}

fn constant() -> impl Strategy<Value = Const> {
    prop_oneof![
        any::<bool>().prop_map(Const::Boolean),
        (
            "-?[0-9]{1,12}(\\.[0-9]{1,6})?",
            proptest::option::of(proptest::sample::select(UNITS)),
        )
            .prop_map(|(n, unit)| Const::Number {
                value: Decimal::parse(&n).unwrap(),
                unit,
            }),
        date().prop_map(Const::Date),
        datetime().prop_map(Const::Datetime),
        "[a-z0-9]{1,6}".prop_map(|s| Const::Option(OptionId::new(s))),
        "\\PC{0,12}".prop_map(Const::Text),
    ]
}

fn operand() -> impl Strategy<Value = Operand> {
    prop_oneof![
        constant().prop_map(Operand::Const),
        column_ref().prop_map(Operand::Column),
    ]
}

/// All twelve atom kinds (§4.1).
fn atom() -> impl Strategy<Value = Atom> {
    let cmp = |ctor: fn(ColumnRef, Operand) -> Atom| {
        (column_ref(), operand()).prop_map(move |(source, right)| ctor(source, right))
    };
    prop_oneof![
        cmp(|source, right| Atom::Eq { source, right }),
        cmp(|source, right| Atom::NotEq { source, right }),
        cmp(|source, right| Atom::Lt { source, right }),
        cmp(|source, right| Atom::Le { source, right }),
        cmp(|source, right| Atom::Gt { source, right }),
        cmp(|source, right| Atom::Ge { source, right }),
        column_ref().prop_map(|source| Atom::IsEmpty { source }),
        column_ref().prop_map(|source| Atom::IsFilled { source }),
        (column_ref(), "[a-z0-9]{1,6}").prop_map(|(source, o)| Atom::Contains {
            source,
            option: OptionId::new(o),
        }),
        (column_ref(), "[a-z0-9]{1,6}").prop_map(|(source, o)| Atom::Excludes {
            source,
            option: OptionId::new(o),
        }),
        "[a-z-]{1,10}".prop_map(|r| Atom::Pending { resolver: ResolverId::new(r) }),
        "[a-z-]{1,10}".prop_map(|r| Atom::NotPending { resolver: ResolverId::new(r) }),
    ]
}

fn expr() -> impl Strategy<Value = Expr> {
    atom().prop_map(Expr::Atom).prop_recursive(8, 64, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Expr::And),
            proptest::collection::vec(inner, 0..6).prop_map(Expr::Or),
        ]
    })
}

/// The obvious recursive depth — the oracle for the iterative one:
/// the deepest position any node sits at (an empty combinator at the
/// root, like an atom, is 0).
fn recursive_depth(e: &Expr) -> usize {
    match e {
        Expr::Atom(_) => 0,
        Expr::And(items) | Expr::Or(items) => {
            items.iter().map(|i| 1 + recursive_depth(i)).max().unwrap_or(0)
        }
    }
}

fn has_float(v: &CanonicalValue) -> bool {
    match v {
        CanonicalValue::Float(_) => true,
        CanonicalValue::Array(a) => a.iter().any(has_float),
        CanonicalValue::Object(o) => o.values().any(has_float),
        _ => false,
    }
}

/// Independent walk: every column and resolver an expression names.
fn mentioned(e: &Expr, columns: &mut BTreeSet<ColumnId>, resolvers: &mut BTreeSet<ResolverId>) {
    match e {
        Expr::And(items) | Expr::Or(items) => {
            for i in items {
                mentioned(i, columns, resolvers);
            }
        }
        Expr::Atom(a) => match a {
            Atom::Eq { source, right }
            | Atom::NotEq { source, right }
            | Atom::Lt { source, right }
            | Atom::Le { source, right }
            | Atom::Gt { source, right }
            | Atom::Ge { source, right } => {
                columns.insert(source.column.clone());
                if let Operand::Column(c) = right {
                    columns.insert(c.column.clone());
                }
            }
            Atom::IsEmpty { source }
            | Atom::IsFilled { source }
            | Atom::Contains { source, .. }
            | Atom::Excludes { source, .. } => {
                columns.insert(source.column.clone());
            }
            Atom::Pending { resolver } | Atom::NotPending { resolver } => {
                resolvers.insert(resolver.clone());
            }
        },
    }
}

proptest! {
    #[test]
    fn canonical_round_trips(e in expr()) {
        let encoded = to_canonical(&e);
        prop_assert_eq!(from_canonical(&encoded).unwrap(), e);
    }

    /// The canonical form is hashable (§2.13): no floats anywhere,
    /// and the JCS bytes always exist — surfaces hash rules by it.
    #[test]
    fn canonical_form_is_hashable(e in expr()) {
        let encoded = to_canonical(&e);
        prop_assert!(!has_float(&encoded));
        prop_assert!(canonical_bytes(&encoded).is_ok());
    }

    /// The iterative depth walk agrees with the recursive definition.
    #[test]
    fn depth_matches_recursive_definition(e in expr()) {
        prop_assert_eq!(e.depth(), recursive_depth(&e));
    }

    /// `sources` reads exactly the columns an expression names (both
    /// sides of a comparison), `resolver_sources` exactly its resolvers.
    #[test]
    fn sources_are_exactly_what_is_mentioned(e in expr()) {
        let mut columns = BTreeSet::new();
        let mut resolvers = BTreeSet::new();
        mentioned(&e, &mut columns, &mut resolvers);
        prop_assert_eq!(sources(&e), columns);
        prop_assert_eq!(resolver_sources(&e), resolvers);
    }
}

#[test]
fn malformed_inputs_error_cleanly() {
    use varve_core::canonical::CanonicalValue;
    let obj = |pairs: &[(&str, CanonicalValue)]| {
        CanonicalValue::Object(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    };
    let s = |t: &str| CanonicalValue::String(t.into());
    let arr = CanonicalValue::Array(vec![]);
    let bad = [
        CanonicalValue::Null,
        s("eq"),
        obj(&[]),
        // Strict: one meaning, one text.
        obj(&[("and", arr.clone()), ("or", arr.clone())]),
        obj(&[("and", arr.clone()), ("junk", CanonicalValue::Null)]),
        obj(&[("op", s("is_empty")), ("source", obj(&[("column", s("a"))])), ("right", obj(&[]))]),
        obj(&[
            ("op", s("eq")),
            ("source", obj(&[("column", s("a"))])),
            ("right", obj(&[("const", obj(&[("boolean", CanonicalValue::Bool(true)), ("text", s("x"))]))])),
        ]),
        obj(&[
            ("op", s("eq")),
            ("source", obj(&[("column", s("a")), ("extra", s("x"))])),
            ("right", obj(&[("const", obj(&[("text", s("x"))]))])),
        ]),
        obj(&[
            ("op", s("eq")),
            ("source", obj(&[("column", s("a"))])),
            ("right", obj(&[("const", obj(&[("text", s("x"))])), ("column_ref", obj(&[("column", s("b"))]))])),
        ]),
    ];
    for value in &bad {
        assert!(from_canonical(value).is_err(), "{value:?} should be refused");
    }
    // Depth is bounded so every walk stays total: 64 levels decode,
    // 65 are refused; the typechecker refuses too if built in-process.
    let nest = |depth: usize| {
        let mut e = obj(&[("or", arr.clone())]);
        for _ in 0..depth {
            e = obj(&[("and", CanonicalValue::Array(vec![e]))]);
        }
        e
    };
    let deep_ok = from_canonical(&nest(varve_logic::MAX_DEPTH)).unwrap();
    assert_eq!(deep_ok.depth(), varve_logic::MAX_DEPTH);
    assert!(from_canonical(&nest(varve_logic::MAX_DEPTH + 1)).is_err());
    let mut too_deep = varve_logic::Expr::Or(vec![]);
    for _ in 0..=varve_logic::MAX_DEPTH {
        too_deep = varve_logic::Expr::And(vec![too_deep]);
    }
    let schema = varve_schema::Schema::default();
    assert!(matches!(
        varve_logic::typecheck(&too_deep, &schema, &Default::default(), &[]).as_slice(),
        [varve_logic::TypeError::TooDeep(_)]
    ));
    // The very deep expression itself is fine to construct and measure
    // (depth() is iterative) even at thousands of levels.
    let mut huge = varve_logic::Expr::Or(vec![]);
    for _ in 0..5000 {
        huge = varve_logic::Expr::And(vec![huge]);
    }
    assert_eq!(huge.depth(), 5000);
    assert!(matches!(
        varve_logic::typecheck(&huge, &schema, &Default::default(), &[]).as_slice(),
        [varve_logic::TypeError::TooDeep(_)]
    ));
    // Dropping a 5000-deep expression must not overflow either.
    drop(huge);
}
