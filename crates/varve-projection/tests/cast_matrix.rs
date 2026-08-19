//! The exhaustive cast/projection matrix (§3, §5.5): every ordered pair
//! of a fixed list of scalar types covering the whole cast table, with
//! representative conforming values, checked against `scalar_cast`'s
//! verdict. The one law that ties them: **a projected cell conforms to
//! the reader schema** — a checked cast either yields a conforming cell
//! or fails the cell and counts it, never a non-conforming cell.

use varve_core::canonical::{CanonicalValue, hash_plain};
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, NomenclatureId, OptionId, RowPath};
use varve_projection::{ColumnStatus, project};
use varve_schema::{
    Arity, AttachmentConstraints, CastClass, Column, Element, NomenclatureRef, NomenclatureTable,
    OptionRow, ScalarType, Schema, Unit, scalar_cast,
};
use varve_value::{
    AttachmentRef, CellAddr, CellState, CellValue, Feature, RecordValues, Scalar, check,
};

fn single(ty: ScalarType, arity: Arity) -> Schema {
    Schema {
        root: vec![Element::Column(Column {
            id: ColumnId::new("c"),
            label: "c".into(),
            ty,
            arity,
        })],
        resolvers: vec![],
    }
}

fn addr() -> CellAddr {
    CellAddr {
        column: ColumnId::new("c"),
        path: RowPath::root(),
    }
}

fn row(id: &str, label: &str) -> OptionRow {
    OptionRow {
        id: OptionId::new(id),
        label: label.into(),
        fields: vec![],
    }
}

fn inline(rows: &[(&str, &str)]) -> ScalarType {
    ScalarType::Enum(NomenclatureRef::Inline(
        rows.iter().map(|(i, l)| row(i, l)).collect(),
    ))
}

fn published(id: &str, version: u32) -> ScalarType {
    ScalarType::Enum(NomenclatureRef::Published {
        id: NomenclatureId::new(id),
        version,
    })
}

fn attachment(accept: &[&str], max_bytes: Option<u64>) -> ScalarType {
    ScalarType::Attachment(AttachmentConstraints {
        accept: accept.iter().map(|s| s.to_string()).collect(),
        max_bytes,
    })
}

/// `cog@1 = {01}`, `cog@2 = {01, 02}` — append-only versions (§2.11).
fn table() -> NomenclatureTable {
    let mut n = NomenclatureTable::new();
    n.insert(NomenclatureId::new("cog"), 1, vec![row("01", "Ain")]);
    n.insert(
        NomenclatureId::new("cog"),
        2,
        vec![row("01", "Ain"), row("02", "Aisne")],
    );
    n
}

/// Every row and column of the cast table (§3), the §2.14 unit rule
/// (same unit, unit added/removed, within-dimension, cross-dimension),
/// the §2.12 enum bindings (inline ⊂/⊄, published same id, published
/// vs inline over the same ids) and the §2.15 attachment constraints.
fn types() -> Vec<ScalarType> {
    use ScalarType::*;
    vec![
        Text,
        Boolean,
        Integer(None),
        Integer(Some(Unit::Day)),
        Integer(Some(Unit::Week)),
        Integer(Some(Unit::Metre)),
        Decimal(None),
        Decimal(Some(Unit::Minute)),
        Decimal(Some(Unit::Hour)),
        Date,
        Datetime,
        inline(&[("o1", "Oui"), ("o2", "Non")]),
        inline(&[("o1", "Oui")]),
        inline(&[("01", "Ain"), ("02", "Aisne")]),
        published("cog", 1),
        published("cog", 2),
        attachment(&[], None),
        attachment(&["application/pdf"], Some(1_000)),
        attachment(&["image/*"], None),
        Geometry,
    ]
}

fn file(name: &str, content_type: &str, byte_size: u64) -> Scalar {
    Scalar::Attachment(Box::new(AttachmentRef {
        id: name.into(),
        hash: hash_plain(&CanonicalValue::String(name.into())).unwrap(),
        filename: name.into(),
        content_type: content_type.into(),
        byte_size,
    }))
}

fn feature() -> Scalar {
    Scalar::Geometry(Box::new(
        Feature::parse(
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[2.35,48.85]},"properties":null}"#,
        )
        .unwrap(),
    ))
}

