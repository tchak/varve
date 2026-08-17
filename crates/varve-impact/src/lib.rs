//! Tier 2 (§7): what does publishing revision N+1 do?
//!
//! The impact report is the product artifact no competitor offers
//! (§1): shown to an administration *before* it publishes. This crate
//! covers change classification (§3), the resolver impact questions
//! (§2.8), and record assessment (running the projection over real
//! records and counting). Broken rule references arrive with
//! `varve-logic`; statically-unreachable required columns with
//! `varve-surface`.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use varve_core::{ColumnId, OptionId, ResolverId};
use varve_projection::project;
use varve_schema::{
    Cast, CastClass, CastError, NomenclatureTable, ResolverDeclaration,
    ScalarType, Schema, SchemaIndex, Unit, column_cast, nomenclature_rows,
};
use varve_value::RecordValues;

/// §8 M1's vocabulary: every transition classifies as safe, lossy, or
/// breaking — with `Checked` in between: total classification needs
/// data, which is what `assess` provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeClass {
    Safe,
    Lossy,
    /// Value-dependent: some records may fail the cast. The record
    /// assessment turns this into an exact count.
    Checked,
    Breaking,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnChange {
    /// Reader-only: every record reads absent (§3 — free).
    Added,
    /// Writer-only: ignored, retained (§3 — free).
    Removed,
    /// Same type, arity, and scope. Moves *within* a scope are this —
    /// order is presentation, not addressing.
    Identical,
    /// Type or arity changed; the cast tells the story.
    Retyped { cast: Cast },
    /// §3 correction: moved into or out of a `many` group — breaking.
    ScopeMoved,
    /// No cast exists between the types.
    Forbidden,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnImpact {
    pub change: ColumnChange,
    pub class: ChangeClass,
    /// For enum transitions that drop options (§2.11): exactly which
    /// ids — the records holding them are the ones that will fail.
    pub removed_options: Vec<OptionId>,
    /// For number transitions whose unit changed (§2.14): named
    /// explicitly, because a unit added or removed is a *semantic*
    /// change even when the cast is free — values unchanged, meaning
    /// changed — and the report must say so.
    pub unit_change: Option<UnitChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitChange {
    pub from: Option<Unit>,
    pub to: Option<Unit>,
}

/// The §2.8 impact questions, answered per resolver.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolverChange {
    Added { id: ResolverId },
    /// Which columns are orphaned (still exist in the new revision but
    /// nothing feeds them anymore); pending resolutions against this
    /// resolver can never land.
    Removed {
        id: ResolverId,
        orphaned_columns: Vec<ColumnId>,
    },
    /// Which cells are stale — and re-derivable from retained snapshots
    /// (§2.7): the mapped target columns.
    MappingChanged {
        id: ResolverId,
        stale_columns: Vec<ColumnId>,
    },
    VersionChanged { id: ResolverId, from: u32, to: u32 },
}

/// Aggregated over a record set by `assess`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordAssessment {
    pub records: u64,
    pub records_with_failures: u64,
    pub records_with_loss: u64,
    pub cells_failed: u64,
    pub cells_lossy: u64,
    pub failed_by_column: BTreeMap<ColumnId, u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImpactReport {
    pub columns: BTreeMap<ColumnId, ColumnImpact>,
    pub resolvers: Vec<ResolverChange>,
    /// Present when `assess` ran over records.
    pub records: Option<RecordAssessment>,
}

impl ImpactReport {
    /// The one-line verdict: the worst class any column hits.
    pub fn worst(&self) -> ChangeClass {
        self.columns
            .values()
            .map(|c| c.class)
            .max()
            .unwrap_or(ChangeClass::Safe)
    }
}

/// Static classification of the transition `from → to` (§3).
pub fn classify(
    from: &Schema,
    to: &Schema,
    nomenclatures: &NomenclatureTable,
) -> Result<ImpactReport, CastError> {
    let from_index = SchemaIndex::build(from);
    let to_index = SchemaIndex::build(to);
    let mut columns = BTreeMap::new();

    for (id, finfo) in &from_index.columns {
        let impact = match to_index.columns.get(id) {
            None => ColumnImpact {
                change: ColumnChange::Removed,
                class: ChangeClass::Safe,
                removed_options: vec![],
                unit_change: None,
            },
            Some(tinfo) if finfo.scope != tinfo.scope => ColumnImpact {
                change: ColumnChange::ScopeMoved,
                class: ChangeClass::Breaking,
                removed_options: vec![],
                unit_change: None,
            },
            Some(tinfo) => {
                let cast = column_cast(
                    (&finfo.ty, finfo.arity),
                    (&tinfo.ty, tinfo.arity),
                    nomenclatures,
                )?;
                let removed_options =
                    removed_options(&finfo.ty, &tinfo.ty, nomenclatures)?;
                let unit_change = unit_change(&finfo.ty, &tinfo.ty);
                match cast.class() {
                    CastClass::Identity => ColumnImpact {
                        change: ColumnChange::Identical,
                        class: ChangeClass::Safe,
                        removed_options,
                        unit_change,
                    },
                    CastClass::Forbidden => ColumnImpact {
                        change: ColumnChange::Forbidden,
                        class: ChangeClass::Breaking,
                        removed_options,
                        unit_change,
                    },
                    CastClass::Widening => ColumnImpact {
                        change: ColumnChange::Retyped { cast },
                        class: ChangeClass::Safe,
                        removed_options,
                        unit_change,
                    },
                    CastClass::Lossy => ColumnImpact {
                        change: ColumnChange::Retyped { cast },
                        class: ChangeClass::Lossy,
                        removed_options,
                        unit_change,
                    },
                    CastClass::Checked => ColumnImpact {
                        change: ColumnChange::Retyped { cast },
                        class: ChangeClass::Checked,
                        removed_options,
                        unit_change,
                    },
                }
            }
        };
        columns.insert(id.clone(), impact);
    }
    for id in to_index.columns.keys() {
        if !from_index.columns.contains_key(id) {
            columns.insert(
                id.clone(),
                ColumnImpact {
                    change: ColumnChange::Added,
                    class: ChangeClass::Safe,
                    removed_options: vec![],
                    unit_change: None,
                },
            );
        }
    }

    Ok(ImpactReport {
        columns,
        resolvers: resolver_changes(from, to, &to_index),
        records: None,
    })
}

/// Full impact: static classification plus the projection run over a
/// record set — turning every `Checked` into an exact count (§7:
/// "count of records whose cells fail the new cast").
pub fn assess<'a>(
    from: &Schema,
    to: &Schema,
    nomenclatures: &NomenclatureTable,
    records: impl IntoIterator<Item = &'a RecordValues>,
) -> Result<ImpactReport, CastError> {
    let mut report = classify(from, to, nomenclatures)?;
    let mut assessment = RecordAssessment::default();
    for values in records {
        let projection = project(values, from, to, nomenclatures)?;
        assessment.records += 1;
        let failed = projection.report.total_failed();
        let lossy = projection.report.total_lossy();
        if failed > 0 {
            assessment.records_with_failures += 1;
        }
        if lossy > 0 {
            assessment.records_with_loss += 1;
        }
        assessment.cells_failed += failed;
        assessment.cells_lossy += lossy;
        for (column, col) in &projection.report.columns {
            if col.cells_failed > 0 {
                *assessment.failed_by_column.entry(column.clone()).or_default() +=
                    col.cells_failed;
            }
        }
    }
    report.records = Some(assessment);
    Ok(report)
}

