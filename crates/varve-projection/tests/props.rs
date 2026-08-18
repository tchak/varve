//! Projection laws: identity is a no-op, and widening casts round-trip.

use proptest::prelude::*;
use varve_core::primitives::{Date, Decimal};
use varve_core::{ColumnId, OptionId, RowPath};
use varve_projection::project;
use varve_schema::{
    Arity, Column, Element, NomenclatureRef, OptionRow, ScalarType, Schema,
};
use varve_value::{CellAddr, CellState, CellValue, RecordValues, Scalar, check};

fn column(id: &str, ty: ScalarType, arity: Arity) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity,
    })
}

fn single(id: &str, ty: ScalarType) -> Schema {
    Schema {
        root: vec![column(id, ty, Arity::One)],
        resolvers: vec![],
    }
}

fn addr(id: &str) -> CellAddr {
    CellAddr {
        column: ColumnId::new(id),
        path: RowPath::root(),
    }
}

fn one(scalar: Scalar) -> CellState {
    CellState::Value(CellValue::One(scalar))
}

fn options() -> NomenclatureRef {
    NomenclatureRef::Inline(
        [("o1", "Oui"), ("o2", "Non")]
            .into_iter()
            .map(|(id, label)| OptionRow {
                id: OptionId::new(id),
                label: label.to_string(),
                fields: vec![],
            })
            .collect(),
    )
}

/// A fixed mixed schema plus a strategy for conforming values over it.
fn mixed_schema() -> Schema {
    Schema {
        root: vec![
            column("t", ScalarType::Text, Arity::One),
            column("n", ScalarType::Integer(None), Arity::One),
            column("choice", ScalarType::Enum(options()), Arity::One),
            column("tags", ScalarType::Enum(options()), Arity::Many),
        ],
        resolvers: vec![],
    }
}

/// Readers a revision of `mixed_schema()` might become: retypes
/// (widening, checked, lens), an arity narrowing, a removal, an addition.
fn reader_variant() -> impl Strategy<Value = Schema> {
    let base = || mixed_schema();
    prop_oneof![
        Just(base()),
        Just(Schema {
            root: base().root.into_iter().map(|e| match e {
                Element::Column(mut c) if c.id.as_str() == "n" => {
                    c.ty = ScalarType::Decimal(None);
                    Element::Column(c)
                }
                other => other,
            }).collect(),
            resolvers: vec![],
        }),
        Just(Schema {
            root: base().root.into_iter().map(|e| match e {
                Element::Column(mut c) if c.id.as_str() == "n" => {
                    c.ty = ScalarType::Text;
                    Element::Column(c)
                }
                other => other,
            }).collect(),
            resolvers: vec![],
        }),
        Just(Schema {
            root: base().root.into_iter().map(|e| match e {
                Element::Column(mut c) if c.id.as_str() == "choice" => {
                    c.ty = ScalarType::Text;
                    Element::Column(c)
                }
                other => other,
            }).collect(),
            resolvers: vec![],
        }),
        Just(Schema {
            root: base().root.into_iter().map(|e| match e {
                Element::Column(mut c) if c.id.as_str() == "tags" => {
                    c.arity = Arity::One;
                    Element::Column(c)
                }
                other => other,
            }).collect(),
            resolvers: vec![],
        }),
        Just(Schema {
            root: base().root.into_iter().filter(|e| !matches!(e, Element::Column(c) if c.id.as_str() == "t")).collect(),
            resolvers: vec![],
        }),
        Just(Schema {
            root: base().root.into_iter().chain([column("added", ScalarType::Boolean, Arity::One)]).collect(),
            resolvers: vec![],
        }),
        // Text → enum: checked through labels.
        Just(Schema {
            root: base().root.into_iter().map(|e| match e {
                Element::Column(mut c) if c.id.as_str() == "t" => {
                    c.ty = ScalarType::Enum(options());
                    Element::Column(c)
                }
                other => other,
            }).collect(),
            resolvers: vec![],
        }),
    ]
}

fn conforming_values() -> impl Strategy<Value = RecordValues> {
    (
        proptest::option::of("[a-z]{0,4}"),
        proptest::option::of(any::<i32>()),
        proptest::option::of(prop_oneof![Just("o1"), Just("o2")]),
        proptest::sample::subsequence(vec!["o1", "o2"], 0..=2),
    )
        .prop_map(|(t, n, choice, tags)| {
            let mut v = RecordValues::new();
            if let Some(t) = t {
                v.cells.insert(addr("t"), one(Scalar::Text(t)));
            }
            if let Some(n) = n {
                v.cells.insert(addr("n"), one(Scalar::Integer(n.into())));
            }
            if let Some(c) = choice {
                v.cells
                    .insert(addr("choice"), one(Scalar::Enum(OptionId::new(c))));
            }
            if !tags.is_empty() {
                v.cells.insert(
                    addr("tags"),
                    CellState::Value(CellValue::Many(
                        tags.into_iter()
                            .map(|o| Scalar::Enum(OptionId::new(o)))
                            .collect(),
                    )),
                );
            }
            v
        })
}

