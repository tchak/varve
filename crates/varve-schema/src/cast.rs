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

use varve_core::{NomenclatureId, OptionId};

use crate::{Arity, AttachmentConstraints, NomenclatureRef, OptionRow, ScalarType, Unit};

/// Published nomenclature rows, keyed by identity **and version**
/// (§2.12): a column binds `(nomenclature, version, id)`, so every
/// lookup names the version it was declared against — a closed id set
/// the checker, casts and conformance can rely on. Inline nomenclatures
/// carry their rows in the schema; published ones travel like blocks
/// and are resolved through this table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NomenclatureTable {
    versions: BTreeMap<NomenclatureId, BTreeMap<u32, Vec<OptionRow>>>,
}

impl NomenclatureTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one version's rows. Re-inserting a version replaces it.
    pub fn insert(&mut self, id: NomenclatureId, version: u32, rows: Vec<OptionRow>) {
        self.versions.entry(id).or_default().insert(version, rows);
    }

    /// The rows of exactly this version — never a newer one.
    pub fn get(&self, id: &NomenclatureId, version: u32) -> Option<&[OptionRow]> {
        self.versions.get(id)?.get(&version).map(Vec::as_slice)
    }

    /// Every version known for a nomenclature, ascending.
    pub fn versions(&self, id: &NomenclatureId) -> impl Iterator<Item = (u32, &[OptionRow])> {
        self.versions
            .get(id)
            .into_iter()
            .flat_map(|v| v.iter().map(|(n, rows)| (*n, rows.as_slice())))
    }

    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }
}

/// How one type reaches another. Properties are orthogonal: an arity +
/// scalar composition can be lossy *and* checked at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cast {
    /// False: no cast exists (§3 "probably breaking").
    pub possible: bool,
    /// Total but information-losing — must appear in a lossiness report.
    pub lossy: bool,
    /// Value-dependent: may fail per cell. The impact report counts the
    /// records whose cells fail (§7 `varve-impact`).
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CastError {
    #[error("unknown published nomenclature '{0}' version {1}")]
    UnknownNomenclature(NomenclatureId, u32),
}

/// The rows an enum's nomenclature reference resolves to: inline rows
/// directly, published ones through the table — at the **declared
/// version**, never a newer one.
pub fn nomenclature_rows<'a>(
    nref: &'a NomenclatureRef,
    nomenclatures: &'a NomenclatureTable,
) -> Result<&'a [OptionRow], CastError> {
    match nref {
        NomenclatureRef::Inline(rows) => Ok(rows),
        NomenclatureRef::Published { id, version } => nomenclatures
            .get(id, *version)
            .ok_or_else(|| CastError::UnknownNomenclature(id.clone(), *version)),
    }
}

fn option_ids(
    nref: &NomenclatureRef,
    nomenclatures: &NomenclatureTable,
) -> Result<BTreeSet<OptionId>, (NomenclatureId, u32)> {
    nomenclature_rows(nref, nomenclatures)
        .map(|rows| rows.iter().map(|r| r.id.clone()).collect())
        .map_err(|e| match e {
            CastError::UnknownNomenclature(id, version) => (id, version),
        })
}

