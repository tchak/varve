//! The cast table and its dual, the type join (§7, §3, §5.5).
//!
//! The compatibility relation between two types is a property of the type
//! system itself. Casts classify per §3 (safe / lossy / checked); safe
//! casts ("widens to") define the partial order whose least upper bound
//! builds aggregate revisions (§5.5).
//!
//! Enum rules are §2.11's: enum→enum compatibility is id-set comparison
//! (option added = free, removed = checked with an exact impact count);
//! enum→text materializes labels and therefore needs a reading lens.

use std::collections::{BTreeMap, BTreeSet};

use strata_core::{NomenclatureId, OptionId};

use crate::{Arity, NomenclatureRef, OptionRow, ScalarType};

/// Published nomenclature rows, keyed by identity. Inline nomenclatures
/// carry their rows in the schema; published ones travel like blocks and
/// are resolved through this table (§2.12).
pub type NomenclatureTable = BTreeMap<NomenclatureId, Vec<OptionRow>>;

/// How one type reaches another. Properties are orthogonal: an arity +
/// scalar composition can be lossy *and* checked at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cast {
    /// False: no cast exists (§3 "probably breaking").
    pub possible: bool,
    /// Total but information-losing — must appear in a lossiness report.
    pub lossy: bool,
    /// Value-dependent: may fail per cell. The impact report counts the
    /// records whose cells fail (§7 `strata-impact`).
    pub checked: bool,
    /// Materializes enum labels: resolved through a reading revision
    /// (§2.11); which one is the projection's lens.
    pub needs_lens: bool,
    identity: bool,
}

impl Cast {
    pub const IDENTITY: Cast = Cast {
        possible: true,
        lossy: false,
        checked: false,
        needs_lens: false,
        identity: true,
    };
    pub const WIDENING: Cast = Cast {
        identity: false,
        ..Cast::IDENTITY
    };
    pub const LOSSY: Cast = Cast {
        lossy: true,
        ..Cast::WIDENING
    };
    pub const CHECKED: Cast = Cast {
        checked: true,
        ..Cast::WIDENING
    };
    pub const FORBIDDEN: Cast = Cast {
        possible: false,
        ..Cast::WIDENING
    };

    pub const fn with_lens(self) -> Cast {
        Cast {
            needs_lens: true,
            ..self
        }
    }

    /// Sequential composition: scalar cast then arity cast (or the
    /// reverse — properties are order-independent).
    pub fn and(self, other: Cast) -> Cast {
        Cast {
            possible: self.possible && other.possible,
            lossy: self.lossy || other.lossy,
            checked: self.checked || other.checked,
            needs_lens: self.needs_lens || other.needs_lens,
            identity: self.identity && other.identity,
        }
    }

    /// Pure widening: the §5.5 "widens to" relation.
    pub fn is_widening(&self) -> bool {
        self.possible && !self.lossy && !self.checked
    }

    pub fn class(&self) -> CastClass {
        if !self.possible {
            CastClass::Forbidden
        } else if self.checked {
            CastClass::Checked
        } else if self.lossy {
            CastClass::Lossy
        } else if self.identity {
            CastClass::Identity
        } else {
            CastClass::Widening
        }
    }
}

/// Display/report classification, most severe property wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastClass {
    Identity,
    Widening,
    Lossy,
    Checked,
    Forbidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastError {
    UnknownNomenclature(NomenclatureId),
}

/// The rows an enum's nomenclature reference resolves to: inline rows
/// directly, published ones through the table.
pub fn nomenclature_rows<'a>(
    nref: &'a NomenclatureRef,
    nomenclatures: &'a NomenclatureTable,
) -> Result<&'a [OptionRow], CastError> {
    match nref {
        NomenclatureRef::Inline(rows) => Ok(rows),
        NomenclatureRef::Published { id, .. } => nomenclatures
            .get(id)
            .map(Vec::as_slice)
            .ok_or_else(|| CastError::UnknownNomenclature(id.clone())),
    }
}

fn option_ids(
    nref: &NomenclatureRef,
    nomenclatures: &NomenclatureTable,
) -> Result<BTreeSet<OptionId>, NomenclatureId> {
    match nref {
        NomenclatureRef::Inline(rows) => {
            Ok(rows.iter().map(|r| r.id.clone()).collect())
        }
        NomenclatureRef::Published { id, .. } => nomenclatures
            .get(id)
            .map(|rows| rows.iter().map(|r| r.id.clone()).collect())
            .ok_or_else(|| id.clone()),
    }
}

