//! Tier 2 (§7): what does publishing revision N+1 do?
//!
//! The impact report is the product artifact no competitor offers
//! (§1): shown to an administration *before* it publishes. This crate
//! covers change classification (§3), the resolver impact questions
//! (§2.8), broken rule references (§4.1 — rules re-typechecked against
//! the new revision), and record assessment (running the projection
//! over real records and counting, including records with pending
//! resolutions against a removed resolver). Not here yet: statically
//! unreachable required columns — that needs the §4.3 solver (§10 Q15).
//!
//! Tier 2 cannot see surfaces or resolution instances, so the caller
//! hands in what it wants judged: the rules (`RuleRef`), and per record
//! the pending resolvers beside its values.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use varve_core::{ColumnId, GroupId, OptionId, ResolverId};
use varve_logic::{Expr, TypeError, typecheck};
use varve_projection::project;
use varve_schema::{
    AttachmentConstraints, BlockRef, Cast, CastClass, CastError, Mapping, NomenclatureTable,
    ResolverDeclaration, ScalarType, Schema, SchemaIndex, Unit, column_cast, included_blocks,
    nomenclature_rows,
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
    /// The column's type, arity, unit, options or constraints changed
    /// — anything with a non-identity cast; the cast tells the story,
    /// `unit_change` / `removed_options` / `constraint_change` name it.
    Cast { cast: Cast },
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
    /// For attachment transitions whose constraints changed (§2.15):
    /// named, so the report can say "accept narrowed to `[pdf]`; limit
    /// lowered" instead of an anonymous retype.
    pub constraint_change: Option<ConstraintChange>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintChange {
    pub from: AttachmentConstraints,
    pub to: AttachmentConstraints,
}

/// The §2.8 impact questions, answered per resolver. One declaration
/// may produce several entries (a version bump *and* a remap).
#[derive(Debug, Clone, PartialEq)]
/// A declaration is identified by `(anchor, id)` (§10 Q17): the same
/// resolver at two groups is two declarations, judged independently.
pub enum ResolverChange {
    Added {
        anchor: GroupId,
        id: ResolverId,
    },
    /// Which columns are orphaned (still exist in the new revision but
    /// nothing feeds them anymore); pending resolutions against this
    /// declaration can never land — `assess` counts the records.
    Removed {
        anchor: GroupId,
        id: ResolverId,
        orphaned_columns: Vec<ColumnId>,
    },
    /// §2.8: "resolver result type changed → which mappings break": the
    /// mappings whose result field vanished or no longer typechecks
    /// against its target column.
    ResultTypeChanged {
        anchor: GroupId,
        id: ResolverId,
        broken_mappings: Vec<Mapping>,
    },
    /// §2.8: "mapping changed → which cells are stale and re-derivable
    /// from retained snapshots": the targets the new mapping feeds
    /// differently or newly (stale), and the targets the old mapping fed
    /// that nothing feeds now (orphaned by the remap).
    MappingChanged {
        anchor: GroupId,
        id: ResolverId,
        stale_columns: Vec<ColumnId>,
        orphaned_columns: Vec<ColumnId>,
    },
    /// The input signature changed: pending resolutions were requested
    /// against the old one (§2.8 rule 1 binds at request time).
    InputChanged {
        anchor: GroupId,
        id: ResolverId,
    },
    VersionChanged {
        anchor: GroupId,
        id: ResolverId,
        from: u32,
        to: u32,
    },
}

/// A rule the caller wants judged against the new revision — Tier 2
/// cannot see where rules live (surfaces, blocks, routing), so the
/// caller names them. `scope` is the rule's attachment scope: empty for
/// record scope, the chain of `many` groups for an item scope (§4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct RuleRef {
    pub name: String,
    pub scope: Vec<GroupId>,
    pub expr: Expr,
}

