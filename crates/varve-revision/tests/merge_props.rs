//! Laws of the three-way schema merge (§7: schemas get git semantics)
//! over generated schemas: a side that did not move takes the other
//! side; identical edits agree; conflicts are symmetric and never
//! guessed; a deleted group never silently swallows an edit inside it.

use proptest::prelude::*;
use varve_core::{ColumnId, GroupId, ResolverId};
use varve_revision::{MergeConflict, merge};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, Mapping, ResolverDeclaration, ResultField,
    ScalarType, Schema,
};

fn column(id: &str, label: &str, ty: ScalarType) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: label.to_string(),
        ty,
        arity: Arity::One,
    })
}

fn scalar_type() -> impl Strategy<Value = ScalarType> {
    prop_oneof![
        Just(ScalarType::Text),
        Just(ScalarType::Integer(None)),
        Just(ScalarType::Decimal(None)),
        Just(ScalarType::Boolean),
    ]
}

fn resolver(id: &str, version: u32) -> ResolverDeclaration {
    ResolverDeclaration {
        id: ResolverId::new(id),
        version,
        anchor: GroupId::new("g"),
        input: vec![(ColumnId::new("a"), ScalarType::Text)],
        result_type: vec![ResultField {
            name: "out".into(),
            ty: ScalarType::Text,
        }],
        mapping: vec![Mapping {
            result_field: "out".into(),
            target: ColumnId::new("b"),
        }],
    }
}

/// Columns from a small alphabet, each with a type and a label, in a
/// random order — collisions and disjoint edits both come up.
fn columns(pool: &'static [&'static str]) -> impl Strategy<Value = Vec<Element>> {
    proptest::sample::subsequence(pool.to_vec(), 0..=pool.len())
        .prop_shuffle()
        .prop_flat_map(|ids| {
            let n = ids.len();
            (
                Just(ids),
                proptest::collection::vec(scalar_type(), n),
                proptest::collection::vec(prop_oneof![Just("L1"), Just("L2")], n),
            )
        })
        .prop_map(|(ids, types, labels)| {
            ids.into_iter()
                .zip(types)
                .zip(labels)
                .map(|((id, ty), label)| column(id, label, ty))
                .collect()
        })
}

fn group() -> impl Strategy<Value = Element> {
    (
        prop_oneof![Just(Cardinality::One), Just(Cardinality::Many)],
        prop_oneof![Just("G1"), Just("G2")],
        columns(&["x", "y"]),
    )
        .prop_map(|(cardinality, label, children)| {
            Element::Group(Group {
                included_from: None,
                id: GroupId::new("g"),
                label: label.into(),
                cardinality,
                children,
            })
        })
}

fn resolvers() -> impl Strategy<Value = Vec<ResolverDeclaration>> {
    (any::<bool>(), 1u32..=3, any::<bool>()).prop_map(|(r1, v, r2)| {
        let mut out = Vec::new();
        if r1 {
            out.push(resolver("r1", v));
        }
        if r2 {
            out.push(resolver("r2", 1));
        }
        out
    })
}

/// A schema: root columns from {a, b, c, d}, optionally one group `g`
/// with children from {x, y}, placed at a random root position, plus
/// resolvers.
fn schema() -> impl Strategy<Value = Schema> {
    (
        columns(&["a", "b", "c", "d"]),
        proptest::option::of(group()),
        any::<usize>(),
        resolvers(),
    )
        .prop_map(|(mut root, group, at, resolvers)| {
            if let Some(g) = group {
                let at = at % (root.len() + 1);
                root.insert(at, g);
            }
            Schema { root, resolvers }
        })
}

