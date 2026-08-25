//! §3.1: the surface diff classifies by admissibility delta —
//! presentation free but *reported*, loosening free, tightening
//! `Checked` — with the conservative `Checked` where a rule changed
//! and direction is statically undecidable (Q15).

use varve_core::{ColumnId, NodeId, RevisionId, SurfaceId};
use varve_impact::ChangeClass;
use varve_logic::{Atom, ColumnRef, Expr};
use varve_surface::{
    ColumnNode, Format, GroupNode, Ineligibility, Node, Note, Section, Surface, SurfaceChangeKind,
    WritePolicy, diff, diff_sets,
};

fn is_filled(c: &str) -> Expr {
    Expr::Atom(Atom::IsFilled {
        source: ColumnRef {
            column: ColumnId::new(c),
            field: None,
        },
    })
}

fn column(id: &str) -> ColumnNode {
    ColumnNode {
        column: ColumnId::new(id),
        prompt: None,
        help: None,
        visibility: None,
        required: None,
        write: WritePolicy::default(),
        format: None,
    }
}

fn surface(nodes: Vec<Node>) -> Surface {
    Surface {
        id: SurfaceId::new("applicant"),
        revision: RevisionId::new("rev-1"),
        nodes,
        ineligibility: None,
    }
}

fn section(id: &str, title: &str, children: Vec<Node>) -> Node {
    Node::Section(Section {
        id: NodeId::new(id),
        title: title.into(),
        help: None,
        visibility: None,
        children,
    })
}

/// One-entry helper: the diff of two single-column surfaces after
/// `edit` mutates the second column node.
fn column_diff(edit: impl FnOnce(&mut ColumnNode)) -> varve_surface::SurfaceReport {
    let before = surface(vec![Node::Column(column("a"))]);
    let mut node = column("a");
    edit(&mut node);
    let after = surface(vec![Node::Column(node)]);
    diff(&before, &after)
}

#[test]
fn identical_surfaces_diff_empty() {
    let s = surface(vec![section(
        "s1",
        "Identité",
        vec![Node::Column(column("a"))],
    )]);
    let report = diff(&s, &s.clone());
    assert!(report.is_empty());
    assert_eq!(report.worst(), ChangeClass::Safe);
}

/// The bug that opened Q23: a section rename is safe — and reported.
#[test]
fn a_section_rename_is_safe_and_reported() {
    let before = surface(vec![section("s1", "Identité", vec![])]);
    let after = surface(vec![section("s1", "Votre identité", vec![])]);
    let report = diff(&before, &after);
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert_eq!(report.changes.len(), 1);
    assert_eq!(
        report.changes[0].kind,
        SurfaceChangeKind::SectionRetitled {
            id: NodeId::new("s1"),
            from: "Identité".into(),
            to: "Votre identité".into(),
        }
    );
}

#[test]
fn presentation_changes_are_safe_and_reported() {
    // Prompt, help, write policy: free, one entry each.
    let report = column_diff(|c| {
        c.prompt = Some("Votre nom".into());
        c.help = Some("Comme sur la carte".into());
        c.write = WritePolicy {
            writable: false,
            override_derived: false,
        };
    });
    assert_eq!(report.worst(), ChangeClass::Safe);
    let kinds: Vec<_> = report
        .changes
        .iter()
        .map(|c| std::mem::discriminant(&c.kind))
        .collect();
    assert_eq!(kinds.len(), 3);

    // Sections and notes added/removed; a group prompt change.
    let before = surface(vec![
        section("s1", "Un", vec![]),
        Node::Note(Note {
            id: NodeId::new("n1"),
            title: None,
            body: "lisez-moi".into(),
        }),
        Node::Group(GroupNode {
            group: varve_core::GroupId::new("g1"),
            prompt: None,
            visibility: None,
            children: vec![],
        }),
    ]);
    let after = surface(vec![
        section("s2", "Deux", vec![]),
        Node::Note(Note {
            id: NodeId::new("n1"),
            title: None,
            body: "relisez-moi".into(),
        }),
        Node::Group(GroupNode {
            group: varve_core::GroupId::new("g1"),
            prompt: Some("Vos adresses".into()),
            visibility: None,
            children: vec![],
        }),
    ]);
    let report = diff(&before, &after);
    assert_eq!(report.worst(), ChangeClass::Safe);
    use SurfaceChangeKind as K;
    assert!(report.changes.iter().any(|c| matches!(
        &c.kind,
        K::SectionRemoved { title, .. } if title == "Un"
    )));
    assert!(report.changes.iter().any(|c| matches!(
        &c.kind,
        K::SectionAdded { title, .. } if title == "Deux"
    )));
    assert!(
        report
            .changes
            .iter()
            .any(|c| matches!(&c.kind, K::NoteChanged { .. }))
    );
    assert!(
        report
            .changes
            .iter()
            .any(|c| matches!(&c.kind, K::GroupPromptChanged { .. }))
    );
}

