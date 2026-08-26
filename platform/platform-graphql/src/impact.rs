//! `ImpactReport` (G.10, G.11.7): the kernel report *pair*'s GraphQL
//! shape, minimal and honest — the composed verdict, what changed per
//! column (`IDENTICAL` entries filtered: the report says what
//! changed), and the §3.1 surface section. The kernel report's unit
//! and constraint detail, blocks, broken rules and record assessments
//! join as the platform grows them.

use async_graphql::{ID, SimpleObject};

/// What a publication would do to the records reading through it —
/// both halves of the §3.1 story: the schema classification and the
/// surface (admissibility + presentation) diff.
#[derive(SimpleObject)]
pub struct ImpactReport {
    /// The one-line verdict: the worst class either half hits — the
    /// exact class `publishRevision` gates on.
    pub worst: ChangeClass,
    /// Per-column changes, unchanged columns omitted.
    pub columns: Vec<ColumnImpactEntry>,
    /// Groups whose labels changed (§3.1): safe, reported.
    pub relabeled_groups: Vec<GroupRelabelEntry>,
    /// The §3.1 surface diff: admissibility and presentation changes,
    /// classified by their admissibility delta.
    pub surfaces: Vec<SurfaceChangeEntry>,
}

/// One §3.1 surface change, named server-side like the column
/// entries: `label` carries the schema label of the column or group
/// the change touches, or a section's title — whichever the change
/// names; `from`/`to` carry both sides of a retitle.
#[derive(SimpleObject, Debug, Clone, PartialEq, Eq)]
pub struct SurfaceChangeEntry {
    /// Which surface: `applicant` or `reviewer` (P.4's fixed pair).
    pub surface: ID,
    pub class: ChangeClass,
    pub change: SurfaceChangeKind,
    pub label: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// §3.1's vocabulary: presentation kinds are safe; tightening kinds
/// are checked; loosening kinds are safe — the entry's `class` is
/// authoritative (visibility, e.g., classifies by whether a required
/// rule rides on the column).
#[derive(async_graphql::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceChangeKind {
    SurfaceAdded,
    SurfaceRemoved,
    SectionAdded,
    SectionRemoved,
    SectionRetitled,
    SectionHelpChanged,
    NoteAdded,
    NoteRemoved,
    NoteChanged,
    ColumnPresented,
    ColumnWithdrawn,
    PromptChanged,
    HelpChanged,
    GroupPromptChanged,
    RequirednessTightened,
    RequirednessLoosened,
    RequirednessChanged,
    VisibilityChanged,
    FormatTightened,
    FormatLoosened,
    FormatChanged,
    WritePolicyChanged,
    IneligibilityAdded,
    IneligibilityRemoved,
    IneligibilityRuleChanged,
    IneligibilityMessageChanged,
}

/// A renamed group (§3.1).
#[derive(async_graphql::SimpleObject, Debug, Clone, PartialEq, Eq)]
pub struct GroupRelabelEntry {
    pub group_id: ID,
    pub from: String,
    pub to: String,
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
    /// For a `RELABELED` change (§3.1): the base schema's label —
    /// `label` above is already the new one.
    pub renamed_from: Option<String>,
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
    /// §3.1: only the label changed — safe, reported (this used to
    /// classify as unchanged, and a rename published as "no
    /// changes").
    Relabeled,
}

impl ImpactReport {
    /// The kernel report pair with its entries named (G.11.5):
    /// `labels` comes from the two schemas of the classification
    /// (`platform_core::ColumnLabels::resolve`); the verdict is the
    /// worst class of the pair — what the gate used (§3.1).
    pub fn labeled(
        report: &varve_impact::ImpactReport,
        surface_report: &varve_surface::SurfaceReport,
        labels: &platform_core::ColumnLabels,
    ) -> Self {
        Self {
            worst: report.worst().max(surface_report.worst()).into(),
            columns: report
                .columns
                .iter()
                .filter_map(|(id, impact)| {
                    let mut renamed_from = None;
                    let change = match &impact.change {
                        varve_impact::ColumnChange::Identical => return None,
                        varve_impact::ColumnChange::Added => ColumnChangeKind::Added,
                        varve_impact::ColumnChange::Removed => ColumnChangeKind::Removed,
                        varve_impact::ColumnChange::Cast { .. } => ColumnChangeKind::Cast,
                        varve_impact::ColumnChange::ScopeMoved => ColumnChangeKind::ScopeMoved,
                        varve_impact::ColumnChange::Forbidden => ColumnChangeKind::Forbidden,
                        varve_impact::ColumnChange::Relabeled { from, .. } => {
                            renamed_from = Some(from.clone());
                            ColumnChangeKind::Relabeled
                        }
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
                        renamed_from,
                    })
                })
                .collect(),
            relabeled_groups: report
                .relabeled_groups
                .iter()
                .map(|g| GroupRelabelEntry {
                    group_id: ID::from(g.group.as_str()),
                    from: g.from.clone(),
                    to: g.to.clone(),
                })
                .collect(),
            surfaces: surface_report
                .changes
                .iter()
                .map(|change| surface_entry(change, labels))
                .collect(),
        }
    }
}