/// One edit at a time, in place: retype, relabel, or drop an element,
/// and append new columns after — so the edited schema keeps the base's
/// relative order, which is what makes `merge(b, b, x) == x` exact.
fn edit_of(base: Schema) -> impl Strategy<Value = Schema> {
    fn edit_elements(
        elements: Vec<Element>,
        fresh: &'static [&'static str],
    ) -> BoxedStrategy<Vec<Element>> {
        let n = elements.len();
        let present: Vec<String> = elements
            .iter()
            .filter_map(|e| match e {
                Element::Column(c) => Some(c.id.to_string()),
                Element::Group(_) => None,
            })
            .collect();
        let fresh: Vec<&'static str> = fresh
            .iter()
            .copied()
            .filter(|id| !present.iter().any(|p| p == id))
            .collect();
        (
            proptest::collection::vec(0u8..4, n),
            proptest::collection::vec(scalar_type(), n),
            proptest::sample::subsequence(fresh.clone(), 0..=fresh.len()),
        )
            .prop_flat_map(move |(ops, types, added)| {
                let mut out = Vec::new();
                let mut group_children: Option<(usize, Vec<Element>)> = None;
                for ((el, op), ty) in elements.iter().cloned().zip(ops).zip(types) {
                    match (el, op) {
                        (_, 0) => {} // dropped
                        (Element::Column(mut c), 1) => {
                            c.ty = ty;
                            out.push(Element::Column(c));
                        }
                        (Element::Column(mut c), 2) => {
                            c.label = format!("{}'", c.label);
                            out.push(Element::Column(c));
                        }
                        (Element::Group(mut g), 1) => {
                            g.label = format!("{}'", g.label);
                            out.push(Element::Group(g));
                        }
                        (Element::Group(g), 2) => {
                            // Children edited recursively (below).
                            group_children = Some((out.len(), g.children.clone()));
                            out.push(Element::Group(g));
                        }
                        (el, _) => out.push(el),
                    }
                }
                for id in &added {
                    out.push(column(id, "new", ScalarType::Text));
                }
                match group_children {
                    None => Just(out).boxed(),
                    Some((index, children)) => edit_elements(children, &["x", "y"])
                        .prop_map(move |children| {
                            let mut out = out.clone();
                            if let Element::Group(g) = &mut out[index] {
                                g.children = children;
                            }
                            out
                        })
                        .boxed(),
                }
            })
            .boxed()
    }
    let resolvers = base.resolvers.clone();
    (
        edit_elements(base.root, &["a", "b", "c", "d"]),
        resolvers_edit(resolvers),
    )
        .prop_map(|(root, resolvers)| Schema { root, resolvers })
}

fn resolvers_edit(
    base: Vec<ResolverDeclaration>,
) -> impl Strategy<Value = Vec<ResolverDeclaration>> {
    proptest::collection::vec(0u8..3, base.len()).prop_map(move |ops| {
        base.iter()
            .cloned()
            .zip(ops)
            .filter_map(|(mut r, op)| match op {
                0 => None,
                1 => {
                    r.version += 10;
                    Some(r)
                }
                _ => Some(r),
            })
            .collect()
    })
}