/// Representative *conforming* values for a type — including, for
/// text, the renderings that checked casts to every other type accept.
fn values_for(ty: &ScalarType) -> Vec<Scalar> {
    use ScalarType::*;
    match ty {
        Text => [
            "12",
            "1.5",
            "true",
            "2024-02-29",
            "2024-02-29T10:00:00Z",
            "Oui",
            "Ain",
            "abc",
        ]
        .into_iter()
        .map(|s| Scalar::Text(s.into()))
        .collect(),
        Boolean => vec![Scalar::Boolean(true), Scalar::Boolean(false)],
        Integer(_) => vec![
            Scalar::Integer(7),
            Scalar::Integer(-3),
            Scalar::Integer(1_440),
        ],
        Decimal(_) => ["1.5", "12", "7"]
            .into_iter()
            .map(|s| Scalar::Decimal(varve_core::primitives::Decimal::parse(s).unwrap()))
            .collect(),
        Date => vec![Scalar::Date(
            varve_core::primitives::Date::parse("2024-02-29").unwrap(),
        )],
        Datetime => vec![
            Scalar::Datetime(Instant::parse("2024-02-29T00:00:00Z").unwrap()),
            Scalar::Datetime(Instant::parse("2024-02-29T10:00:00Z").unwrap()),
        ],
        Enum(nref) => varve_schema::nomenclature_rows(nref, &table())
            .unwrap()
            .iter()
            .map(|r| Scalar::Enum(r.id.clone()))
            .collect(),
        Attachment(c) => [
            file("a.pdf", "application/pdf", 500),
            file("b.png", "image/png", 5_000),
        ]
        .into_iter()
        .filter(|s| match s {
            Scalar::Attachment(a) => c.accepts(&a.content_type) && c.admits_size(a.byte_size),
            _ => unreachable!(),
        })
        .collect(),
        Geometry => vec![feature()],
    }
}

fn values(state: CellState) -> RecordValues {
    let mut v = RecordValues::new();
    v.cells.insert(addr(), state);
    v
}

fn one(scalar: &Scalar) -> CellState {
    CellState::Value(CellValue::One(scalar.clone()))
}

#[test]
fn every_value_of_every_type_conforms_to_its_own_column() {
    // The fixture is sound: the values are conforming inputs.
    let n = table();
    for ty in types() {
        let vs = values_for(&ty);
        assert!(!vs.is_empty(), "no values for {ty:?}");
        for v in vs {
            let schema = single(ty.clone(), Arity::One);
            assert_eq!(
                check(&values(one(&v)), &schema, &n),
                vec![],
                "{v:?} in {ty:?}"
            );
        }
    }
}

/// The matrix proper: (from, to) × values, verdict-by-verdict.
#[test]
fn projection_matrix_agrees_with_the_cast_table() {
    let n = table();
    let types = types();
    for from in &types {
        for to in &types {
            let cast = scalar_cast(from, to, &n).unwrap();
            let writer = single(from.clone(), Arity::One);
            let reader = single(to.clone(), Arity::One);
            for value in values_for(from) {
                let ctx = format!("{from:?} → {to:?} with {value:?}");
                let p = project(&values(one(&value)), &writer, &reader, &n).unwrap();
                let col = &p.report.columns[&ColumnId::new("c")];
                let projected = p.values.cells.get(&addr());

                // (a) The output, whatever it is, conforms to the reader.
                assert_eq!(check(&p.values, &reader, &n), vec![], "{ctx}");
                // Every cell is either projected or failed — unless the
                // column has no cast at all, when it is neither.
                assert_eq!(
                    col.cells_projected + col.cells_failed,
                    u64::from(cast.possible),
                    "{ctx}"
                );
                assert_eq!(col.cells_projected == 1, projected.is_some(), "{ctx}");
                assert!(col.cells_lossy <= col.cells_projected, "{ctx}");

                match cast.class() {
                    // (b) No cast: nothing projected, nothing "failed" —
                    // the cell has nowhere to go (§3 "probably breaking").
                    CastClass::Forbidden => {
                        assert_eq!(col.status, ColumnStatus::Forbidden, "{ctx}");
                        assert!(projected.is_none(), "{ctx}");
                        assert_eq!(col.cells_failed, 0, "{ctx}");
                        assert_eq!(col.cells_projected, 0, "{ctx}");
                    }
                    // (c) Identity: copied verbatim, clean.
                    CastClass::Identity => {
                        assert_eq!(col.status, ColumnStatus::Identity, "{ctx}");
                        assert_eq!(projected, Some(&one(&value)), "{ctx}");
                        assert!(p.report.is_clean(), "{ctx}");
                    }
                    // (d) Widening: total, never fails, never loses.
                    CastClass::Widening => {
                        assert_eq!(col.status, ColumnStatus::Cast, "{ctx}");
                        assert!(projected.is_some(), "{ctx}");
                        assert_eq!(col.cells_failed, 0, "{ctx}");
                        assert_eq!(col.cells_lossy, 0, "{ctx}");
                    }
                    // Lossy: total; loss is counted per cell where it
                    // actually happens (checked below, per pair).
                    CastClass::Lossy => {
                        assert_eq!(col.status, ColumnStatus::Cast, "{ctx}");
                        assert!(projected.is_some(), "{ctx}");
                        assert_eq!(col.cells_failed, 0, "{ctx}");
                    }
                    // (e) Checked: kept-and-conforming or failed-and-
                    // counted — the conformance assert above covers the
                    // first half; the totals the second.
                    CastClass::Checked => {
                        assert_eq!(col.status, ColumnStatus::Cast, "{ctx}");
                        assert!(
                            (projected.is_some() && col.cells_failed == 0)
                                || (projected.is_none() && col.cells_failed == 1),
                            "{ctx}: {col:?}"
                        );
                    }
                }
            }
        }
    }
}

