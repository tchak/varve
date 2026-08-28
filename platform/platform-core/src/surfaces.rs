//! Surface compilation (P.4 *Publication*): the authored tree
//! deterministically compiles into the **fixed pair** — `reviewer` =
//! the full tree, `applicant` = the tree with reviewer-only subtrees
//! pruned by effective audience — so the compiled surfaces cannot
//! disagree with the authored markers (P.4 *Surfaces on the draft*:
//! two hand-kept trees drift; pruning makes the drift
//! unrepresentable).
//!
//! Write policy, settled from DN semantics and DESIGN §2.7
//! ("back-office yes, public form no"): on the applicant surface
//! every column present is writable and never overrides derived
//! cells; on the reviewer surface only reviewer-only columns are
//! writable (annotations privées are the instructeur's — the
//! dossier's own fields are corrected through `returnToApplicant`
//! and messaging, never edited in place) and those may override.
//! `required: true` compiles to the vacuous always-required rule on
//! every surface the column appears on (DESIGN §2.6's constant
//! case).

use varve_core::{RevisionId, SurfaceId};
use varve_logic::Expr;
use varve_surface::{ColumnNode, GroupNode, Node, Note, Section, Surface, WritePolicy};

use crate::tree::{Audience, Tree, TreeElement};

/// The fixed surface ids (P.4): stable across revisions, so a
/// surface's identity persists the way §2.6 node identity does.
pub const APPLICANT_SURFACE: &str = "applicant";
pub const REVIEWER_SURFACE: &str = "reviewer";

/// The compiled pair.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfacePair {
    pub applicant: Surface,
    pub reviewer: Surface,
}

impl SurfacePair {
    /// The pair as publication stores it.
    pub fn into_vec(self) -> Vec<Surface> {
        vec![self.applicant, self.reviewer]
    }
}