/// One §3.1 entry, named: columns and groups by their schema labels
/// (the raw id only if neither schema names them — total, never a
/// refusal), sections by the titles the kernel diff carries.
fn surface_entry(
    change: &varve_surface::SurfaceChange,
    labels: &platform_core::ColumnLabels,
) -> SurfaceChangeEntry {
    use SurfaceChangeKind as G;
    use varve_surface::SurfaceChangeKind as K;
    let column = |c: &varve_core::ColumnId| Some(labels.get(c).unwrap_or(c.as_str()).to_owned());
    let (kind, label, from, to) = match &change.kind {
        K::SurfaceAdded => (G::SurfaceAdded, None, None, None),
        K::SurfaceRemoved => (G::SurfaceRemoved, None, None, None),
        K::SectionAdded { title, .. } => (G::SectionAdded, Some(title.clone()), None, None),
        K::SectionRemoved { title, .. } => (G::SectionRemoved, Some(title.clone()), None, None),
        K::SectionRetitled { from, to, .. } => (
            G::SectionRetitled,
            None,
            Some(from.clone()),
            Some(to.clone()),
        ),
        K::SectionHelpChanged { title, .. } => {
            (G::SectionHelpChanged, Some(title.clone()), None, None)
        }
        K::NoteAdded { .. } => (G::NoteAdded, None, None, None),
        K::NoteRemoved { .. } => (G::NoteRemoved, None, None, None),
        K::NoteChanged { .. } => (G::NoteChanged, None, None, None),
        K::ColumnPresented { column: c } => (G::ColumnPresented, column(c), None, None),
        K::ColumnWithdrawn { column: c } => (G::ColumnWithdrawn, column(c), None, None),
        K::PromptChanged { column: c } => (G::PromptChanged, column(c), None, None),
        K::HelpChanged { column: c } => (G::HelpChanged, column(c), None, None),
        K::GroupPromptChanged { group } => (
            G::GroupPromptChanged,
            Some(labels.get_group(group).unwrap_or(group.as_str()).to_owned()),
            None,
            None,
        ),
        K::RequirednessTightened { column: c } => (G::RequirednessTightened, column(c), None, None),
        K::RequirednessLoosened { column: c } => (G::RequirednessLoosened, column(c), None, None),
        K::RequirednessChanged { column: c } => (G::RequirednessChanged, column(c), None, None),
        K::VisibilityChanged { column: c } => (G::VisibilityChanged, column(c), None, None),
        K::FormatTightened { column: c } => (G::FormatTightened, column(c), None, None),
        K::FormatLoosened { column: c } => (G::FormatLoosened, column(c), None, None),
        K::FormatChanged { column: c } => (G::FormatChanged, column(c), None, None),
        K::WritePolicyChanged { column: c } => (G::WritePolicyChanged, column(c), None, None),
        K::IneligibilityAdded => (G::IneligibilityAdded, None, None, None),
        K::IneligibilityRemoved => (G::IneligibilityRemoved, None, None, None),
        K::IneligibilityRuleChanged => (G::IneligibilityRuleChanged, None, None, None),
        K::IneligibilityMessageChanged => (G::IneligibilityMessageChanged, None, None, None),
    };
    SurfaceChangeEntry {
        surface: ID::from(change.surface.as_str()),
        class: change.class.into(),
        change: kind,
        label,
        from,
        to,
    }
}