/// Every scalar with an unambiguous canonical text rendering widens to
/// `Text`. Attachments and geometry do not — they are not text.
fn widens_to_text(ty: &ScalarType) -> bool {
    !matches!(ty, ScalarType::Attachment | ScalarType::Geometry)
}

/// The cast table between two scalar types (§3).
pub fn scalar_cast(
    from: &ScalarType,
    to: &ScalarType,
    nomenclatures: &NomenclatureTable,
) -> Result<Cast, CastError> {
    use ScalarType::*;
    Ok(match (from, to) {
        (a, b) if a == b => Cast::IDENTITY,
        // §2.11: id-set comparison. Target superset → free; otherwise
        // cells holding removed ids fail, and are countable exactly.
        (Enum(a), Enum(b)) => {
            let from_ids =
                option_ids(a, nomenclatures).map_err(CastError::UnknownNomenclature)?;
            let to_ids =
                option_ids(b, nomenclatures).map_err(CastError::UnknownNomenclature)?;
            if from_ids.is_subset(&to_ids) {
                Cast::WIDENING
            } else {
                Cast::CHECKED
            }
        }
        (Integer, Decimal) => Cast::WIDENING,
        // Exact-or-fail: administrations do not want silent truncation.
        (Decimal, Integer) => Cast::CHECKED,
        // Injective embedding at midnight UTC; reversible.
        (Date, Datetime) => Cast::WIDENING,
        (Datetime, Date) => Cast::LOSSY,
        (Enum(_), Text) => Cast::WIDENING.with_lens(),
        (a, Text) if widens_to_text(a) => Cast::WIDENING,
        (Text, Enum(_)) => Cast::CHECKED.with_lens(),
        (Text, Boolean | Integer | Decimal | Date | Datetime) => Cast::CHECKED,
        _ => Cast::FORBIDDEN,
    })
}

/// Arity casts (§3: arity change = cast required). `many → one` is the
/// first genuinely lossy cast that isn't type narrowing.
pub fn arity_cast(from: Arity, to: Arity) -> Cast {
    match (from, to) {
        (Arity::One, Arity::One) | (Arity::Many, Arity::Many) => Cast::IDENTITY,
        (Arity::One, Arity::Many) => Cast::WIDENING,
        (Arity::Many, Arity::One) => Cast::LOSSY,
    }
}

/// The per-column compatibility relation — exactly Avro's reader/writer
/// resolution shape (§3).
pub fn column_cast(
    from: (&ScalarType, Arity),
    to: (&ScalarType, Arity),
    nomenclatures: &NomenclatureTable,
) -> Result<Cast, CastError> {
    Ok(scalar_cast(from.0, to.0, nomenclatures)?.and(arity_cast(from.1, to.1)))
}

/// How a join was reached. `ViaText` is the §5.5 "widen to opaque text"
/// outcome applied where it is the genuine least upper bound — it must
/// appear in the AggregateReport, or aggregation quietly stringifies
/// typed data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinPath {
    Direct,
    ViaText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinConflict {
    /// No upper bound exists (attachment/geometry against anything else):
    /// §5.5 policy territory — split or omit, never silently coerce.
    Incompatible,
    UnknownNomenclature(NomenclatureId),
}

/// Least upper bound of two scalars in the widening order (§5.5).
pub fn scalar_join(
    a: &ScalarType,
    b: &ScalarType,
    nomenclatures: &NomenclatureTable,
) -> Result<(ScalarType, JoinPath), JoinConflict> {
    use ScalarType::*;
    if a == b {
        return Ok((a.clone(), JoinPath::Direct));
    }
    match (a, b) {
        (Integer, Decimal) | (Decimal, Integer) => Ok((Decimal, JoinPath::Direct)),
        (Date, Datetime) | (Datetime, Date) => Ok((Datetime, JoinPath::Direct)),
        (Enum(x), Enum(y)) => enum_join(x, y, nomenclatures),
        (Text, other) | (other, Text) if widens_to_text(other) => {
            Ok((Text, JoinPath::Direct))
        }
        (a, b) if widens_to_text(a) && widens_to_text(b) => {
            Ok((Text, JoinPath::ViaText))
        }
        _ => Err(JoinConflict::Incompatible),
    }
}