/// Why a rule breaks — §4.1's taxonomy, mirroring DN's
/// `not_available` / `incompatible` / `not_included`.
#[derive(Debug, Clone, PartialEq)]
pub enum BreakKind {
    /// A source column no longer exists (`not_available`).
    SourceRemoved(ColumnId),
    /// A source was retyped or rescoped so the atom no longer
    /// typechecks (`incompatible`).
    SourceRetyped(ColumnId),
    /// An enum constant names an option id the new nomenclature lacks
    /// (`not_included`; the §2.11/§3 flagged case reaching rules).
    OptionRemoved(ColumnId, OptionId),
    /// A projected nomenclature field disappeared.
    FieldRemoved(ColumnId, String),
    /// `pending(g)` names a group with no resolver anchored to it in
    /// the new revision (§10 Q17).
    ResolverRemoved(GroupId),
    /// Anything else the typechecker refuses (policy, unknown
    /// nomenclature).
    Other(TypeError),
}

/// A rule that fails to typecheck against the new revision.
#[derive(Debug, Clone, PartialEq)]
pub struct BrokenRule {
    pub name: String,
    pub kinds: Vec<BreakKind>,
    /// It did not typecheck against the *old* revision either — the
    /// transition is not what broke it.
    pub already_broken: bool,
}

/// One record as `assess` sees it: its folded values and the
/// declarations with a pending resolution on it — `(anchor, resolver)`
/// pairs, a declaration's identity (§10 Q17); project them from the
/// fold's pending resolutions (`varve_record::FoldResult`).
#[derive(Debug, Clone, Copy)]
pub struct RecordUnderAssessment<'a> {
    pub values: &'a RecordValues,
    pub pending: &'a BTreeSet<(GroupId, ResolverId)>,
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
    /// Records whose cells hit a column with no cast at all (`Forbidden`
    /// or `ScopeMoved`): the projection drops them, so they are not
    /// "failed cells" — they are cells with nowhere to go. Counted per
    /// column so a breaking verdict comes with its blast radius.
    pub records_with_uncastable: u64,
    pub uncastable_by_column: BTreeMap<ColumnId, u64>,
    /// §2.8: records with a pending resolution against a declaration
    /// the new revision removes — those can never land. Keyed by the
    /// declaration's identity, `(anchor, resolver)`.
    pub pending_on_removed_resolvers: BTreeMap<(GroupId, ResolverId), u64>,
}

/// A block-level view of the transition (§2.1, Q5): what the per-column
/// rows say, grouped by the block a group was included from — so a
/// block bump reads as one change with its columns under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockChange {
    /// A group included from a block appears in the new revision.
    Included { group: GroupId, block: BlockRef },
    /// A group included from a block is gone.
    Removed { group: GroupId, block: BlockRef },
    /// The same group is included from another version of its block
    /// (or another block): the block's columns cast per the §3 rows.
    Bumped {
        group: GroupId,
        from: BlockRef,
        to: BlockRef,
    },
    /// The group stays but lost its provenance: edited by hand — no
    /// longer pinned to any block version.
    Detached { group: GroupId, was: BlockRef },
    /// The group stays and gained provenance: adopted into a block.
    Attached { group: GroupId, now: BlockRef },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImpactReport {
    pub columns: BTreeMap<ColumnId, ColumnImpact>,
    pub resolvers: Vec<ResolverChange>,
    /// Block inclusions that changed between the revisions.
    pub blocks: Vec<BlockChange>,
    /// §4.1 "broken rule references": filled by `broken_rules`, or by
    /// `classify_with_rules`/`assess` when rules are supplied.
    pub rules: Vec<BrokenRule>,
    /// Present when `assess` ran over records.
    pub records: Option<RecordAssessment>,
}

impl ImpactReport {
    /// The one-line verdict: the worst class any column hits — and a
    /// rule newly broken by the transition is breaking.
    pub fn worst(&self) -> ChangeClass {
        let columns = self
            .columns
            .values()
            .map(|c| c.class)
            .max()
            .unwrap_or(ChangeClass::Safe);
        if self.rules.iter().any(|r| !r.already_broken) {
            ChangeClass::Breaking
        } else {
            columns
        }
    }
}