/// Every possible cast is a total function on `Empty`: blank is blank
/// under every interpretation (§2.4); a forbidden column drops it.
#[test]
fn empty_cells_pass_through_every_possible_cast() {
    let n = table();
    let types = types();
    for from in &types {
        for to in &types {
            let cast = scalar_cast(from, to, &n).unwrap();
            let p = project(
                &values(CellState::Empty),
                &single(from.clone(), Arity::One),
                &single(to.clone(), Arity::One),
                &n,
            )
            .unwrap();
            let col = &p.report.columns[&ColumnId::new("c")];
            if cast.possible {
                assert_eq!(
                    p.values.cells.get(&addr()),
                    Some(&CellState::Empty),
                    "{from:?} → {to:?}"
                );
                assert_eq!(
                    (col.cells_projected, col.cells_lossy, col.cells_failed),
                    (1, 0, 0)
                );
            } else {
                assert!(p.values.cells.is_empty(), "{from:?} → {to:?}");
                assert_eq!(
                    (col.cells_projected, col.cells_lossy, col.cells_failed),
                    (0, 0, 0)
                );
            }
        }
    }
}

/// (f) Loss is counted where information actually goes, not where it
/// merely could (§5.5 lossiness report).
#[test]
fn loss_is_counted_per_cell_where_it_actually_happens() {
    let n = table();
    let lossy_of = |from: ScalarType, to: ScalarType, value: Scalar| {
        let p = project(
            &values(one(&value)),
            &single(from, Arity::One),
            &single(to, Arity::One),
            &n,
        )
        .unwrap();
        let col = &p.report.columns[&ColumnId::new("c")];
        assert_eq!(col.cells_failed, 0);
        (col.cells_lossy, p.values.cells[&addr()].clone())
    };
    let midnight = Scalar::Datetime(Instant::parse("2024-02-29T00:00:00Z").unwrap());
    let ten = Scalar::Datetime(Instant::parse("2024-02-29T10:00:00Z").unwrap());
    let date = one(&Scalar::Date(Date::parse("2024-02-29").unwrap()));
    // Datetime → Date: lossy only when a time-of-day existed.
    assert_eq!(
        lossy_of(ScalarType::Datetime, ScalarType::Date, midnight),
        (0, date.clone())
    );
    assert_eq!(
        lossy_of(ScalarType::Datetime, ScalarType::Date, ten),
        (1, date)
    );
    // Unit removed (§2.14): the bytes survive, the meaning does not —
    // every cell counts.
    assert_eq!(
        lossy_of(
            ScalarType::Integer(Some(Unit::Day)),
            ScalarType::Integer(None),
            Scalar::Integer(7)
        ),
        (1, one(&Scalar::Integer(7)))
    );
    // Unit added: free, and clean.
    assert_eq!(
        lossy_of(
            ScalarType::Integer(None),
            ScalarType::Integer(Some(Unit::Day)),
            Scalar::Integer(7)
        ),
        (0, one(&Scalar::Integer(7)))
    );
    // Published → inline over the same ids (§2.12): id kept, référentiel
    // lost — lossy per cell. Inline → published (the lift-out): free.
    let cog_inline = inline(&[("01", "Ain"), ("02", "Aisne")]);
    let ain = Scalar::Enum(OptionId::new("01"));
    assert_eq!(
        lossy_of(published("cog", 1), cog_inline.clone(), ain.clone()),
        (1, one(&ain))
    );
    assert_eq!(
        lossy_of(cog_inline, published("cog", 2), ain.clone()),
        (0, one(&ain))
    );
    // Same published nomenclature, higher version: free (append-only).
    assert_eq!(
        lossy_of(published("cog", 1), published("cog", 2), ain.clone()),
        (0, one(&ain))
    );
}