proptest! {
    /// project(v, s, s) == v, with a clean report — projection is a
    /// no-op over an unchanged revision (§3).
    #[test]
    fn identity_projection_is_a_no_op(v in conforming_values()) {
        let s = mixed_schema();
        let p = project(&v, &s, &s, &Default::default()).unwrap();
        prop_assert_eq!(p.values, v);
        prop_assert!(p.report.is_clean());
    }

    /// Widening then narrowing round-trips exactly for values that
    /// originated narrow: integer → decimal → integer.
    #[test]
    fn integer_decimal_round_trip(n in any::<i64>()) {
        let int = single("c", ScalarType::Integer(None));
        let dec = single("c", ScalarType::Decimal(None));
        let mut v = RecordValues::new();
        v.cells.insert(addr("c"), one(Scalar::Integer(n)));

        let widened = project(&v, &int, &dec, &Default::default()).unwrap();
        prop_assert!(widened.report.is_clean());
        let back = project(&widened.values, &dec, &int, &Default::default()).unwrap();
        prop_assert_eq!(back.values, v);
        prop_assert_eq!(back.report.total_failed(), 0);
    }

    /// date → datetime → date round-trips losslessly.
    #[test]
    fn date_datetime_round_trip(
        y in 1900i32..=2100, m in 1u8..=12, d in 1u8..=28,
    ) {
        let date = single("c", ScalarType::Date);
        let datetime = single("c", ScalarType::Datetime);
        let mut v = RecordValues::new();
        v.cells.insert(
            addr("c"),
            one(Scalar::Date(Date::parse(&format!("{y:04}-{m:02}-{d:02}")).unwrap())),
        );

        let widened = project(&v, &date, &datetime, &Default::default()).unwrap();
        prop_assert!(widened.report.is_clean());
        let back =
            project(&widened.values, &datetime, &date, &Default::default()).unwrap();
        prop_assert_eq!(back.values, v);
        prop_assert_eq!(back.report.total_lossy(), 0);
    }

    /// integer → text → integer round-trips through canonical text.
    #[test]
    fn integer_text_round_trip(n in any::<i64>()) {
        let int = single("c", ScalarType::Integer(None));
        let text = single("c", ScalarType::Text);
        let mut v = RecordValues::new();
        v.cells.insert(addr("c"), one(Scalar::Integer(n)));

        let widened = project(&v, &int, &text, &Default::default()).unwrap();
        let back = project(&widened.values, &text, &int, &Default::default()).unwrap();
        prop_assert_eq!(back.values, v);
    }

    /// decimal → integer is exact-or-nothing: it never silently alters
    /// a value — either the exact integer comes back, or the cell
    /// fails and is counted.
    #[test]
    fn decimal_to_integer_never_lies(s in "-?[0-9]{1,15}(\\.[0-9]{1,4})?") {
        let value = Decimal::parse(&s).unwrap();
        let dec = single("c", ScalarType::Decimal(None));
        let int = single("c", ScalarType::Integer(None));
        let mut v = RecordValues::new();
        v.cells.insert(addr("c"), one(Scalar::Decimal(value.clone())));

        let p = project(&v, &dec, &int, &Default::default()).unwrap();
        match p.values.cells.get(&addr("c")) {
            Some(CellState::Value(CellValue::One(Scalar::Integer(i)))) => {
                // Kept: must be exactly the same number.
                prop_assert_eq!(Decimal::from_i64(*i), value);
                prop_assert_eq!(p.report.total_failed(), 0);
            }
            None => prop_assert_eq!(p.report.total_failed(), 1),
            other => prop_assert!(false, "unexpected projection: {other:?}"),
        }
    }

    /// Whatever the reader, the projection's output conforms to it, and
    /// every writer cell of a reader column is accounted for: projected
    /// or failed, never lost in between (§3, §5.5).
    #[test]
    fn projection_output_conforms_to_the_reader(
        v in conforming_values(),
        reader in reader_variant(),
    ) {
        let writer = mixed_schema();
        let p = project(&v, &writer, &reader, &Default::default()).unwrap();
        prop_assert_eq!(check(&p.values, &reader, &Default::default()), vec![]);
        for (column, report) in &p.report.columns {
            let had = v.cells.keys().filter(|a| &a.column == column).count() as u64;
            prop_assert_eq!(
                report.cells_projected + report.cells_failed,
                had,
                "column {}: {:?}",
                column,
                report
            );
            prop_assert!(report.cells_lossy <= report.cells_projected);
        }
        // Reader-only columns read absent; nothing lands in them.
        for (column, report) in &p.report.columns {
            if report.status == varve_projection::ColumnStatus::AddedAbsent {
                prop_assert!(!p.values.cells.keys().any(|a| &a.column == column));
            }
        }
    }
}