/// Join of two enums.
///
/// Same published nomenclature: the higher version, on the §2.11
/// assumption that nomenclature versions are append-only (removal is
/// deprecation, ids are never deleted), so the higher version's id set
/// contains the lower's. Two inline enums merge row-wise when shared ids
/// agree on labels. Everything else falls back to the genuine upper
/// bound both sides widen to: `Text`, reported as `ViaText`.
fn enum_join(
    x: &NomenclatureRef,
    y: &NomenclatureRef,
    nomenclatures: &NomenclatureTable,
) -> Result<(ScalarType, JoinPath), JoinConflict> {
    match (x, y) {
        (
            NomenclatureRef::Published { id: xi, version: xv },
            NomenclatureRef::Published { id: yi, version: yv },
        ) if xi == yi => Ok((
            ScalarType::Enum(NomenclatureRef::Published {
                id: xi.clone(),
                version: *xv.max(yv),
            }),
            JoinPath::Direct,
        )),
        (NomenclatureRef::Inline(xr), NomenclatureRef::Inline(yr)) => {
            let mut merged: Vec<OptionRow> = xr.clone();
            for row in yr {
                match merged.iter().find(|r| r.id == row.id) {
                    None => merged.push(row.clone()),
                    Some(existing) if existing.label == row.label => {}
                    // Same synthesized id, different meaning: these are
                    // unrelated enums that happen to collide.
                    Some(_) => return Ok((ScalarType::Text, JoinPath::ViaText)),
                }
            }
            Ok((
                ScalarType::Enum(NomenclatureRef::Inline(merged)),
                JoinPath::Direct,
            ))
        }
        _ => {
            // Different codelists, or inline vs published: both widen to
            // text; verify the table knows any published side first.
            for nref in [x, y] {
                if let Err(id) = option_ids(nref, nomenclatures) {
                    return Err(JoinConflict::UnknownNomenclature(id));
                }
            }
            Ok((ScalarType::Text, JoinPath::ViaText))
        }
    }
}

/// Arity join: §5.5 "widen to `many` where possible".
pub fn arity_join(a: Arity, b: Arity) -> Arity {
    if a == Arity::Many || b == Arity::Many {
        Arity::Many
    } else {
        Arity::One
    }
}