/// §2.14: within-dimension conversion is exact-or-nothing per cell.
#[test]
fn unit_conversion_is_exact_or_fails_the_cell() {
    let n = table();
    let run = |from: ScalarType, to: ScalarType, value: Scalar| {
        let p = project(
            &values(one(&value)),
            &single(from, Arity::One),
            &single(to, Arity::One),
            &n,
        )
        .unwrap();
        let col = &p.report.columns[&ColumnId::new("c")];
        (p.values.cells.get(&addr()).cloned(), col.cells_failed)
    };
    let day = ScalarType::Integer(Some(Unit::Day));
    let week = ScalarType::Integer(Some(Unit::Week));
    assert_eq!(
        run(day.clone(), week.clone(), Scalar::Integer(7)),
        (Some(one(&Scalar::Integer(1))), 0)
    );
    assert_eq!(
        run(day.clone(), week.clone(), Scalar::Integer(-3)),
        (None, 1)
    );
    assert_eq!(
        run(week, day, Scalar::Integer(-3)),
        (Some(one(&Scalar::Integer(-21))), 0)
    );
    let minute = ScalarType::Decimal(Some(Unit::Minute));
    let hour = ScalarType::Decimal(Some(Unit::Hour));
    let d = |s: &str| Scalar::Decimal(Decimal::parse(s).unwrap());
    assert_eq!(
        run(minute.clone(), hour.clone(), d("90")),
        (Some(one(&d("1.5"))), 0)
    );
    assert_eq!(run(minute.clone(), hour.clone(), d("7")), (None, 1));
    assert_eq!(run(hour, minute, d("1.5")), (Some(one(&d("90"))), 0));
}

/// The arity side of the matrix (§3): one→many wraps, many→one
/// truncates (loss iff more than one element), many→many keeps —
/// composed with every scalar identity, and a zero-length list never
/// appears (a many cell narrowing to nothing is `Empty`, §2.4).
#[test]
fn arity_matrix() {
    let n = table();
    for ty in types() {
        let vs = values_for(&ty);
        let first = vs[0].clone();
        let list: Vec<Scalar> = vs.iter().take(2).cloned().collect();
        let one_schema = single(ty.clone(), Arity::One);
        let many_schema = single(ty.clone(), Arity::Many);
        let ctx = format!("{ty:?}");

        // one → many wraps.
        let p = project(&values(one(&first)), &one_schema, &many_schema, &n).unwrap();
        assert_eq!(
            p.values.cells[&addr()],
            CellState::Value(CellValue::Many(vec![first.clone()])),
            "{ctx}"
        );
        assert!(p.report.is_clean(), "{ctx}");
        assert_eq!(check(&p.values, &many_schema, &n), vec![], "{ctx}");

        // many → many keeps.
        let many = CellState::Value(CellValue::Many(list.clone()));
        let p = project(&values(many.clone()), &many_schema, &many_schema, &n).unwrap();
        assert_eq!(p.values.cells[&addr()], many, "{ctx}");
        assert!(p.report.is_clean(), "{ctx}");

        // many → one truncates; loss iff len > 1.
        let p = project(&values(many), &many_schema, &one_schema, &n).unwrap();
        assert_eq!(p.values.cells[&addr()], one(&first), "{ctx}");
        let col = &p.report.columns[&ColumnId::new("c")];
        assert_eq!(col.cells_lossy, u64::from(list.len() > 1), "{ctx}");
        assert_eq!(col.cells_failed, 0, "{ctx}");
        assert_eq!(check(&p.values, &one_schema, &n), vec![], "{ctx}");
        let single_element = CellState::Value(CellValue::Many(vec![first.clone()]));
        let p = project(&values(single_element), &many_schema, &one_schema, &n).unwrap();
        assert_eq!(
            p.report.columns[&ColumnId::new("c")].cells_lossy,
            0,
            "{ctx}"
        );
    }

    // A one→many→one round trip is the identity, and Empty stays Empty
    // in every arity direction.
    let one_schema = single(ScalarType::Text, Arity::One);
    let many_schema = single(ScalarType::Text, Arity::Many);
    let v = values(one(&Scalar::Text("x".into())));
    let wide = project(&v, &one_schema, &many_schema, &n).unwrap();
    let back = project(&wide.values, &many_schema, &one_schema, &n).unwrap();
    assert_eq!(back.values, v);
    for (w, r) in [(&one_schema, &many_schema), (&many_schema, &one_schema)] {
        let p = project(&values(CellState::Empty), w, r, &n).unwrap();
        assert_eq!(p.values.cells[&addr()], CellState::Empty);
        assert!(p.report.is_clean());
    }
}

