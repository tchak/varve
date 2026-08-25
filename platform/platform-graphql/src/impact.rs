//! `ImpactReport` (G.10): the kernel report's GraphQL shape, minimal
//! and honest — the verdict and what changed per column, `IDENTICAL`
//! entries filtered (the report says what changed). The kernel
//! report's unit and constraint detail, blocks, broken rules and
//! record assessments join as the platform grows them.

use async_graphql::{ID, SimpleObject};

/// What a publication would do to the records reading through it.
#[derive(SimpleObject)]
pub struct ImpactReport {
    /// The one-line verdict: the worst class any column hits.
    pub worst: ChangeClass,
    /// Per-column changes, unchanged columns omitted.
    pub columns: Vec<ColumnImpactEntry>,
}

/// §3's vocabulary, generated from the kernel enum.
#[derive(async_graphql::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[graphql(remote = "varve_impact::ChangeClass")]
pub enum ChangeClass {
    Safe,
    Lossy,
    Checked,
    Breaking,
}

/// One changed column.
#[derive(SimpleObject)]
pub struct ColumnImpactEntry {
    pub column_id: ID,
    /// The column's label (G.11.5), resolved from the next schema —
    /// the base schema for a `REMOVED` column. The raw id only if
    /// neither names it (total, never a refusal).
    pub label: String,
    pub class: ChangeClass,
    pub change: ColumnChangeKind,
    /// For choice transitions that drop options (§2.11): exactly
    /// which ids.
    pub removed_options: Vec<ID>,
}

/// The change's shape (§3); the cast detail stays kernel-side for
/// now.
#[derive(async_graphql::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnChangeKind {
    Added,
    Removed,
    Cast,
    ScopeMoved,
    Forbidden,
}

impl ImpactReport {
    /// The kernel report with its entries named (G.11.5): `labels`
    /// comes from the two schemas of the classification
    /// (`platform_core::ColumnLabels::resolve`).
    pub fn labeled(
        report: &varve_impact::ImpactReport,
        labels: &platform_core::ColumnLabels,
    ) -> Self {
        Self {
            worst: report.worst().into(),
            columns: report
                .columns
                .iter()
                .filter_map(|(id, impact)| {
                    let change = match &impact.change {
                        varve_impact::ColumnChange::Identical => return None,
                        varve_impact::ColumnChange::Added => ColumnChangeKind::Added,
                        varve_impact::ColumnChange::Removed => ColumnChangeKind::Removed,
                        varve_impact::ColumnChange::Cast { .. } => ColumnChangeKind::Cast,
                        varve_impact::ColumnChange::ScopeMoved => ColumnChangeKind::ScopeMoved,
                        varve_impact::ColumnChange::Forbidden => ColumnChangeKind::Forbidden,
                    };
                    Some(ColumnImpactEntry {
                        column_id: ID::from(id.as_str()),
                        label: labels.get(id).unwrap_or(id.as_str()).to_owned(),
                        class: impact.class.into(),
                        change,
                        removed_options: impact
                            .removed_options
                            .iter()
                            .map(|o| ID::from(o.as_str()))
                            .collect(),
                    })
                })
                .collect(),
        }
    }
}