/// Column-level join: scalar LUB plus arity join.
pub fn column_join(
    a: (&ScalarType, Arity),
    b: (&ScalarType, Arity),
    nomenclatures: &NomenclatureTable,
) -> Result<((ScalarType, Arity), JoinPath), JoinConflict> {
    let (ty, path) = scalar_join(a.0, b.0, nomenclatures)?;
    Ok(((ty, arity_join(a.1, b.1)), path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::OptionId;

    fn inline(pairs: &[(&str, &str)]) -> NomenclatureRef {
        NomenclatureRef::Inline(
            pairs
                .iter()
                .map(|(id, label)| OptionRow {
                    id: OptionId::new(*id),
                    label: (*label).to_string(),
                    fields: vec![],
                })
                .collect(),
        )
    }

    fn no_noms() -> NomenclatureTable {
        NomenclatureTable::new()
    }

    #[test]
    fn cast_table_rows() {
        use ScalarType::*;
        let n = no_noms();
        let cast = |a: &ScalarType, b: &ScalarType| scalar_cast(a, b, &n).unwrap();
        assert_eq!(cast(&Text, &Text).class(), CastClass::Identity);
        assert_eq!(cast(&Integer, &Decimal).class(), CastClass::Widening);
        assert_eq!(cast(&Decimal, &Integer).class(), CastClass::Checked);
        assert_eq!(cast(&Date, &Datetime).class(), CastClass::Widening);
        assert_eq!(cast(&Datetime, &Date).class(), CastClass::Lossy);
        assert_eq!(cast(&Integer, &Text).class(), CastClass::Widening);
        assert_eq!(cast(&Text, &Integer).class(), CastClass::Checked);
        assert_eq!(cast(&Attachment, &Text).class(), CastClass::Forbidden);
        assert_eq!(cast(&Geometry, &Attachment).class(), CastClass::Forbidden);

        let e = Enum(inline(&[("o1", "Oui")]));
        let to_text = cast(&e, &Text);
        assert_eq!(to_text.class(), CastClass::Widening);
        assert!(to_text.needs_lens);
        let from_text = cast(&Text, &e);
        assert_eq!(from_text.class(), CastClass::Checked);
        assert!(from_text.needs_lens);
    }

    #[test]
    fn enum_cast_is_id_set_comparison() {
        use ScalarType::Enum;
        let n = no_noms();
        let small = Enum(inline(&[("o1", "Oui")]));
        let big = Enum(inline(&[("o1", "Oui"), ("o2", "Non")]));
        // Option added → free; option removed → checked (§3).
        assert!(scalar_cast(&small, &big, &n).unwrap().is_widening());
        assert_eq!(
            scalar_cast(&big, &small, &n).unwrap().class(),
            CastClass::Checked
        );
        // Relabel only: same ids, still free — renames cost nothing
        // (§2.11).
        let relabeled = Enum(inline(&[("o1", "Oui bien sûr")]));
        assert!(scalar_cast(&small, &relabeled, &n).unwrap().is_widening());
    }

    #[test]
    fn unknown_published_nomenclature_errors() {
        use ScalarType::Enum;
        let published = Enum(NomenclatureRef::Published {
            id: NomenclatureId::new("insee-pays"),
            version: 1,
        });
        let other = Enum(inline(&[("o1", "x")]));
        assert!(matches!(
            scalar_cast(&published, &other, &no_noms()),
            Err(CastError::UnknownNomenclature(_))
        ));
    }

    #[test]
    fn arity_and_column_composition() {
        use ScalarType::*;
        let n = no_noms();
        assert!(arity_cast(Arity::One, Arity::Many).is_widening());
        assert_eq!(arity_cast(Arity::Many, Arity::One).class(), CastClass::Lossy);
        // Decimal-many → Integer-one: checked (per element) AND lossy
        // (arity) at once — why Cast is properties, not one enum.
        let cast = column_cast(
            (&Decimal, Arity::Many),
            (&Integer, Arity::One),
            &n,
        )
        .unwrap();
        assert!(cast.lossy && cast.checked);
        assert_eq!(cast.class(), CastClass::Checked);
    }

    #[test]
    fn joins() {
        use ScalarType::*;
        let n = no_noms();
        let join = |a: &ScalarType, b: &ScalarType| scalar_join(a, b, &n).unwrap();
        assert_eq!(join(&Integer, &Decimal), (Decimal, JoinPath::Direct));
        assert_eq!(join(&Date, &Datetime), (Datetime, JoinPath::Direct));
        assert_eq!(join(&Integer, &Text), (Text, JoinPath::Direct));
        // Neither side is text: the LUB is Text but it must be reported.
        assert_eq!(join(&Integer, &Date), (Text, JoinPath::ViaText));
        assert_eq!(join(&Boolean, &Integer), (Text, JoinPath::ViaText));
        assert_eq!(
            scalar_join(&Attachment, &Integer, &n),
            Err(JoinConflict::Incompatible)
        );
        assert_eq!(arity_join(Arity::One, Arity::Many), Arity::Many);
    }

    #[test]
    fn enum_joins() {
        use ScalarType::Enum;
        let n = no_noms();
        // Inline merge: union when shared ids agree on labels.
        let a = Enum(inline(&[("o1", "Oui")]));
        let b = Enum(inline(&[("o1", "Oui"), ("o2", "Non")]));
        let (joined, path) = scalar_join(&a, &b, &n).unwrap();
        assert_eq!(path, JoinPath::Direct);
        assert_eq!(joined, b);
        // Same id, different meaning: unrelated enums → ViaText.
        let c = Enum(inline(&[("o1", "Rouge")]));
        assert_eq!(scalar_join(&a, &c, &n).unwrap(), (ScalarType::Text, JoinPath::ViaText));
        // Same published nomenclature: higher version (append-only §2.11).
        let v1 = Enum(NomenclatureRef::Published {
            id: NomenclatureId::new("cog"),
            version: 1,
        });
        let v3 = Enum(NomenclatureRef::Published {
            id: NomenclatureId::new("cog"),
            version: 3,
        });
        assert_eq!(scalar_join(&v1, &v3, &n).unwrap(), (v3, JoinPath::Direct));
    }

    #[test]
    fn join_is_an_upper_bound_in_the_widening_order() {
        use ScalarType::*;
        let n = no_noms();
        let samples = [
            Text,
            Boolean,
            Integer,
            Decimal,
            Date,
            Datetime,
            Enum(inline(&[("o1", "Oui"), ("o2", "Non")])),
            Attachment,
            Geometry,
        ];
        for a in &samples {
            for b in &samples {
                let Ok((joined, _)) = scalar_join(a, b, &n) else {
                    continue;
                };
                for side in [a, b] {
                    let cast = scalar_cast(side, &joined, &n).unwrap();
                    assert!(
                        cast.is_widening() || cast.class() == CastClass::Identity,
                        "join({a:?}, {b:?}) = {joined:?} is not reachable \
                         from {side:?} by pure widening"
                    );
                }
            }
        }
    }
}
