//! §3.1: surface changes classify by their **admissibility delta**.
//! Presentation-only changes — a section retitled, prompts, help,
//! notes, write policy — are free, *reported* (the report is the
//! history's diff, and free is not invisible). Admissibility loosened
//! is free; admissibility tightened is `Checked`: a record never
//! becomes globally invalid (§2.6), but an in-flight record admissible
//! yesterday can lapse today, and grading `Checked` exactly is the
//! record assessment's job, as for casts. Where a rule *changed* and
//! the direction is statically undecidable without the §4.3 solver
//! (Q15), the classification is conservatively `Checked`.
//!
//! The diff lives here and not in `varve-impact` by the §7 tier
//! argument (Q24): Tier 2 cannot name surface types, so this crate
//! reuses `impact`'s [`ChangeClass`] vocabulary (Tier 3 → Tier 2,
//! legal) and whoever holds both reports — `varve-service`, the
//! platform — composes them, gating on the worst class of the pair.

use std::collections::BTreeMap;

use varve_core::{ColumnId, NodeId, SurfaceId};
use varve_impact::ChangeClass;
use varve_logic::Expr;

use crate::{ColumnNode, GroupNode, Node, Note, Section, Surface, column_entries};

/// The §3.1 report over one publication's surface transition —
/// [`diff_sets`]' answer, the surface half of the impact story.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SurfaceReport {
    /// Every change, in (surface id, discovery) order.
    pub changes: Vec<SurfaceChange>,
}

impl SurfaceReport {
    /// The one-line verdict, composable with
    /// `varve_impact::ImpactReport::worst` by taking the max.
    pub fn worst(&self) -> ChangeClass {
        self.changes
            .iter()
            .map(|c| c.class)
            .max()
            .unwrap_or(ChangeClass::Safe)
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// One classified surface change.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceChange {
    pub surface: SurfaceId,
    pub class: ChangeClass,
    pub kind: SurfaceChangeKind,
}

/// What changed. Presentation kinds carry the names the report line
/// needs; admissibility kinds carry the column, named by the schema
/// label above this crate.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceChangeKind {
    /// A surface id new to the set: a new admissibility context, not
    /// a lapse of an existing one — safe, reported as one entry (its
    /// nodes are not enumerated).
    SurfaceAdded,
    /// A surface id gone from the set: every constraint it carried is
    /// gone with it — loosening, safe.
    SurfaceRemoved,
    SectionAdded {
        id: NodeId,
        title: String,
    },
    SectionRemoved {
        id: NodeId,
        title: String,
    },
    /// The empty-diff bug that opened Q23: a section rename.
    SectionRetitled {
        id: NodeId,
        from: String,
        to: String,
    },
    SectionHelpChanged {
        id: NodeId,
        title: String,
    },
    NoteAdded {
        id: NodeId,
    },
    NoteRemoved {
        id: NodeId,
    },
    NoteChanged {
        id: NodeId,
    },
    /// A column node new on this surface. Tightening iff it arrives
    /// with a required rule.
    ColumnPresented {
        column: ColumnId,
    },
    /// A column node gone from this surface: its requiredness and
    /// format went with it — loosening.
    ColumnWithdrawn {
        column: ColumnId,
    },
    PromptChanged {
        column: ColumnId,
    },
    HelpChanged {
        column: ColumnId,
    },
    GroupPromptChanged {
        group: varve_core::GroupId,
    },
    /// `None` → `Some`: the column can now be demanded.
    RequirednessTightened {
        column: ColumnId,
    },
    /// `Some` → `None`: never required any more.
    RequirednessLoosened {
        column: ColumnId,
    },
    /// Rule → different rule: direction undecidable, `Checked`.
    RequirednessChanged {
        column: ColumnId,
    },
    /// The column's *effective* visibility (ancestors ∧ own, §2.6)
    /// changed: `Checked` when a required rule rides on the column —
    /// the admissible set may tighten — presentation otherwise.
    VisibilityChanged {
        column: ColumnId,
    },
    /// `None` → `Some`: values that used to pass may not.
    FormatTightened {
        column: ColumnId,
    },
    /// `Some` → `None`.
    FormatLoosened {
        column: ColumnId,
    },
    /// Format → different format: direction undecidable, `Checked`.
    FormatChanged {
        column: ColumnId,
    },
    /// Writability or derived-override changed: constrains future
    /// writes, never stored data — free, reported.
    WritePolicyChanged {
        column: ColumnId,
    },
    /// §4.1 record-scoped admissibility gained a rule.
    IneligibilityAdded,
    IneligibilityRemoved,
    IneligibilityRuleChanged,
    /// Only the message: presentation.
    IneligibilityMessageChanged,
}