/// §4.1: which rules the transition breaks. A rule breaks when a source
/// column is removed, a source is retyped so the atom no longer
/// typechecks, an enum constant references a removed option id, or a
/// projected nomenclature field disappears — exactly the failures the
/// typechecker reports against the new revision, classified.
pub fn broken_rules(
    rules: &[RuleRef],
    from: &Schema,
    to: &Schema,
    nomenclatures: &NomenclatureTable,
) -> Vec<BrokenRule> {
    let from_index = SchemaIndex::build(from);
    let mut out = Vec::new();
    for rule in rules {
        let errors = typecheck(&rule.expr, to, nomenclatures, &rule.scope);
        if errors.is_empty() {
            continue;
        }
        let already_broken = !typecheck(&rule.expr, from, nomenclatures, &rule.scope).is_empty();
        let kinds = errors
            .into_iter()
            .map(|e| match e {
                TypeError::UnknownColumn(c) if from_index.columns.contains_key(&c) => {
                    BreakKind::SourceRemoved(c)
                }
                TypeError::AtomNotAllowed(c)
                | TypeError::TypeMismatch(c)
                | TypeError::UnitMismatch(c)
                | TypeError::ScopeViolation(c) => BreakKind::SourceRetyped(c),
                TypeError::UnknownOption(c, o) => BreakKind::OptionRemoved(c, o),
                TypeError::UnknownField(c, f) => BreakKind::FieldRemoved(c, f),
                TypeError::NoResolverAnchored(g) => BreakKind::ResolverRemoved(g),
                other => BreakKind::Other(other),
            })
            .collect();
        out.push(BrokenRule {
            name: rule.name.clone(),
            kinds,
            already_broken,
        });
    }
    out
}

/// `classify` plus the §4.1 broken-rule section.
pub fn classify_with_rules(
    from: &Schema,
    to: &Schema,
    nomenclatures: &NomenclatureTable,
    rules: &[RuleRef],
) -> Result<ImpactReport, CastError> {
    let mut report = classify(from, to, nomenclatures)?;
    report.rules = broken_rules(rules, from, to, nomenclatures);
    Ok(report)
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
                constraint_change: None,
            },
            Some(tinfo) if finfo.scope != tinfo.scope => ColumnImpact {
                change: ColumnChange::ScopeMoved,
                class: ChangeClass::Breaking,
                removed_options: vec![],
                unit_change: None,
                constraint_change: None,
            },
            Some(tinfo) => {
                let cast = column_cast(
                    (&finfo.ty, finfo.arity),
                    (&tinfo.ty, tinfo.arity),
                    nomenclatures,
                )?;
                let removed_options = removed_options(&finfo.ty, &tinfo.ty, nomenclatures)?;
                let unit_change = unit_change(&finfo.ty, &tinfo.ty);
                let constraint_change = constraint_change(&finfo.ty, &tinfo.ty);
                match cast.class() {
                    CastClass::Identity => ColumnImpact {
                        change: ColumnChange::Identical,
                        class: ChangeClass::Safe,
                        removed_options,
                        unit_change,
                        constraint_change: constraint_change.clone(),
                    },
                    CastClass::Forbidden => ColumnImpact {
                        change: ColumnChange::Forbidden,
                        class: ChangeClass::Breaking,
                        removed_options,
                        unit_change,
                        constraint_change: constraint_change.clone(),
                    },
                    CastClass::Widening => ColumnImpact {
                        change: ColumnChange::Cast { cast },
                        class: ChangeClass::Safe,
                        removed_options,
                        unit_change,
                        constraint_change: constraint_change.clone(),
                    },
                    CastClass::Lossy => ColumnImpact {
                        change: ColumnChange::Cast { cast },
                        class: ChangeClass::Lossy,
                        removed_options,
                        unit_change,
                        constraint_change: constraint_change.clone(),
                    },
                    CastClass::Checked => ColumnImpact {
                        change: ColumnChange::Cast { cast },
                        class: ChangeClass::Checked,
                        removed_options,
                        unit_change,
                        constraint_change: constraint_change.clone(),
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
                    constraint_change: None,
                },
            );
        }
    }

    Ok(ImpactReport {
        columns,
        resolvers: resolver_changes(from, to, &to_index),
        blocks: block_changes(from, to, &from_index, &to_index),
        rules: Vec::new(),
        records: None,
    })
}