/// Every scalar with an unambiguous canonical text rendering widens to
/// `Text`. Attachments and geometry do not — they are not text.
fn widens_to_text(ty: &ScalarType) -> bool {
    !matches!(ty, ScalarType::Attachment(_) | ScalarType::Geometry)
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
            let from_ids = option_ids(a, nomenclatures)
                .map_err(|(id, v)| CastError::UnknownNomenclature(id, v))?;
            let to_ids = option_ids(b, nomenclatures)
                .map_err(|(id, v)| CastError::UnknownNomenclature(id, v))?;
            if from_ids.is_subset(&to_ids) {
                Cast::WIDENING
            } else {
                Cast::CHECKED
            }
        }
        // §2.15: broaden free, narrow checked — the enum id-set rules
        // replayed over media-type patterns and size limits.
        (Attachment(from), Attachment(to)) => {
            if to.covers(from) {
                Cast::WIDENING
            } else {
                Cast::CHECKED
            }
        }
        // Number casts compose the representation rule with the §2.14
        // unit rule. Exact-or-fail: administrations do not want silent
        // truncation or rounding.
        (Integer(fu), Integer(tu)) => number_cast(Cast::IDENTITY, *fu, *tu),
        (Integer(fu), Decimal(tu)) => number_cast(Cast::WIDENING, *fu, *tu),
        (Decimal(fu), Decimal(tu)) => number_cast(Cast::IDENTITY, *fu, *tu),
        (Decimal(fu), Integer(tu)) => number_cast(Cast::CHECKED, *fu, *tu),
        // Injective embedding at midnight UTC; reversible.
        (Date, Datetime) => Cast::WIDENING,
        (Datetime, Date) => Cast::LOSSY,
        (Enum(_), Text) => Cast::WIDENING.with_lens(),
        (a, Text) if widens_to_text(a) => Cast::WIDENING,
        (Text, Enum(_)) => Cast::CHECKED.with_lens(),
        (Text, Boolean | Integer(_) | Decimal(_) | Date | Datetime) => Cast::CHECKED,
        _ => Cast::FORBIDDEN,
    })
}

/// §2.14 unit rule, composed onto the representation cast: same unit
/// defers to the representation; **unit added** is widening (adds
/// meaning — the values were always implicitly in that unit; never
/// identity, so the impact report sees the semantic change); **unit
/// removed** is lossy (drops meaning); a change within a dimension is
/// exact-or-fail; a dimension change is no cast at all. The asymmetry
/// is what makes "widens to" a partial order (§5.5): were both
/// directions free, `day → none → week` would compose two free casts
/// into the unit swap the direct cast refuses.
fn number_cast(repr: Cast, from: Option<Unit>, to: Option<Unit>) -> Cast {
    match (from, to) {
        (a, b) if a == b => repr,
        (None, Some(_)) => repr.and(Cast::WIDENING),
        (Some(_), None) => repr.and(Cast::LOSSY),
        (Some(a), Some(b)) if a.dimension() == b.dimension() => {
            repr.and(Cast::CHECKED)
        }
        _ => Cast::FORBIDDEN,
    }
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JoinConflict {
    /// No upper bound exists (attachment/geometry against anything else):
    /// §5.5 policy territory — split or omit, never silently coerce.
    #[error("no type join exists — split or omit (§5.5 policy)")]
    Incompatible,
    #[error("unknown published nomenclature '{0}' version {1}")]
    UnknownNomenclature(NomenclatureId, u32),
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
        // §2.15: union of accepts (either-unrestricted wins), max of
        // limits — always an upper bound, always Direct.
        (Attachment(x), Attachment(y)) => {
            Ok((Attachment(attachment_join(x, y)), JoinPath::Direct))
        }
        (Integer(a), Integer(b)) => Ok(number_join(false, *a, *b)),
        (Integer(a), Decimal(b))
        | (Decimal(a), Integer(b))
        | (Decimal(a), Decimal(b)) => Ok(number_join(true, *a, *b)),
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
/// contains the lower's. Two inline enums merge row-wise by id — a
/// shared id with two labels is a **rename** (§2.11: ids are identity,
/// labels are interpretation), so the merged row keeps the id and takes
/// the right-hand label (the aggregate folds history forward, so the
/// later revision's label wins; the aggregate's own surface takes
/// labels from the latest revision anyway). Everything else falls back
/// to the genuine upper bound both sides widen to: `Text`, reported as
/// `ViaText`.
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
                match merged.iter_mut().find(|r| r.id == row.id) {
                    None => merged.push(row.clone()),
                    // Same id: the right-hand row's label and fields win
                    // (a rename, not a conflict — §2.11).
                    Some(existing) => *existing = row.clone(),
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
                if let Err((id, version)) = option_ids(nref, nomenclatures) {
                    return Err(JoinConflict::UnknownNomenclature(id, version));
                }
            }
            Ok((ScalarType::Text, JoinPath::ViaText))
        }
    }
}