/// Diff two surface *sets* — the §2.13 decision 9 publication unit —
/// matched by surface id, each pair diffed with [`diff`].
pub fn diff_sets(from: &[Surface], to: &[Surface]) -> SurfaceReport {
    let from_by_id: BTreeMap<&SurfaceId, &Surface> = from.iter().map(|s| (&s.id, s)).collect();
    let to_by_id: BTreeMap<&SurfaceId, &Surface> = to.iter().map(|s| (&s.id, s)).collect();
    let mut changes = Vec::new();
    for (id, before) in &from_by_id {
        match to_by_id.get(id) {
            Some(after) => changes.extend(diff(before, after).changes),
            None => changes.push(SurfaceChange {
                surface: (*id).clone(),
                class: ChangeClass::Safe,
                kind: SurfaceChangeKind::SurfaceRemoved,
            }),
        }
    }
    for id in to_by_id.keys() {
        if !from_by_id.contains_key(id) {
            changes.push(SurfaceChange {
                surface: (*id).clone(),
                class: ChangeClass::Safe,
                kind: SurfaceChangeKind::SurfaceAdded,
            });
        }
    }
    SurfaceReport { changes }
}

/// Diff one surface against its successor (same id expected; the
/// caller matches). Column changes compare the *flattened* view —
/// node prompt/help/required/format/write plus effective visibility —
/// so a rule moved from a section onto the column itself reads as the
/// visibility change it is (or as none, when the conjunction is
/// unchanged).
pub fn diff(from: &Surface, to: &Surface) -> SurfaceReport {
    let mut changes = Vec::new();
    let surface = to.id.clone();
    let push = |changes: &mut Vec<SurfaceChange>, class, kind| {
        changes.push(SurfaceChange {
            surface: surface.clone(),
            class,
            kind,
        });
    };
    use ChangeClass::{Checked, Safe};
    use SurfaceChangeKind as K;

    // ---- presentation nodes: sections, notes, group prompts -------
    let (from_sections, from_notes, from_groups) = presentation(from);
    let (to_sections, to_notes, to_groups) = presentation(to);
    for (id, b) in &from_sections {
        match to_sections.get(id) {
            None => push(
                &mut changes,
                Safe,
                K::SectionRemoved {
                    id: (*id).clone(),
                    title: b.title.clone(),
                },
            ),
            Some(a) => {
                if a.title != b.title {
                    push(
                        &mut changes,
                        Safe,
                        K::SectionRetitled {
                            id: (*id).clone(),
                            from: b.title.clone(),
                            to: a.title.clone(),
                        },
                    );
                }
                if a.help != b.help {
                    push(
                        &mut changes,
                        Safe,
                        K::SectionHelpChanged {
                            id: (*id).clone(),
                            title: a.title.clone(),
                        },
                    );
                }
            }
        }
    }
    for (id, a) in &to_sections {
        if !from_sections.contains_key(id) {
            push(
                &mut changes,
                Safe,
                K::SectionAdded {
                    id: (*id).clone(),
                    title: a.title.clone(),
                },
            );
        }
    }
    for (id, b) in &from_notes {
        match to_notes.get(id) {
            None => push(&mut changes, Safe, K::NoteRemoved { id: (*id).clone() }),
            Some(a) if (a.title != b.title) || (a.body != b.body) => {
                push(&mut changes, Safe, K::NoteChanged { id: (*id).clone() })
            }
            Some(_) => {}
        }
    }
    for id in to_notes.keys() {
        if !from_notes.contains_key(id) {
            push(&mut changes, Safe, K::NoteAdded { id: (*id).clone() });
        }
    }
    for (id, b) in &from_groups {
        if let Some(a) = to_groups.get(id)
            && a.prompt != b.prompt
        {
            push(
                &mut changes,
                Safe,
                K::GroupPromptChanged {
                    group: (*id).clone(),
                },
            );
        }
    }

    // ---- columns: the flattened admissibility view ----------------
    let from_columns = flat_columns(from);
    let to_columns = flat_columns(to);
    for (column, b) in &from_columns {
        let Some(a) = to_columns.get(column) else {
            push(
                &mut changes,
                Safe,
                K::ColumnWithdrawn {
                    column: (*column).clone(),
                },
            );
            continue;
        };
        let column = || (*column).clone();
        if a.node.prompt != b.node.prompt {
            push(&mut changes, Safe, K::PromptChanged { column: column() });
        }
        if a.node.help != b.node.help {
            push(&mut changes, Safe, K::HelpChanged { column: column() });
        }
        match (&b.node.required, &a.node.required) {
            (None, Some(_)) => push(
                &mut changes,
                Checked,
                K::RequirednessTightened { column: column() },
            ),
            (Some(_), None) => push(
                &mut changes,
                Safe,
                K::RequirednessLoosened { column: column() },
            ),
            (Some(x), Some(y)) if x != y => push(
                &mut changes,
                Checked,
                K::RequirednessChanged { column: column() },
            ),
            _ => {}
        }
        if a.visibility != b.visibility {
            // §3.1: classified through the admissible set — a
            // required column whose reachability changed can tighten.
            let class = if a.node.required.is_some() || b.node.required.is_some() {
                Checked
            } else {
                Safe
            };
            push(
                &mut changes,
                class,
                K::VisibilityChanged { column: column() },
            );
        }
        match (&b.node.format, &a.node.format) {
            (None, Some(_)) => push(
                &mut changes,
                Checked,
                K::FormatTightened { column: column() },
            ),
            (Some(_), None) => push(&mut changes, Safe, K::FormatLoosened { column: column() }),
            (Some(x), Some(y)) if x != y => {
                push(&mut changes, Checked, K::FormatChanged { column: column() })
            }
            _ => {}
        }
        if a.node.write != b.node.write {
            push(
                &mut changes,
                Safe,
                K::WritePolicyChanged { column: column() },
            );
        }
    }
    for (column, a) in &to_columns {
        if !from_columns.contains_key(column) {
            let class = if a.node.required.is_some() {
                Checked
            } else {
                Safe
            };
            push(
                &mut changes,
                class,
                K::ColumnPresented {
                    column: (*column).clone(),
                },
            );
        }
    }

    // ---- ineligibility (§4.1) -------------------------------------
    match (&from.ineligibility, &to.ineligibility) {
        (None, Some(_)) => push(&mut changes, Checked, K::IneligibilityAdded),
        (Some(_), None) => push(&mut changes, Safe, K::IneligibilityRemoved),
        (Some(b), Some(a)) => {
            if a.rule != b.rule {
                push(&mut changes, Checked, K::IneligibilityRuleChanged);
            } else if a.message != b.message {
                push(&mut changes, Safe, K::IneligibilityMessageChanged);
            }
        }
        (None, None) => {}
    }

    SurfaceReport { changes }
}