/// Order-free view: every element with its container, children
/// detached (they appear on their own), plus the resolvers.
fn flattened(schema: &Schema) -> Vec<String> {
    fn walk(elements: &[Element], parent: Option<&GroupId>, out: &mut Vec<String>) {
        for el in elements {
            match el {
                Element::Column(c) => out.push(format!("{parent:?} {c:?}")),
                Element::Group(g) => {
                    let mut shell = g.clone();
                    shell.children = Vec::new();
                    out.push(format!("{parent:?} {shell:?}"));
                    walk(&g.children, Some(&g.id), out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&schema.root, None, &mut out);
    for r in &schema.resolvers {
        out.push(format!("resolver {r:?}"));
    }
    out.sort();
    out
}

proptest! {
    /// The side that did not move takes the other side — exactly, when
    /// the edit keeps the base's order (retype/relabel/drop/append), and
    /// order-free in general.
    #[test]
    fn unchanged_side_yields_the_other((base, edited) in schema().prop_flat_map(|b| (Just(b.clone()), edit_of(b)))) {
        prop_assert_eq!(merge(&base, &base, &edited), Ok(edited.clone()));
        prop_assert_eq!(merge(&base, &edited, &base), Ok(edited.clone()));
    }

    /// `merge(b, x, b) == x` for *any* x: the left side's order is the
    /// merged order, so nothing can shuffle. `merge(b, b, x)` is x up to
    /// order.
    #[test]
    fn unchanged_side_yields_the_other_for_arbitrary_schemas(base in schema(), x in schema()) {
        prop_assert_eq!(merge(&base, &x, &base), Ok(x.clone()));
        prop_assert_eq!(merge(&base, &x, &x), Ok(x.clone()));
        let via_right = merge(&base, &base, &x).unwrap();
        prop_assert_eq!(flattened(&via_right), flattened(&x));
    }

    /// Symmetric: both orientations succeed or fail together, with the
    /// same conflicts; when they succeed the results differ at most in
    /// element order.
    #[test]
    fn merge_is_symmetric(base in schema(), left in schema(), right in schema()) {
        match (merge(&base, &left, &right), merge(&base, &right, &left)) {
            (Ok(lr), Ok(rl)) => prop_assert_eq!(flattened(&lr), flattened(&rl)),
            (Err(lr), Err(rl)) => prop_assert_eq!(lr, rl),
            (lr, rl) => prop_assert!(false, "asymmetric: {lr:?} vs {rl:?}"),
        }
    }

    /// Whatever it returns, a merge never invents: every element of a
    /// successful merge comes verbatim from one of the three inputs, and
    /// a conflict names an id that exists in at least one of them.
    #[test]
    fn merge_never_invents(base in schema(), left in schema(), right in schema()) {
        let inputs: Vec<String> = [&base, &left, &right].iter().flat_map(|s| flattened(s)).collect();
        match merge(&base, &left, &right) {
            Ok(merged) => {
                for entry in flattened(&merged) {
                    prop_assert!(inputs.contains(&entry), "invented: {entry}");
                }
            }
            Err(conflicts) => {
                prop_assert!(!conflicts.is_empty());
                for c in conflicts {
                    let id = match &c {
                        MergeConflict::Column(id) => id.to_string(),
                        MergeConflict::Group(id) => id.to_string(),
                        MergeConflict::Resolver(id) => id.to_string(),
                        MergeConflict::OrphanedByDeletedGroup { element, .. } => element.clone(),
                    };
                    prop_assert!(inputs.iter().any(|e| e.contains(&format!("\"{id}\""))), "{c:?}");
                }
            }
        }
    }

    /// Delete a group on one side, edit inside it on the other: a
    /// conflict on either side, never a silent drop — an addition inside
    /// the deleted group is the orphan case, a retype inside it is a
    /// plain both-changed conflict on the column. Delete on one side
    /// with the other untouched: deleted, children included.
    #[test]
    fn delete_versus_edit_inside_a_group_conflicts(base in schema().prop_filter("has a group", |s| {
        s.root.iter().any(|e| matches!(e, Element::Group(_)))
    }), retype in scalar_type()) {
        let deleted = Schema {
            root: base.root.iter().filter(|e| !matches!(e, Element::Group(_))).cloned().collect(),
            resolvers: base.resolvers.clone(),
        };
        let mut added = base.clone();
        let mut retyped = base.clone();
        let mut first_child: Option<ColumnId> = None;
        for el in &mut added.root {
            if let Element::Group(g) = el {
                g.children.push(column("z", "z", ScalarType::Text));
            }
        }
        for el in &mut retyped.root {
            if let Element::Group(g) = el
                && let Some(Element::Column(c)) = g.children.first_mut()
            {
                c.ty = if c.ty == retype { ScalarType::Date } else { retype.clone() };
                first_child = Some(c.id.clone());
            }
        }
        for (l, r) in [(&deleted, &added), (&added, &deleted)] {
            let conflicts = merge(&base, l, r).unwrap_err();
            prop_assert_eq!(
                conflicts,
                vec![MergeConflict::OrphanedByDeletedGroup { group: GroupId::new("g"), element: "z".into() }]
            );
        }
        if let Some(child) = first_child {
            for (l, r) in [(&deleted, &retyped), (&retyped, &deleted)] {
                let conflicts = merge(&base, l, r).unwrap_err();
                prop_assert_eq!(conflicts, vec![MergeConflict::Column(child.clone())]);
            }
        }
        prop_assert_eq!(merge(&base, &deleted, &base), Ok(deleted.clone()));
        prop_assert_eq!(merge(&base, &base, &deleted), Ok(deleted.clone()));
    }
}

/// Resolver declarations follow the same three-way rule, by id.
#[test]
fn resolvers_merge_three_way() {
    let with = |decls: Vec<ResolverDeclaration>| Schema {
        root: vec![],
        resolvers: decls,
    };
    let base = with(vec![resolver("r", 1)]);
    // Both bump differently: conflict, named.
    assert_eq!(
        merge(
            &base,
            &with(vec![resolver("r", 2)]),
            &with(vec![resolver("r", 3)])
        ),
        Err(vec![MergeConflict::Resolver(ResolverId::new("r"))])
    );
    // One side bumps, the other leaves it: the bump wins.
    assert_eq!(
        merge(&base, &with(vec![resolver("r", 2)]), &base),
        Ok(with(vec![resolver("r", 2)]))
    );
    assert_eq!(
        merge(&base, &base, &with(vec![resolver("r", 2)])),
        Ok(with(vec![resolver("r", 2)]))
    );
    // Identical bumps agree.
    assert_eq!(
        merge(
            &base,
            &with(vec![resolver("r", 2)]),
            &with(vec![resolver("r", 2)])
        ),
        Ok(with(vec![resolver("r", 2)]))
    );
    // Delete vs untouched deletes; delete vs bump conflicts.
    assert_eq!(merge(&base, &with(vec![]), &base), Ok(with(vec![])));
    assert_eq!(
        merge(&base, &with(vec![]), &with(vec![resolver("r", 2)])),
        Err(vec![MergeConflict::Resolver(ResolverId::new("r"))])
    );
    // Disjoint additions combine.
    assert_eq!(
        merge(
            &base,
            &with(vec![resolver("r", 1), resolver("s", 1)]),
            &with(vec![resolver("r", 1), resolver("t", 1)])
        ),
        Ok(with(vec![
            resolver("r", 1),
            resolver("s", 1),
            resolver("t", 1)
        ]))
    );
}