#[test]
fn requiredness_classifies_by_direction() {
    use SurfaceChangeKind as K;
    // None → Some: tightening — an in-flight record can lapse.
    let report = column_diff(|c| c.required = Some(Expr::And(vec![])));
    assert_eq!(report.worst(), ChangeClass::Checked);
    assert!(matches!(
        report.changes[0].kind,
        K::RequirednessTightened { .. }
    ));
    // Some → None: loosening — free.
    let before = {
        let mut node = column("a");
        node.required = Some(Expr::And(vec![]));
        surface(vec![Node::Column(node)])
    };
    let after = surface(vec![Node::Column(column("a"))]);
    let report = diff(&before, &after);
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert!(matches!(
        report.changes[0].kind,
        K::RequirednessLoosened { .. }
    ));
    // Rule → different rule: direction undecidable — Checked.
    let mut b = column("a");
    b.required = Some(is_filled("x"));
    let mut a = column("a");
    a.required = Some(is_filled("y"));
    let report = diff(
        &surface(vec![Node::Column(b)]),
        &surface(vec![Node::Column(a)]),
    );
    assert_eq!(report.worst(), ChangeClass::Checked);
    assert!(matches!(
        report.changes[0].kind,
        K::RequirednessChanged { .. }
    ));
}

#[test]
fn formats_classify_like_accept_sets() {
    use SurfaceChangeKind as K;
    let report = column_diff(|c| c.format = Some(Format::Email));
    assert_eq!(report.worst(), ChangeClass::Checked);
    assert!(matches!(report.changes[0].kind, K::FormatTightened { .. }));

    let mut b = column("a");
    b.format = Some(Format::Email);
    let report = diff(
        &surface(vec![Node::Column(b.clone())]),
        &surface(vec![Node::Column(column("a"))]),
    );
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert!(matches!(report.changes[0].kind, K::FormatLoosened { .. }));

    let mut a = column("a");
    a.format = Some(Format::Iban);
    let report = diff(
        &surface(vec![Node::Column(b)]),
        &surface(vec![Node::Column(a)]),
    );
    assert_eq!(report.worst(), ChangeClass::Checked);
    assert!(matches!(report.changes[0].kind, K::FormatChanged { .. }));
}

/// Effective visibility is the ancestor conjunction (§2.6): a rule
/// moved from the section onto the column itself is *no* change; a
/// genuinely new rule classifies by whether requiredness rides on
/// the column.
#[test]
fn visibility_is_effective_and_classifies_through_requiredness() {
    use SurfaceChangeKind as K;
    // Moved, not changed: section rule → own rule.
    let rule = is_filled("x");
    let before = surface(vec![Node::Section(Section {
        id: NodeId::new("s1"),
        title: "Un".into(),
        help: None,
        visibility: Some(rule.clone()),
        children: vec![Node::Column(column("a"))],
    })]);
    let mut moved = column("a");
    moved.visibility = Some(rule.clone());
    let after = surface(vec![Node::Section(Section {
        id: NodeId::new("s1"),
        title: "Un".into(),
        help: None,
        visibility: None,
        children: vec![Node::Column(moved)],
    })]);
    assert!(diff(&before, &after).is_empty());

    // A new rule over a plain column: presentation, safe.
    let report = column_diff(|c| c.visibility = Some(is_filled("x")));
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert!(matches!(
        report.changes[0].kind,
        K::VisibilityChanged { .. }
    ));

    // A new rule over a *required* column: the admissible set can
    // tighten — Checked.
    let mut b = column("a");
    b.required = Some(Expr::And(vec![]));
    let mut a = b.clone();
    a.visibility = Some(is_filled("x"));
    let report = diff(
        &surface(vec![Node::Column(b)]),
        &surface(vec![Node::Column(a)]),
    );
    assert_eq!(report.worst(), ChangeClass::Checked);
    assert!(matches!(
        report.changes[0].kind,
        K::VisibilityChanged { .. }
    ));
}

