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

impl From<&varve_impact::ImpactReport> for ImpactReport {
    fn from(report: &varve_impact::ImpactReport) -> Self {
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