/// The Attachment→Attachment arm (§2.15): broaden free, narrow checked
/// per claim; case/order in the accept set is not identity-bearing.
#[test]
fn attachment_constraint_changes_recheck_the_claims() {
    let n = table();
    let run = |from: ScalarType, to: ScalarType, value: Scalar| {
        let p = project(
            &values(one(&value)),
            &single(from, Arity::One),
            &single(to, Arity::One),
            &n,
        )
        .unwrap();
        let col = p.report.columns[&ColumnId::new("c")].clone();
        (p.values.cells.get(&addr()).cloned(), col)
    };
    let pdf = file("a.pdf", "application/pdf", 500);
    let png = file("b.png", "image/png", 5_000);
    let any = attachment(&[], None);
    let pdf_only = attachment(&["application/pdf"], Some(1_000));
    let images = attachment(&["image/*"], None);

    // Narrowing the accept set: a non-matching claim fails the cell, a
    // matching one is kept.
    let (cell, col) = run(any.clone(), pdf_only.clone(), png.clone());
    assert_eq!(
        (cell, col.status.clone(), col.cells_failed),
        (None, ColumnStatus::Cast, 1)
    );
    let (cell, col) = run(any.clone(), pdf_only.clone(), pdf.clone());
    assert_eq!((cell, col.cells_failed), (Some(one(&pdf)), 0));
    // Narrowing the size limit rejects a larger file.
    let (cell, col) = run(any.clone(), attachment(&[], Some(1_000)), png.clone());
    assert_eq!((cell, col.cells_failed), (None, 1));
    let (cell, col) = run(any.clone(), attachment(&[], Some(1_000)), pdf.clone());
    assert_eq!((cell, col.cells_failed), (Some(one(&pdf)), 0));
    // Broadening keeps everything, cleanly.
    for (from, value) in [
        (pdf_only.clone(), pdf.clone()),
        (images.clone(), png.clone()),
    ] {
        let (cell, col) = run(from, any.clone(), value.clone());
        assert_eq!(
            (cell, col.status, col.cells_failed, col.cells_lossy),
            (Some(one(&value)), ColumnStatus::Cast, 0, 0)
        );
    }
    // Wildcard subtype: `image/*` admits `image/png`, refuses a pdf.
    let (cell, col) = run(any.clone(), images.clone(), png.clone());
    assert_eq!((cell, col.cells_failed), (Some(one(&png)), 0));
    let (cell, col) = run(any, images.clone(), pdf.clone());
    assert_eq!((cell, col.cells_failed), (None, 1));
    // pdf-only → images: neither covers the other — checked, and the
    // pdf claim fails.
    let (cell, col) = run(pdf_only.clone(), images, pdf.clone());
    assert_eq!(
        (cell, col.status, col.cells_failed),
        (None, ColumnStatus::Cast, 1)
    );
    // Constraints differing only by case and order are one constraint:
    // Identity status, verbatim copy.
    let shuffled = attachment(&["Image/*", "APPLICATION/pdf"], Some(1_000));
    let sorted = attachment(&["application/pdf", "image/*"], Some(1_000));
    let (cell, col) = run(shuffled, sorted, pdf.clone());
    assert_eq!(
        (cell, col.status),
        (Some(one(&pdf)), ColumnStatus::Identity)
    );
}