#[test]
fn columns_presented_and_withdrawn() {
    use SurfaceChangeKind as K;
    // Withdrawn: loosening, safe.
    let before = surface(vec![Node::Column(column("a")), Node::Column(column("b"))]);
    let after = surface(vec![Node::Column(column("a"))]);
    let report = diff(&before, &after);
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert!(matches!(report.changes[0].kind, K::ColumnWithdrawn { .. }));

    // Presented plain: safe. Presented required: Checked.
    let report = diff(&after, &before);
    assert_eq!(report.worst(), ChangeClass::Safe);
    let mut required = column("b");
    required.required = Some(Expr::And(vec![]));
    let with_required = surface(vec![Node::Column(column("a")), Node::Column(required)]);
    let report = diff(&after, &with_required);
    assert_eq!(report.worst(), ChangeClass::Checked);
    assert!(matches!(report.changes[0].kind, K::ColumnPresented { .. }));
}

#[test]
fn ineligibility_classifies_by_rule_and_message() {
    use SurfaceChangeKind as K;
    let none = surface(vec![]);
    let with = |rule: Expr, message: &str| {
        let mut s = surface(vec![]);
        s.ineligibility = Some(Ineligibility {
            rule,
            message: message.into(),
        });
        s
    };
    let report = diff(&none, &with(is_filled("x"), "non"));
    assert_eq!(report.worst(), ChangeClass::Checked);
    assert!(matches!(report.changes[0].kind, K::IneligibilityAdded));

    let report = diff(&with(is_filled("x"), "non"), &none);
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert!(matches!(report.changes[0].kind, K::IneligibilityRemoved));

    let report = diff(&with(is_filled("x"), "non"), &with(is_filled("y"), "non"));
    assert_eq!(report.worst(), ChangeClass::Checked);
    assert!(matches!(
        report.changes[0].kind,
        K::IneligibilityRuleChanged
    ));

    let report = diff(&with(is_filled("x"), "non"), &with(is_filled("x"), "niet"));
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert!(matches!(
        report.changes[0].kind,
        K::IneligibilityMessageChanged
    ));
}

/// Sets match by surface id: a new id is one safe entry, a gone id
/// one safe entry, a shared id diffs node-by-node.
#[test]
fn set_diff_matches_by_id() {
    use SurfaceChangeKind as K;
    let applicant = surface(vec![section("s1", "Un", vec![])]);
    let mut reviewer = surface(vec![]);
    reviewer.id = SurfaceId::new("reviewer");
    let mut renamed = surface(vec![section("s1", "Deux", vec![])]);
    renamed.id = SurfaceId::new("applicant");

    let report = diff_sets(
        std::slice::from_ref(&applicant),
        &[renamed, reviewer.clone()],
    );
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert!(
        report
            .changes
            .iter()
            .any(|c| matches!(&c.kind, K::SectionRetitled { .. })
                && c.surface == SurfaceId::new("applicant"))
    );
    assert!(
        report
            .changes
            .iter()
            .any(|c| matches!(&c.kind, K::SurfaceAdded) && c.surface == SurfaceId::new("reviewer"))
    );

    let report = diff_sets(&[applicant, reviewer], &[]);
    assert_eq!(report.changes.len(), 2);
    assert!(
        report
            .changes
            .iter()
            .all(|c| matches!(&c.kind, K::SurfaceRemoved))
    );
}
