//! Round-trip law for the canonical form: `from(to(expr)) == expr` over
//! generated expressions of every shape.

use proptest::prelude::*;
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, OptionId, ResolverId};
use varve_logic::{
    Atom, ColumnRef, Const, Expr, Operand, from_canonical, to_canonical,
};
use varve_schema::Unit;

fn column_ref() -> impl Strategy<Value = ColumnRef> {
    ("[a-z][a-z0-9_]{0,6}", proptest::option::of("[a-z]{1,8}")).prop_map(
        |(column, field)| ColumnRef {
            column: ColumnId::new(column),
            field,
        },
    )
}

fn constant() -> impl Strategy<Value = Const> {
    prop_oneof![
        any::<bool>().prop_map(Const::Boolean),
        (any::<i32>(), proptest::option::of(prop_oneof![
            Just(Unit::Day), Just(Unit::Month), Just(Unit::Kilometre),
        ]))
            .prop_map(|(n, unit)| Const::Number {
                value: Decimal::from_i64(n.into()),
                unit,
            }),
        (1970i32..2100, 1u8..=12, 1u8..=28).prop_map(|(y, m, d)| Const::Date(
            Date::parse(&format!("{y:04}-{m:02}-{d:02}")).unwrap()
        )),
        Just(Const::Datetime(Instant::parse("2026-08-17T10:00:00Z").unwrap())),
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

fn atom() -> impl Strategy<Value = Atom> {
    prop_oneof![
        (column_ref(), operand()).prop_map(|(source, right)| Atom::Eq { source, right }),
        (column_ref(), operand()).prop_map(|(source, right)| Atom::NotEq { source, right }),
        (column_ref(), operand()).prop_map(|(source, right)| Atom::Lt { source, right }),
        (column_ref(), operand()).prop_map(|(source, right)| Atom::Ge { source, right }),
        column_ref().prop_map(|source| Atom::IsEmpty { source }),
        column_ref().prop_map(|source| Atom::IsFilled { source }),
        (column_ref(), "[a-z0-9]{1,6}").prop_map(|(source, o)| Atom::Contains {
            source,
            option: OptionId::new(o),
        }),
        "[a-z-]{1,10}".prop_map(|r| Atom::Pending { resolver: ResolverId::new(r) }),
    ]
}

fn expr() -> impl Strategy<Value = Expr> {
    atom().prop_map(Expr::Atom).prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Expr::And),
            proptest::collection::vec(inner, 0..4).prop_map(Expr::Or),
        ]
    })
}

proptest! {
    #[test]
    fn canonical_round_trips(e in expr()) {
        let encoded = to_canonical(&e);
        prop_assert_eq!(from_canonical(&encoded).unwrap(), e);
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
}
