//! Three-way schema merge (§7): schemas are few and high-value, so
//! they get git semantics. Property-wise merge over columns, groups
//! and resolver declarations by stable id; the merged tree keeps the
//! left side's ordering with right-side additions appended per
//! container. Conflicts are reported, never guessed.

use std::collections::{BTreeMap, BTreeSet};

use varve_core::{ColumnId, GroupId, ResolverId};
use varve_schema::{Column, Element, Group, ResolverDeclaration, Schema};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Column(ColumnId),
    Group(GroupId),
}

/// Direct parent container: root or a group.
type Container = Option<GroupId>;

#[derive(Debug, Clone, PartialEq)]
enum Def {
    Column { column: Column, parent: Container },
    Group { label: String, group: Group, parent: Container },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MergeConflict {
    #[error("column '{0}': both sides modified it differently")]
    Column(ColumnId),
    #[error("group '{0}': both sides modified it differently")]
    Group(GroupId),
    #[error("resolver '{0}': both sides modified it differently")]
    Resolver(ResolverId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    BothModified,
}

struct Flat {
    defs: BTreeMap<Key, Def>,
    /// Ordered element keys per container.
    containers: BTreeMap<Container, Vec<Key>>,
}

fn flatten(schema: &Schema) -> Flat {
    fn walk(elements: &[Element], parent: &Container, flat: &mut Flat) {
        for el in elements {
            match el {
                Element::Column(c) => {
                    let key = Key::Column(c.id.clone());
                    flat.containers.entry(parent.clone()).or_default().push(key.clone());
                    flat.defs.insert(
                        key,
                        Def::Column { column: c.clone(), parent: parent.clone() },
                    );
                }
                Element::Group(g) => {
                    let key = Key::Group(g.id.clone());
                    flat.containers.entry(parent.clone()).or_default().push(key.clone());
                    let mut shell = g.clone();
                    shell.children = Vec::new(); // children merge on their own
                    flat.defs.insert(
                        key,
                        Def::Group {
                            label: g.label.clone(),
                            group: shell,
                            parent: parent.clone(),
                        },
                    );
                    walk(&g.children, &Some(g.id.clone()), flat);
                }
            }
        }
    }
    let mut flat = Flat { defs: BTreeMap::new(), containers: BTreeMap::new() };
    walk(&schema.root, &None, &mut flat);
    flat
}

/// The three-way rule: unchanged-on-one-side takes the other side;
/// both-changed-identically agrees; both-changed-differently conflicts.
fn merge3<T: PartialEq + Clone>(
    base: Option<&T>,
    left: Option<&T>,
    right: Option<&T>,
) -> Result<Option<T>, ()> {
    if left == right {
        return Ok(left.cloned());
    }
    if left == base {
        return Ok(right.cloned());
    }
    if right == base {
        return Ok(left.cloned());
    }
    Err(())
}

pub fn merge(
    base: &Schema,
    left: &Schema,
    right: &Schema,
) -> Result<Schema, Vec<MergeConflict>> {
    let (b, l, r) = (flatten(base), flatten(left), flatten(right));
    let mut conflicts = Vec::new();
    let mut merged: BTreeMap<Key, Def> = BTreeMap::new();

    let keys: BTreeSet<&Key> =
        b.defs.keys().chain(l.defs.keys()).chain(r.defs.keys()).collect();
    for key in keys {
        match merge3(b.defs.get(key), l.defs.get(key), r.defs.get(key)) {
            Ok(Some(def)) => {
                merged.insert(key.clone(), def);
            }
            Ok(None) => {}
            Err(()) => conflicts.push(match key {
                Key::Column(id) => MergeConflict::Column(id.clone()),
                Key::Group(id) => MergeConflict::Group(id.clone()),
            }),
        }
    }

    // Resolvers: whole-declaration three-way by id.
    let resolver_map = |s: &Schema| -> BTreeMap<ResolverId, ResolverDeclaration> {
        s.resolvers.iter().map(|d| (d.id.clone(), d.clone())).collect()
    };
    let (rb, rl, rr) = (resolver_map(base), resolver_map(left), resolver_map(right));
    let mut resolvers = Vec::new();
    let ids: BTreeSet<&ResolverId> = rb.keys().chain(rl.keys()).chain(rr.keys()).collect();
    for id in ids {
        match merge3(rb.get(id), rl.get(id), rr.get(id)) {
            Ok(Some(decl)) => resolvers.push(decl),
            Ok(None) => {}
            Err(()) => conflicts.push(MergeConflict::Resolver(id.clone())),
        }
    }

    if !conflicts.is_empty() {
        return Err(conflicts);
    }

    // Rebuild: per container, left's order then right-only additions;
    // elements are placed by their *merged* parent.
    fn rebuild(
        container: &Container,
        merged: &BTreeMap<Key, Def>,
        l: &Flat,
        r: &Flat,
    ) -> Vec<Element> {
        let mut ordered: Vec<Key> = Vec::new();
        for source in [l, r] {
            if let Some(keys) = source.containers.get(container) {
                for key in keys {
                    if !ordered.contains(key) {
                        ordered.push(key.clone());
                    }
                }
            }
        }
        let mut out = Vec::new();
        for key in ordered {
            match merged.get(&key) {
                Some(Def::Column { column, parent }) if parent == container => {
                    out.push(Element::Column(column.clone()));
                }
                Some(Def::Group { group, parent, .. }) if parent == container => {
                    let mut group = group.clone();
                    group.children = rebuild(&Some(group.id.clone()), merged, l, r);
                    out.push(Element::Group(group));
                }
                _ => {}
            }
        }
        out
    }
    Ok(Schema { root: rebuild(&None, &merged, &l, &r), resolvers })
}