/// Unit side of the number join: equal units keep; Some beats None (a
/// united column carries the meaning — the unitless side reaches it by
/// widening, and not the reverse: dropping a unit is lossy); two
/// different units have no upper bound in the widening order
/// (conversion is checked, not widening; the unitless type is *below*
/// both, not above), so the genuine LUB is `Text`, reported `ViaText`
/// (§5.5).
fn number_join(
    decimal: bool,
    a: Option<Unit>,
    b: Option<Unit>,
) -> (ScalarType, JoinPath) {
    let unit = match (a, b) {
        (None, None) => None,
        (Some(x), Some(y)) if x == y => Some(x),
        (Some(u), None) | (None, Some(u)) => Some(u),
        (Some(_), Some(_)) => return (ScalarType::Text, JoinPath::ViaText),
    };
    let ty = if decimal {
        ScalarType::Decimal(unit)
    } else {
        ScalarType::Integer(unit)
    };
    (ty, JoinPath::Direct)
}

fn attachment_join(
    a: &AttachmentConstraints,
    b: &AttachmentConstraints,
) -> AttachmentConstraints {
    let accept = if a.accept.is_empty() || b.accept.is_empty() {
        Vec::new() // unrestricted is the top
    } else {
        let mut union: Vec<String> =
            a.accept.iter().chain(&b.accept).cloned().collect();
        union.sort();
        union.dedup();
        union
    };
    let max_bytes = match (a.max_bytes, b.max_bytes) {
        (Some(x), Some(y)) => Some(x.max(y)),
        _ => None,
    };
    AttachmentConstraints { accept, max_bytes }
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
    use varve_core::OptionId;

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
        assert_eq!(cast(&Integer(None), &Decimal(None)).class(), CastClass::Widening);
        assert_eq!(cast(&Decimal(None), &Integer(None)).class(), CastClass::Checked);
        assert_eq!(cast(&Date, &Datetime).class(), CastClass::Widening);
        assert_eq!(cast(&Datetime, &Date).class(), CastClass::Lossy);
        assert_eq!(cast(&Integer(None), &Text).class(), CastClass::Widening);
        assert_eq!(cast(&Text, &Integer(None)).class(), CastClass::Checked);
        assert_eq!(
            cast(&Attachment(Default::default()), &Text).class(),
            CastClass::Forbidden
        );
        assert_eq!(
            cast(&Geometry, &Attachment(Default::default())).class(),
            CastClass::Forbidden
        );

        // §2.14 unit rows.
        let m = Integer(Some(Unit::Metre));
        let km_int = Integer(Some(Unit::Kilometre));
        let km_dec = Decimal(Some(Unit::Kilometre));
        let month = Integer(Some(Unit::Month));
        assert_eq!(cast(&m, &km_int).class(), CastClass::Checked);
        assert_eq!(cast(&m, &km_dec).class(), CastClass::Checked);
        // Unit added: widening (never identity); unit removed: lossy —
        // the asymmetry that keeps "widens to" a partial order (§5.5).
        assert_eq!(cast(&Integer(None), &m).class(), CastClass::Widening);
        assert_ne!(cast(&Integer(None), &m).class(), CastClass::Identity);
        assert_eq!(cast(&m, &Integer(None)).class(), CastClass::Lossy);
        // Two free casts never compose into what the direct cast refuses:
        // day → none is lossy, so day → none → week is not free.
        assert_eq!(cast(&Integer(Some(Unit::Day)), &Integer(Some(Unit::Week))).class(), CastClass::Checked);
        // Cross-dimension: no cast (days ↔ months included).
        assert_eq!(cast(&m, &month).class(), CastClass::Forbidden);
        assert_eq!(
            cast(&Integer(Some(Unit::Day)), &month).class(),
            CastClass::Forbidden
        );

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
    fn published_enum_casts_honour_the_bound_version() {
        // §2.12: a column binds (nomenclature, version, id). v2 ⊇ v1
        // (append-only), so v1→v2 is free and v2→v1 is checked — cells
        // holding ids v1 never had are exactly the countable failures.
        use ScalarType::Enum;
        let cog = NomenclatureId::new("cog");
        let mut n = NomenclatureTable::new();
        n.insert(cog.clone(), 1, vec![OptionRow { id: OptionId::new("01"), label: "Ain".into(), fields: vec![] }]);
        n.insert(
            cog.clone(),
            2,
            vec![
                OptionRow { id: OptionId::new("01"), label: "Ain".into(), fields: vec![] },
                OptionRow { id: OptionId::new("02"), label: "Aisne".into(), fields: vec![] },
            ],
        );
        let v1 = Enum(NomenclatureRef::Published { id: cog.clone(), version: 1 });
        let v2 = Enum(NomenclatureRef::Published { id: cog.clone(), version: 2 });
        assert!(scalar_cast(&v1, &v2, &n).unwrap().is_widening());
        assert!(scalar_cast(&v2, &v1, &n).unwrap().checked);
        // Rows resolve at the declared version — never the latest.
        assert_eq!(nomenclature_rows(&NomenclatureRef::Published { id: cog.clone(), version: 1 }, &n).unwrap().len(), 1);
        // An unknown *version* is unknown, even when the id is known.
        assert_eq!(
            scalar_cast(&v1, &Enum(NomenclatureRef::Published { id: cog.clone(), version: 3 }), &n),
            Err(CastError::UnknownNomenclature(cog, 3))
        );
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
            Err(CastError::UnknownNomenclature(..))
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
            (&Decimal(None), Arity::Many),
            (&Integer(None), Arity::One),
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
        assert_eq!(join(&Integer(None), &Decimal(None)), (Decimal(None), JoinPath::Direct));
        assert_eq!(join(&Date, &Datetime), (Datetime, JoinPath::Direct));
        assert_eq!(join(&Integer(None), &Text), (Text, JoinPath::Direct));
        // Neither side is text: the LUB is Text but it must be reported.
        assert_eq!(join(&Integer(None), &Date), (Text, JoinPath::ViaText));
        assert_eq!(join(&Boolean, &Integer(None)), (Text, JoinPath::ViaText));
        assert_eq!(
            scalar_join(&Attachment(Default::default()), &Integer(None), &n),
            Err(JoinConflict::Incompatible)
        );
        assert_eq!(arity_join(Arity::One, Arity::Many), Arity::Many);

        // §2.14 unit joins: Some beats None (free reinterpretation
        // reaches it); two different units have no upper bound but Text.
        let day = Integer(Some(Unit::Day));
        assert_eq!(join(&Integer(None), &day), (day.clone(), JoinPath::Direct));
        assert_eq!(
            join(&day, &Decimal(None)),
            (Decimal(Some(Unit::Day)), JoinPath::Direct)
        );
        assert_eq!(
            join(&day, &Integer(Some(Unit::Week))),
            (Text, JoinPath::ViaText)
        );
        assert_eq!(
            join(&day, &Integer(Some(Unit::Month))),
            (Text, JoinPath::ViaText)
        );
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
        // Same id, different label: a rename (§2.11) — ids kept, the
        // right-hand label wins; both sides widen to the result.
        let c = Enum(inline(&[("o1", "Rouge")]));
        assert_eq!(scalar_join(&a, &c, &n).unwrap(), (c.clone(), JoinPath::Direct));
        assert!(scalar_cast(&a, &c, &n).unwrap().is_widening());
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
            Integer(None),
            Integer(Some(Unit::Day)),
            Integer(Some(Unit::Month)),
            Decimal(None),
            Decimal(Some(Unit::Metre)),
            Date,
            Datetime,
            Enum(inline(&[("o1", "Oui"), ("o2", "Non")])),
            Attachment(Default::default()),
            Attachment(AttachmentConstraints {
                accept: vec!["application/pdf".into()],
                max_bytes: Some(10_000_000),
            }),
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