/// Full impact: static classification, the §4.1 broken-rule section,
/// and the projection run over a record set — turning every `Checked`
/// into an exact count (§7: "count of records whose cells fail the new
/// cast"), naming the records whose cells have nowhere to go under a
/// breaking column change, and counting records with pending
/// resolutions against a removed resolver (§2.8).
pub fn assess<'a>(
    from: &Schema,
    to: &Schema,
    nomenclatures: &NomenclatureTable,
    rules: &[RuleRef],
    records: impl IntoIterator<Item = RecordUnderAssessment<'a>>,
) -> Result<ImpactReport, CastError> {
    let mut report = classify_with_rules(from, to, nomenclatures, rules)?;
    let removed_resolvers: BTreeSet<(GroupId, ResolverId)> = report
        .resolvers
        .iter()
        .filter_map(|c| match c {
            ResolverChange::Removed { anchor, id, .. } => Some((anchor.clone(), id.clone())),
            _ => None,
        })
        .collect();
    let uncastable: BTreeSet<ColumnId> = report
        .columns
        .iter()
        .filter(|(_, c)| matches!(c.change, ColumnChange::Forbidden | ColumnChange::ScopeMoved))
        .map(|(id, _)| id.clone())
        .collect();
    let mut assessment = RecordAssessment::default();
    for record in records {
        let values = record.values;
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
        // Cells of uncastable columns are dropped by the projection, so
        // count them here: a breaking verdict with a blast radius.
        let mut hit_uncastable = false;
        for addr in values.cells.keys() {
            if uncastable.contains(&addr.column) {
                hit_uncastable = true;
                *assessment
                    .uncastable_by_column
                    .entry(addr.column.clone())
                    .or_default() += 1;
            }
        }
        if hit_uncastable {
            assessment.records_with_uncastable += 1;
        }
        for declaration in record.pending.intersection(&removed_resolvers) {
            *assessment
                .pending_on_removed_resolvers
                .entry(declaration.clone())
                .or_default() += 1;
        }
        for (column, col) in &projection.report.columns {
            if col.cells_failed > 0 {
                *assessment
                    .failed_by_column
                    .entry(column.clone())
                    .or_default() += col.cells_failed;
            }
        }
    }
    report.records = Some(assessment);
    Ok(report)
}

