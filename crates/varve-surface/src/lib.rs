//! Tier 3 (§7): the surface — presentation + admissibility tree over a
//! revision (§2.1). **Nothing depends on this crate**: that is the
//! proof that "form isn't core" — which is why surfaces reach the wire
//! as opaque bodies whose codec lives here (`canon`, §5).
//!
//! The schema defines what is *representable*; a surface defines what
//! is *admissible* (§2.6): visibility, requiredness, format
//! constraints, write policy, prompts. A record is never globally
//! invalid — it is non-admissible **with respect to a surface**.

#![forbid(unsafe_code)]

mod admissibility;
mod block;
pub mod canon;
mod format;
mod reach;
mod validate;

pub use admissibility::{AdmissibilityReport, Finding, admissibility};
pub use block::{BlockDefaults, BlockDefaultsError, IncludeError};
pub use canon::{
    SurfaceDecodeError, block_defaults_canonical, block_defaults_from, format_canonical,
    format_from, node_canonical, node_from, surface_canonical, surface_from,
};
pub use format::{CompiledFormat, Format};
pub use reach::{Reachability, reachability};
pub use validate::{SurfaceError, validate};

use std::collections::BTreeSet;

use varve_core::{ColumnId, GroupId, RevisionId, SurfaceId};
use varve_logic::Expr;

/// A surface: an ordered tree of nodes over one revision. A "form" is
/// one kind of surface, alongside review screens, export layouts,
/// print templates (§2.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub id: SurfaceId,
    pub revision: RevisionId,
    pub nodes: Vec<Node>,
    /// §4.1: a record-scoped admissibility predicate on the submission
    /// surface — DN's ineligibilité. When it holds, the record is
    /// non-admissible with the given message.
    pub ineligibility: Option<Ineligibility>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ineligibility {
    pub rule: Expr,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Column(ColumnNode),
    Group(GroupNode),
    /// Header section: presentation, may carry a visibility rule that
    /// hides everything under it.
    Section(Section),
    /// Explication: prose, no data.
    Note(Note),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnNode {
    pub column: ColumnId,
    /// Presentation overrides; the schema label is the default.
    pub prompt: Option<String>,
    pub help: Option<String>,
    /// Visible when the rule holds; no rule = always visible. Ancestor
    /// rules compose by conjunction.
    pub visibility: Option<Expr>,
    /// Required when the rule holds; `None` = never required. "Always
    /// required" is the vacuous `Expr::And([])`. "Required unless
    /// pending" is `And([NotPending { resolver }])` (§2.8 rule 3).
    pub required: Option<Expr>,
    /// §2.9: surfaces absorb writability.
    pub write: WritePolicy,
    /// §2.6: format constraints are surface admissibility over `text`.
    pub format: Option<Format>,
}

/// Per-column write policy (§2.9), generalizing §2.7's "may derived
/// cells be overridden". Declarative: enforcement happens where entries
/// are authored, above this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WritePolicy {
    pub writable: bool,
    /// May a human overwrite resolver-derived cells *on this surface*
    /// (back-office yes, public form no — §2.7).
    pub override_derived: bool,
}

impl Default for WritePolicy {
    fn default() -> Self {
        Self {
            writable: true,
            override_derived: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupNode {
    pub group: GroupId,
    pub prompt: Option<String>,
    pub visibility: Option<Expr>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    pub help: Option<String>,
    pub visibility: Option<Expr>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub title: Option<String>,
    pub body: String,
}

/// A column node with the visibility rules of all its ancestors
/// (groups, sections) — ancestor rules compose by conjunction into the
/// column's effective visibility.
pub(crate) struct ColumnEntry<'a> {
    pub node: &'a ColumnNode,
    pub ancestors: Vec<&'a Expr>,
}

impl<'a> ColumnEntry<'a> {
    /// The effective visibility rule: `None` = unconditionally visible.
    pub fn effective_visibility(&self) -> Option<Expr> {
        let mut rules: Vec<Expr> = self.ancestors.iter().map(|e| (*e).clone()).collect();
        if let Some(own) = &self.node.visibility {
            rules.push(own.clone());
        }
        match rules.len() {
            0 => None,
            1 => Some(rules.pop().expect("one rule")),
            _ => Some(Expr::And(rules)),
        }
    }
}

pub(crate) fn column_entries(surface: &Surface) -> Vec<ColumnEntry<'_>> {
    fn walk<'a>(nodes: &'a [Node], ancestors: &mut Vec<&'a Expr>, out: &mut Vec<ColumnEntry<'a>>) {
        for node in nodes {
            match node {
                Node::Column(c) => out.push(ColumnEntry {
                    node: c,
                    ancestors: ancestors.clone(),
                }),
                Node::Group(g) => {
                    let pushed = g.visibility.as_ref().inspect(|e| ancestors.push(e));
                    walk(&g.children, ancestors, out);
                    if pushed.is_some() {
                        ancestors.pop();
                    }
                }
                Node::Section(s) => {
                    let pushed = s.visibility.as_ref().inspect(|e| ancestors.push(e));
                    walk(&s.children, ancestors, out);
                    if pushed.is_some() {
                        ancestors.pop();
                    }
                }
                Node::Note(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&surface.nodes, &mut Vec::new(), &mut out);
    out
}

impl Surface {
    /// The columns this surface presents at all — the *static* column
    /// set the §2.9 entry-visibility filter (`filter(log, surface)`,
    /// specified, not built — §10 Q14) is defined over: an entry is
    /// visible through S iff it touches a column S presents.
    pub fn columns(&self) -> BTreeSet<ColumnId> {
        fn walk(nodes: &[Node], out: &mut BTreeSet<ColumnId>) {
            for node in nodes {
                match node {
                    Node::Column(c) => {
                        out.insert(c.column.clone());
                    }
                    Node::Group(g) => walk(&g.children, out),
                    Node::Section(s) => walk(&s.children, out),
                    Node::Note(_) => {}
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(&self.nodes, &mut out);
        out
    }

    /// The columns writable through this surface — what a checkpoint
    /// taken through it freezes (§2.8: the freeze is surface-scoped;
    /// `varve-record::Checkpoint::frozen_columns`).
    pub fn writable_columns(&self) -> BTreeSet<ColumnId> {
        column_entries(self)
            .into_iter()
            .filter(|e| e.node.write.writable)
            .map(|e| e.node.column.clone())
            .collect()
    }

    /// The groups whose items can be added, removed or reordered
    /// through this surface: every group node holding at least one
    /// writable column (`Checkpoint::frozen_groups`).
    pub fn writable_groups(&self) -> BTreeSet<GroupId> {
        fn walk(nodes: &[Node], out: &mut BTreeSet<GroupId>) -> bool {
            let mut any = false;
            for node in nodes {
                any |= match node {
                    Node::Column(c) => c.write.writable,
                    Node::Group(g) => {
                        let inner = walk(&g.children, out);
                        if inner {
                            out.insert(g.group.clone());
                        }
                        inner
                    }
                    Node::Section(s) => walk(&s.children, out),
                    Node::Note(_) => false,
                };
            }
            any
        }
        let mut out = BTreeSet::new();
        walk(&self.nodes, &mut out);
        out
    }
}
