//! Lattice laws for the type join (§5.5), over generated types —
//! including random inline enums, where the merge/conflict logic lives.

use proptest::prelude::*;
use varve_core::OptionId;
use varve_schema::{
    Arity, JoinPath, NomenclatureRef, NomenclatureTable, OptionRow, ScalarType,
    Unit, arity_join, column_join, scalar_cast, scalar_join,
};

/// Small id/label alphabets force collisions, merges, and label
/// conflicts.
fn inline_enum() -> impl Strategy<Value = ScalarType> {
    proptest::collection::btree_map(
        prop_oneof![Just("o1"), Just("o2"), Just("o3")],
        prop_oneof![Just("A"), Just("B")],
        0..=3,
    )
    .prop_map(|rows| {
        ScalarType::Enum(NomenclatureRef::Inline(
            rows.into_iter()
                .map(|(id, label)| OptionRow {
                    id: OptionId::new(id),
                    label: label.to_string(),
                    fields: vec![],
                })
                .collect(),
        ))
    })
}

fn scalar_type() -> impl Strategy<Value = ScalarType> {
    prop_oneof![
        Just(ScalarType::Text),
        Just(ScalarType::Boolean),
        Just(ScalarType::Integer(None)),
        Just(ScalarType::Integer(Some(Unit::Day))),
        Just(ScalarType::Integer(Some(Unit::Month))),
        Just(ScalarType::Decimal(None)),
        Just(ScalarType::Decimal(Some(Unit::Metre))),
        Just(ScalarType::Date),
        Just(ScalarType::Datetime),
        Just(ScalarType::Attachment),
        Just(ScalarType::Geometry),
        inline_enum(),
    ]
}

fn arity() -> impl Strategy<Value = Arity> {
    prop_oneof![Just(Arity::One), Just(Arity::Many)]
}

proptest! {
    /// join(a, a) = a, reached directly.
    #[test]
    fn join_is_idempotent(a in scalar_type()) {
        let n = NomenclatureTable::new();
        let (joined, path) = scalar_join(&a, &a, &n).unwrap();
        prop_assert_eq!(joined, a);
        prop_assert_eq!(path, JoinPath::Direct);
    }

    /// Symmetric in success/failure and path; the two orientations'
    /// results are mutually reachable by pure widening (equal up to
    /// enum row order).
    #[test]
    fn join_is_commutative(a in scalar_type(), b in scalar_type()) {
        let n = NomenclatureTable::new();
        match (scalar_join(&a, &b, &n), scalar_join(&b, &a, &n)) {
            (Err(_), Err(_)) => {}
            (Ok((ab, pa)), Ok((ba, pb))) => {
                prop_assert_eq!(pa, pb);
                prop_assert!(scalar_cast(&ab, &ba, &n).unwrap().is_widening());
                prop_assert!(scalar_cast(&ba, &ab, &n).unwrap().is_widening());
            }
            _ => prop_assert!(false, "join symmetric in neither"),
        }
    }

    /// The join is an upper bound: both inputs reach it by pure
    /// widening. (The generalization of the hand-swept sample test.)
    #[test]
    fn join_is_an_upper_bound(a in scalar_type(), b in scalar_type()) {
        let n = NomenclatureTable::new();
        if let Ok((joined, _)) = scalar_join(&a, &b, &n) {
            for side in [&a, &b] {
                let cast = scalar_cast(side, &joined, &n).unwrap();
                prop_assert!(
                    cast.is_widening(),
                    "{side:?} does not widen to join {joined:?}"
                );
            }
        }
    }

    /// Column joins: arity is the max, and the scalar law carries over.
    #[test]
    fn column_join_widens_both_sides(
        a in scalar_type(), b in scalar_type(),
        aa in arity(), ab in arity(),
    ) {
        let n = NomenclatureTable::new();
        prop_assume!(!matches!(
            (&a, &b),
            (ScalarType::Attachment | ScalarType::Geometry, _)
                | (_, ScalarType::Attachment | ScalarType::Geometry)
        ) || a == b);
        if let Ok(((ty, arity), _)) = column_join((&a, aa), (&b, ab), &n) {
            prop_assert_eq!(arity, arity_join(aa, ab));
            for (side_ty, side_arity) in [(&a, aa), (&b, ab)] {
                let cast = varve_schema::column_cast(
                    (side_ty, side_arity),
                    (&ty, arity),
                    &n,
                )
                .unwrap();
                prop_assert!(cast.is_widening());
            }
        }
    }
}
