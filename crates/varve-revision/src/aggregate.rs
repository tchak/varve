//! Aggregate revision construction (§5.5): the one revision a
//! mixed-revision table view or export projects through — computed over
//! the **entire revision history**, never the result set. The aggregate
//! type per column is the least upper bound in the widening order; the
//! report keeps every policy hit loud, or aggregation is a quiet
//! data-corruption machine with a nice UI.

use std::collections::BTreeMap;

use varve_core::{ColumnId, GroupId, RevisionId};
use varve_schema::{
    Arity, CastError, Element, JoinPath, NomenclatureTable, ScalarType, Schema, SchemaIndex,
    column_join,
};

/// Synthetic and non-publishable **by type** (§5.5 guard): an
/// `AggregateRevision` is not a `RevisionId`-bearing revision, so no
/// record can ever be created on it.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateRevision {
    /// Latest revision's column order first, then deprecated columns
    /// appended (§5.5 — its surface is auto-derived the same way).
    pub columns: Vec<AggregateColumn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AggregateColumn {
    pub column: ColumnId,
    /// Latest occurrence's label.
    pub label: String,
    pub ty: ScalarType,
    pub arity: Arity,
    pub scope: Vec<GroupId>,
    /// §5.5 guard: absent from the latest revision — table views grey
    /// it, CSV headers flag it. The value is the first revision where
    /// the column is gone.
    pub deprecated_since: Option<RevisionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregatePolicy {
    /// The LUB landed on `Text` from two non-text types (§5.5): valid,
    /// but stringifies typed data — must be visible in the report.
    ViaText,
    /// No join exists (attachment/geometry mixes): omitted entirely.
    Omitted,
    /// §5.5 "scope moved — must split": v1 keeps the latest scope's
    /// occurrences and reports; older-scope cells are unreachable
    /// through the aggregate. (A true split into per-range columns is
    /// the recorded follow-up.)
    ScopeKeptLatest,
    /// §5.5 "removed then re-added with a different type, same id —
    /// split by revision range": v1 joins across the gap and reports;
    /// the two lives of the id share one header. (The split is the
    /// recorded follow-up, alongside `ScopeKeptLatest`.)
    ReAddedRetyped,
}

/// Same shape as the impact report (§5.5): which columns hit which
/// policy.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AggregateReport {
    pub entries: Vec<(ColumnId, AggregatePolicy)>,
}

/// Build the aggregate over an oldest-first revision history (the whole
/// DAG lineage — §5.5: stable, cacheable, computed once per
/// publication).
pub fn aggregate(
    history: &[(RevisionId, &Schema)],
    nomenclatures: &NomenclatureTable,
) -> Result<(AggregateRevision, AggregateReport), CastError> {
    struct Occurrence {
        revision: usize,
        label: String,
        ty: ScalarType,
        arity: Arity,
        scope: Vec<GroupId>,
    }
    let mut occurrences: BTreeMap<ColumnId, Vec<Occurrence>> = BTreeMap::new();
    for (index, (_, schema)) in history.iter().enumerate() {
        let schema_index = SchemaIndex::build(schema);
        let labels = column_labels(schema);
        for (column, info) in schema_index.columns {
            occurrences
                .entry(column.clone())
                .or_default()
                .push(Occurrence {
                    revision: index,
                    label: labels.get(&column).cloned().unwrap_or_default(),
                    ty: info.ty,
                    arity: info.arity,
                    scope: info.scope,
                });
        }
    }

    let mut report = AggregateReport::default();
    let mut aggregated: BTreeMap<ColumnId, AggregateColumn> = BTreeMap::new();

    for (column, occs) in &occurrences {
        let latest = occs.last().expect("non-empty");
        if occs.iter().any(|o| o.scope != latest.scope) {
            report
                .entries
                .push((column.clone(), AggregatePolicy::ScopeKeptLatest));
        }
        let in_scope: Vec<&Occurrence> = occs.iter().filter(|o| o.scope == latest.scope).collect();
        // Removed then re-added with another type: a gap in the
        // revision indices with a type change across it.
        let re_added_retyped = occs.windows(2).any(|w| {
            w[1].revision > w[0].revision + 1 && (w[1].ty != w[0].ty || w[1].arity != w[0].arity)
        });
        if re_added_retyped {
            report
                .entries
                .push((column.clone(), AggregatePolicy::ReAddedRetyped));
        }

        let mut ty = in_scope[0].ty.clone();
        let mut arity = in_scope[0].arity;
        let mut via_text = false;
        let mut omitted = false;
        for occ in &in_scope[1..] {
            match column_join((&ty, arity), (&occ.ty, occ.arity), nomenclatures) {
                Ok(((joined, joined_arity), path)) => {
                    if path == JoinPath::ViaText {
                        via_text = true;
                    }
                    ty = joined;
                    arity = joined_arity;
                }
                Err(varve_schema::JoinConflict::Incompatible) => {
                    omitted = true;
                    break;
                }
                Err(varve_schema::JoinConflict::UnknownNomenclature(id, version)) => {
                    return Err(CastError::UnknownNomenclature(id, version));
                }
            }
        }
        if omitted {
            report
                .entries
                .push((column.clone(), AggregatePolicy::Omitted));
            continue;
        }
        if via_text {
            report
                .entries
                .push((column.clone(), AggregatePolicy::ViaText));
        }

        let last_present = latest.revision;
        let deprecated_since = if last_present + 1 < history.len() {
            Some(history[last_present + 1].0.clone())
        } else {
            None
        };
        aggregated.insert(
            column.clone(),
            AggregateColumn {
                column: column.clone(),
                label: latest.label.clone(),
                ty,
                arity,
                scope: latest.scope.clone(),
                deprecated_since,
            },
        );
    }

    // Order: latest revision's column order, then deprecated columns in
    // first-appearance order.
    let mut columns = Vec::new();
    if let Some((_, latest_schema)) = history.last() {
        for column in column_order(latest_schema) {
            if let Some(aggregate) = aggregated.remove(&column) {
                columns.push(aggregate);
            }
        }
    }
    let mut deprecated: Vec<AggregateColumn> = aggregated.into_values().collect();
    deprecated.sort_by_key(|c| {
        occurrences[&c.column]
            .first()
            .map(|o| o.revision)
            .unwrap_or(0)
    });
    columns.extend(deprecated);

    Ok((AggregateRevision { columns }, report))
}

fn column_labels(schema: &Schema) -> BTreeMap<ColumnId, String> {
    fn walk(elements: &[Element], out: &mut BTreeMap<ColumnId, String>) {
        for el in elements {
            match el {
                Element::Column(c) => {
                    out.insert(c.id.clone(), c.label.clone());
                }
                Element::Group(g) => walk(&g.children, out),
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&schema.root, &mut out);
    out
}

fn column_order(schema: &Schema) -> Vec<ColumnId> {
    fn walk(elements: &[Element], out: &mut Vec<ColumnId>) {
        for el in elements {
            match el {
                Element::Column(c) => out.push(c.id.clone()),
                Element::Group(g) => walk(&g.children, out),
            }
        }
    }
    let mut out = Vec::new();
    walk(&schema.root, &mut out);
    out
}
