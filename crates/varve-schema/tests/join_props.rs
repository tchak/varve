//! Lattice laws for the type join (§5.5), over generated types —
//! including random inline enums, where the merge/conflict logic lives.

use proptest::prelude::*;
use varve_core::{NomenclatureId, OptionId};
use varve_schema::{
    Arity, AttachmentConstraints, JoinPath, NomenclatureRef, NomenclatureTable,
    OptionRow, ScalarType, Unit, arity_join, column_join, scalar_cast,
    scalar_join,
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

fn row(id: &str, label: &str) -> OptionRow {
    OptionRow { id: OptionId::new(id), label: label.into(), fields: vec![] }
}

/// The table every law runs against: one published nomenclature in two
/// append-only versions (§2.11), plus an unrelated one — so joins and
/// casts between `cog@1`, `cog@2` and `pays@1` are exercised.
fn table() -> NomenclatureTable {
    let mut n = NomenclatureTable::new();
    n.insert(NomenclatureId::new("cog"), 1, vec![row("01", "Ain")]);
    n.insert(NomenclatureId::new("cog"), 2, vec![row("01", "Ain"), row("02", "Aisne")]);
    n.insert(NomenclatureId::new("pays"), 1, vec![row("FR", "France")]);
    n
}

fn published(id: &str, version: u32) -> ScalarType {
    ScalarType::Enum(NomenclatureRef::Published { id: NomenclatureId::new(id), version })
}

fn scalar_type() -> impl Strategy<Value = ScalarType> {
    prop_oneof![
        Just(published("cog", 1)),
        Just(published("cog", 2)),
        Just(published("pays", 1)),
        Just(ScalarType::Text),
        Just(ScalarType::Boolean),
        Just(ScalarType::Integer(None)),
        Just(ScalarType::Integer(Some(Unit::Day))),
        Just(ScalarType::Integer(Some(Unit::Week))),
        Just(ScalarType::Integer(Some(Unit::Month))),
        Just(ScalarType::Decimal(None)),
        Just(ScalarType::Decimal(Some(Unit::Day))),
        Just(ScalarType::Decimal(Some(Unit::Metre))),
        Just(ScalarType::Date),
        Just(ScalarType::Datetime),
        Just(ScalarType::Attachment(Default::default())),
        Just(ScalarType::Attachment(AttachmentConstraints {
            accept: vec!["application/pdf".into()],
            max_bytes: Some(1_000),
        })),
        Just(ScalarType::Attachment(AttachmentConstraints {
            accept: vec!["image/*".into()],
            max_bytes: None,
        })),
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
        let n = table();
        let (joined, path) = scalar_join(&a, &a, &n).unwrap();
        prop_assert_eq!(joined, a);
        prop_assert_eq!(path, JoinPath::Direct);
    }

    /// Symmetric in success/failure and path; the two orientations'
    /// results are mutually reachable by pure widening (equal up to
    /// enum row order).
    #[test]
    fn join_is_commutative(a in scalar_type(), b in scalar_type()) {
        let n = table();
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
        let n = table();
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

    /// The join is the *least* upper bound: any common widening target
    /// of both inputs is reachable from the join by widening. This is
    /// the §5.5 "dual of the cast table" claim, checked — it fails if
    /// the widening order is not a partial order (e.g. were unit
    /// add/remove free both ways).
    #[test]
    fn join_is_least(a in scalar_type(), b in scalar_type(), c in scalar_type()) {
        let n = table();
        if let Ok((joined, _)) = scalar_join(&a, &b, &n) {
            let widens = |x: &ScalarType, y: &ScalarType| {
                scalar_cast(x, y, &n).map(|c| c.is_widening()).unwrap_or(false)
            };
            if widens(&a, &c) && widens(&b, &c) {
                prop_assert!(
                    widens(&joined, &c),
                    "{c:?} bounds {a:?} and {b:?} but join {joined:?} does not widen to it"
                );
            }
        }
    }

    /// Associative up to mutual widening (the path tag is a per-step
    /// report, ORed by the aggregate; the *type* must not depend on
    /// fold order).
    #[test]
    fn join_is_associative(a in scalar_type(), b in scalar_type(), c in scalar_type()) {
        let n = table();
        let left = scalar_join(&a, &b, &n).and_then(|(ab, _)| scalar_join(&ab, &c, &n));
        let right = scalar_join(&b, &c, &n).and_then(|(bc, _)| scalar_join(&a, &bc, &n));
        match (left, right) {
            (Err(_), Err(_)) => {}
            (Ok((l, _)), Ok((r, _))) => {
                prop_assert!(scalar_cast(&l, &r, &n).unwrap().is_widening(), "{l:?} vs {r:?}");
                prop_assert!(scalar_cast(&r, &l, &n).unwrap().is_widening(), "{r:?} vs {l:?}");
            }
            (l, r) => prop_assert!(false, "associativity: {l:?} vs {r:?}"),
        }
    }

    /// Column joins: arity is the max, and the scalar law carries over.
    #[test]
    fn column_join_widens_both_sides(
        a in scalar_type(), b in scalar_type(),
        aa in arity(), ab in arity(),
    ) {
        let n = table();
        // Attachment/geometry against anything else has no join by
        // design (Incompatible): nothing to check for those pairs.
        let incompatible = matches!(
            (&a, &b),
            (ScalarType::Attachment(_) | ScalarType::Geometry, _)
                | (_, ScalarType::Attachment(_) | ScalarType::Geometry)
        ) && a != b;
        if incompatible {
            return Ok(());
        }
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