/// One column's admissibility-relevant view: the node plus its
/// effective visibility (ancestors ∧ own).
struct FlatColumn<'a> {
    node: &'a ColumnNode,
    visibility: Option<Expr>,
}

fn flat_columns(surface: &Surface) -> BTreeMap<&ColumnId, FlatColumn<'_>> {
    column_entries(surface)
        .into_iter()
        .map(|entry| {
            (
                &entry.node.column,
                FlatColumn {
                    visibility: entry.effective_visibility(),
                    node: entry.node,
                },
            )
        })
        .collect()
}

type Presentation<'a> = (
    BTreeMap<&'a NodeId, &'a Section>,
    BTreeMap<&'a NodeId, &'a Note>,
    BTreeMap<&'a varve_core::GroupId, &'a GroupNode>,
);

fn presentation(surface: &Surface) -> Presentation<'_> {
    fn walk<'a>(
        nodes: &'a [Node],
        sections: &mut BTreeMap<&'a NodeId, &'a Section>,
        notes: &mut BTreeMap<&'a NodeId, &'a Note>,
        groups: &mut BTreeMap<&'a varve_core::GroupId, &'a GroupNode>,
    ) {
        for node in nodes {
            match node {
                Node::Column(_) => {}
                Node::Group(g) => {
                    groups.insert(&g.group, g);
                    walk(&g.children, sections, notes, groups);
                }
                Node::Section(s) => {
                    sections.insert(&s.id, s);
                    walk(&s.children, sections, notes, groups);
                }
                Node::Note(n) => {
                    notes.insert(&n.id, n);
                }
            }
        }
    }
    let mut sections = BTreeMap::new();
    let mut notes = BTreeMap::new();
    let mut groups = BTreeMap::new();
    walk(&surface.nodes, &mut sections, &mut notes, &mut groups);
    (sections, notes, groups)
}