/// §2.15: name the constraint transition whenever both sides are
/// attachments and the constraints differ.
fn constraint_change(from: &ScalarType, to: &ScalarType) -> Option<ConstraintChange> {
    match (from, to) {
        (ScalarType::Attachment(f), ScalarType::Attachment(t)) if f != t => {
            Some(ConstraintChange {
                from: f.clone(),
                to: t.clone(),
            })
        }
        _ => None,
    }
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

fn block_changes(
    from: &Schema,
    to: &Schema,
    from_index: &SchemaIndex,
    to_index: &SchemaIndex,
) -> Vec<BlockChange> {
    let before: BTreeMap<GroupId, BlockRef> = included_blocks(from).into_iter().collect();
    let after: BTreeMap<GroupId, BlockRef> = included_blocks(to).into_iter().collect();
    let mut changes = Vec::new();
    for (group, was) in &before {
        match after.get(group) {
            Some(now) if now == was => {}
            Some(now) => changes.push(BlockChange::Bumped {
                group: group.clone(),
                from: was.clone(),
                to: now.clone(),
            }),
            None if to_index.groups.contains_key(group) => changes.push(BlockChange::Detached {
                group: group.clone(),
                was: was.clone(),
            }),
            None => changes.push(BlockChange::Removed {
                group: group.clone(),
                block: was.clone(),
            }),
        }
    }
    for (group, now) in &after {
        if before.contains_key(group) {
            continue;
        }
        if from_index.groups.contains_key(group) {
            changes.push(BlockChange::Attached {
                group: group.clone(),
                now: now.clone(),
            });
        } else {
            changes.push(BlockChange::Included {
                group: group.clone(),
                block: now.clone(),
            });
        }
    }
    changes
}

fn resolver_changes(from: &Schema, to: &Schema, to_index: &SchemaIndex) -> Vec<ResolverChange> {
    let by_id = |s: &Schema| -> BTreeMap<(GroupId, ResolverId), ResolverDeclaration> {
        s.resolvers
            .iter()
            .map(|r| ((r.anchor.clone(), r.id.clone()), r.clone()))
            .collect()
    };
    let from_resolvers = by_id(from);
    let to_resolvers = by_id(to);
    let mut changes = Vec::new();

    for ((anchor, id), fdecl) in &from_resolvers {
        let key = (anchor.clone(), id.clone());
        match to_resolvers.get(&key) {
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
                    anchor: anchor.clone(),
                    id: id.clone(),
                    orphaned_columns: orphaned,
                });
            }
            Some(tdecl) => {
                // Independent questions, independent answers (§2.8): a
                // declaration can bump its version, retype its result and
                // remap at once — each is reported.
                if tdecl.result_type != fdecl.result_type {
                    let broken_mappings: Vec<Mapping> = tdecl
                        .mapping
                        .iter()
                        .filter(|m| {
                            let field = tdecl.result_type.iter().find(|f| f.name == m.result_field);
                            match (field, to_index.columns.get(&m.target)) {
                                (None, _) => true, // field vanished
                                (Some(f), Some(col)) => !f.ty.same_constructor(&col.ty),
                                (Some(_), None) => false, // target gone: a column question, not a resolver one
                            }
                        })
                        .cloned()
                        .collect();
                    changes.push(ResolverChange::ResultTypeChanged {
                        anchor: anchor.clone(),
                        id: id.clone(),
                        broken_mappings,
                    });
                }
                if tdecl.mapping != fdecl.mapping {
                    // §2.8: stale cells, re-derivable from retained
                    // snapshots — the genuinely valuable bulk operation.
                    let stale_columns: Vec<ColumnId> = tdecl
                        .mapping
                        .iter()
                        .filter(|m| !fdecl.mapping.contains(m))
                        .map(|m| m.target.clone())
                        .collect();
                    let orphaned_columns: Vec<ColumnId> = fdecl
                        .mapping
                        .iter()
                        .map(|m| &m.target)
                        .filter(|t| !tdecl.mapping.iter().any(|m| m.target == **t))
                        .filter(|t| to_index.columns.contains_key(*t))
                        .cloned()
                        .collect();
                    changes.push(ResolverChange::MappingChanged {
                        anchor: anchor.clone(),
                        id: id.clone(),
                        stale_columns,
                        orphaned_columns,
                    });
                }
                if tdecl.input != fdecl.input {
                    changes.push(ResolverChange::InputChanged {
                        anchor: anchor.clone(),
                        id: id.clone(),
                    });
                }
                if tdecl.version != fdecl.version {
                    changes.push(ResolverChange::VersionChanged {
                        anchor: anchor.clone(),
                        id: id.clone(),
                        from: fdecl.version,
                        to: tdecl.version,
                    });
                }
            }
        }
    }
    for (anchor, id) in to_resolvers.keys() {
        if !from_resolvers.contains_key(&(anchor.clone(), id.clone())) {
            changes.push(ResolverChange::Added {
                anchor: anchor.clone(),
                id: id.clone(),
            });
        }
    }
    changes
}