/// Compiles the pair against `revision` — the id the caller computed
/// from the tree's schema (`varve_schema::revision_id`); publication
/// re-validates the pairing.
pub fn compile_surfaces(tree: &Tree, revision: &RevisionId) -> SurfacePair {
    SurfacePair {
        applicant: Surface {
            id: SurfaceId::new(APPLICANT_SURFACE),
            revision: revision.clone(),
            nodes: compile(&tree.elements, Audience::All, Viewer::Applicant),
            ineligibility: None,
        },
        reviewer: Surface {
            id: SurfaceId::new(REVIEWER_SURFACE),
            revision: revision.clone(),
            nodes: compile(&tree.elements, Audience::All, Viewer::Reviewer),
            ineligibility: None,
        },
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Viewer {
    Applicant,
    Reviewer,
}

fn compile(elements: &[TreeElement], inherited: Audience, viewer: Viewer) -> Vec<Node> {
    let mut nodes = Vec::with_capacity(elements.len());
    for element in elements {
        let effective = inherited.narrowest(element.audience());
        // Pruning: an element outside the applicant's audience is
        // absent from the applicant surface — not hidden (P.4: a
        // flagged-hidden column would leak through the redacted log).
        if viewer == Viewer::Applicant && effective == Audience::Reviewer {
            continue;
        }
        nodes.push(match element {
            TreeElement::Column(c) => Node::Column(ColumnNode {
                column: c.id.clone(),
                prompt: None,
                help: None,
                visibility: None,
                required: c.required.then(|| Expr::And(vec![])),
                write: write_policy(viewer, effective),
                format: c.format.clone(),
            }),
            TreeElement::Group(g) => Node::Group(GroupNode {
                group: g.id.clone(),
                prompt: None,
                visibility: None,
                children: compile(&g.children, effective, viewer),
            }),
            TreeElement::Section(s) => Node::Section(Section {
                id: s.id.clone(),
                title: s.title.clone(),
                help: s.help.clone(),
                visibility: None,
                children: compile(&s.children, effective, viewer),
            }),
            TreeElement::Note(n) => Node::Note(Note {
                id: n.id.clone(),
                title: n.title.clone(),
                body: n.body.clone(),
            }),
        });
    }
    nodes
}

fn write_policy(viewer: Viewer, effective: Audience) -> WritePolicy {
    match viewer {
        // Every column present on the applicant surface is theirs to
        // fill; the public form never overrides derived cells (§2.7).
        Viewer::Applicant => WritePolicy {
            writable: true,
            override_derived: false,
        },
        // The reviewer writes only their own columns, with back-office
        // override; the dossier's fields are read-only here.
        Viewer::Reviewer => WritePolicy {
            writable: effective == Audience::Reviewer,
            override_derived: effective == Audience::Reviewer,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{TreeColumn, TreeGroup, TreeNote, TreeSection};
    use varve_core::{ColumnId, GroupId, NodeId};
    use varve_schema::{Arity, Cardinality, NomenclatureTable, ScalarType, revision_id};

    fn column(id: &str, audience: Audience, required: bool) -> TreeElement {
        TreeElement::Column(TreeColumn {
            id: ColumnId::new(id),
            label: id.to_string(),
            ty: ScalarType::Text,
            arity: Arity::One,
            required,
            format: None,
            audience,
        })
    }

    fn tree() -> Tree {
        Tree {
            elements: vec![
                column("name", Audience::All, true),
                TreeElement::Section(TreeSection {
                    id: NodeId::new("s1"),
                    title: "Situation".into(),
                    help: Some("aide".into()),
                    audience: Audience::All,
                    children: vec![
                        column("income", Audience::All, false),
                        column("assessment", Audience::Reviewer, false),
                    ],
                }),
                TreeElement::Group(TreeGroup {
                    id: GroupId::new("g1"),
                    label: "Enfants".into(),
                    cardinality: Cardinality::Many,
                    audience: Audience::All,
                    children: vec![
                        column("age", Audience::All, false),
                        column("note", Audience::Reviewer, false),
                    ],
                }),
                TreeElement::Note(TreeNote {
                    id: NodeId::new("n1"),
                    title: None,
                    body: "Instructions internes".into(),
                    audience: Audience::Reviewer,
                }),
            ],
        }
    }

    fn column_ids(nodes: &[Node]) -> Vec<&str> {
        let mut out = Vec::new();
        fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a str>) {
            for node in nodes {
                match node {
                    Node::Column(c) => out.push(c.column.as_str()),
                    Node::Group(g) => walk(&g.children, out),
                    Node::Section(s) => walk(&s.children, out),
                    Node::Note(_) => {}
                }
            }
        }
        walk(nodes, &mut out);
        out
    }

    fn find_column<'a>(nodes: &'a [Node], id: &str) -> Option<&'a ColumnNode> {
        for node in nodes {
            let found = match node {
                Node::Column(c) if c.column.as_str() == id => Some(c),
                Node::Group(g) => find_column(&g.children, id),
                Node::Section(s) => find_column(&s.children, id),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    #[test]
    fn the_pair_prunes_and_sets_write_policy() {
        let tree = tree();
        let revision = revision_id(&tree.schema());
        let pair = compile_surfaces(&tree, &revision);

        // Applicant: reviewer-only columns and notes are absent, not
        // hidden.
        assert_eq!(column_ids(&pair.applicant.nodes), ["name", "income", "age"]);
        assert!(
            !pair
                .applicant
                .nodes
                .iter()
                .any(|n| matches!(n, Node::Note(_))),
            "the reviewer-only note must be pruned"
        );
        // Reviewer: the full tree, document order preserved.
        assert_eq!(
            column_ids(&pair.reviewer.nodes),
            ["name", "income", "assessment", "age", "note"]
        );

        // Write policies: the applicant fills their form and never
        // overrides; the reviewer writes only their own columns.
        let applicant_name = find_column(&pair.applicant.nodes, "name").unwrap();
        assert_eq!(
            applicant_name.write,
            WritePolicy {
                writable: true,
                override_derived: false
            }
        );
        let reviewer_name = find_column(&pair.reviewer.nodes, "name").unwrap();
        assert_eq!(
            reviewer_name.write,
            WritePolicy {
                writable: false,
                override_derived: false
            }
        );
        let reviewer_note = find_column(&pair.reviewer.nodes, "note").unwrap();
        assert_eq!(
            reviewer_note.write,
            WritePolicy {
                writable: true,
                override_derived: true
            }
        );

        // Required compiles to the vacuous rule where true, no rule
        // where false.
        assert_eq!(applicant_name.required, Some(Expr::And(vec![])));
        assert_eq!(
            find_column(&pair.applicant.nodes, "income")
                .unwrap()
                .required,
            None
        );
    }

    #[test]
    fn inheritance_prunes_whole_subtrees() {
        let tree = Tree {
            elements: vec![TreeElement::Section(TreeSection {
                id: NodeId::new("s"),
                title: "Interne".into(),
                help: None,
                audience: Audience::Reviewer,
                children: vec![
                    // Authored `all` under a reviewer-only section:
                    // the effective audience narrows, so the
                    // applicant loses the whole subtree.
                    column("inner", Audience::All, false),
                ],
            })],
        };
        let revision = revision_id(&tree.schema());
        let pair = compile_surfaces(&tree, &revision);
        assert!(pair.applicant.nodes.is_empty());
        assert_eq!(column_ids(&pair.reviewer.nodes), ["inner"]);
        // And on the reviewer surface the inherited narrowing makes
        // the inner column theirs to write.
        assert!(
            find_column(&pair.reviewer.nodes, "inner")
                .unwrap()
                .write
                .writable
        );
    }

    #[test]
    fn both_compiled_surfaces_always_validate() {
        let tree = tree();
        let schema = tree.schema();
        let revision = revision_id(&schema);
        let pair = compile_surfaces(&tree, &revision);
        let nomenclatures = NomenclatureTable::new();
        for surface in [&pair.applicant, &pair.reviewer] {
            let errors = varve_surface::validate(surface, &schema, &nomenclatures);
            assert!(errors.is_empty(), "{}: {errors:?}", surface.id);
        }
    }
}

/// One admissibility finding tagged with the surface it holds on
/// (`applicant` / `reviewer` — P.4's fixed pair). Shared by the
/// preview (G.12, over compiled-from-draft surfaces) and the case
/// file (G.14, over the head publication's stored surfaces).
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceFinding {
    pub surface: &'static str,
    pub finding: varve_surface::Finding,
}

/// Admissibility of `values` evaluated per named surface, findings
/// tagged. Never a gate (G.12/G.14): the output of a read.
pub fn surface_findings(
    surfaces: &[(&'static str, &varve_surface::Surface)],
    schema: &varve_schema::Schema,
    values: &varve_value::RecordValues,
    pending: &varve_logic::PendingSet,
) -> Result<Vec<SurfaceFinding>, varve_surface::SurfaceError> {
    let nomenclatures = varve_schema::NomenclatureTable::new();
    let mut findings = Vec::new();
    for (name, surface) in surfaces {
        let report =
            varve_surface::admissibility(surface, schema, &nomenclatures, values, pending)?;
        findings.extend(report.findings.into_iter().map(|finding| SurfaceFinding {
            surface: name,
            finding,
        }));
    }
    Ok(findings)
}