/// §2.14: name the unit transition whenever both sides are numbers and
/// the units differ — including add/remove, where the cast is free but
/// the meaning changed.
fn unit_change(from: &ScalarType, to: &ScalarType) -> Option<UnitChange> {
    let unit_of = |ty: &ScalarType| match ty {
        ScalarType::Integer(u) | ScalarType::Decimal(u) => Some(*u),
        _ => None,
    };
    let (from, to) = (unit_of(from)?, unit_of(to)?);
    (from != to).then_some(UnitChange { from, to })
}

fn removed_options(
    from: &ScalarType,
    to: &ScalarType,
    nomenclatures: &NomenclatureTable,
) -> Result<Vec<OptionId>, CastError> {
    let (ScalarType::Enum(f), ScalarType::Enum(t)) = (from, to) else {
        return Ok(vec![]);
    };
    let from_ids: BTreeSet<_> = nomenclature_rows(f, nomenclatures)?
        .iter()
        .map(|r| r.id.clone())
        .collect();
    let to_ids: BTreeSet<_> = nomenclature_rows(t, nomenclatures)?
        .iter()
        .map(|r| r.id.clone())
        .collect();
    Ok(from_ids.difference(&to_ids).cloned().collect())
}

fn resolver_changes(
    from: &Schema,
    to: &Schema,
    to_index: &SchemaIndex,
) -> Vec<ResolverChange> {
    let by_id = |s: &Schema| -> BTreeMap<ResolverId, ResolverDeclaration> {
        s.resolvers
            .iter()
            .map(|r| (r.id.clone(), r.clone()))
            .collect()
    };
    let from_resolvers = by_id(from);
    let to_resolvers = by_id(to);
    let mut changes = Vec::new();

    for (id, fdecl) in &from_resolvers {
        match to_resolvers.get(id) {
            None => {
                // §2.8: which columns are orphaned — mapped targets that
                // still exist in the new revision, now fed by nothing.
                let orphaned = fdecl
                    .mapping
                    .iter()
                    .map(|m| m.target.clone())
                    .filter(|c| to_index.columns.contains_key(c))
                    .collect();
                changes.push(ResolverChange::Removed {
                    id: id.clone(),
                    orphaned_columns: orphaned,
                });
            }
            Some(tdecl) => {
                if tdecl.mapping != fdecl.mapping
                    || tdecl.result_type != fdecl.result_type
                {
                    // §2.8: stale cells, re-derivable from retained
                    // snapshots — the genuinely valuable bulk operation.
                    changes.push(ResolverChange::MappingChanged {
                        id: id.clone(),
                        stale_columns: tdecl
                            .mapping
                            .iter()
                            .map(|m| m.target.clone())
                            .collect(),
                    });
                } else if tdecl.version != fdecl.version {
                    changes.push(ResolverChange::VersionChanged {
                        id: id.clone(),
                        from: fdecl.version,
                        to: tdecl.version,
                    });
                }
            }
        }
    }
    for id in to_resolvers.keys() {
        if !from_resolvers.contains_key(id) {
            changes.push(ResolverChange::Added { id: id.clone() });
        }
    }
    changes
}
