//! Element operations over a kernel [`Schema`] — the editing API a
//! procedure draft exposes (design/platform.md P.4, *Procedure
//! drafts*). Pure: no IO, no database; [`crate::procedure`] loads the
//! draft, runs one of these, and stores the result.
//!
//! The unit of editing is the **element** (column or group), because
//! groups nest. Every operation is atomic over the schema: it works on
//! a copy, runs the kernel's structural validation
//! ([`varve_schema::validate`], default [`DepthPolicy`]) and only then
//! replaces the original — a rejected edit leaves the draft untouched.
//!
//! Two consequences of the kernel model that the editor must respect:
//!
//! - **Ids are identity.** A column keeps its id across label, type
//!   and arity changes, which is what lets the impact report classify
//!   a type change rather than see a removal plus an addition (DESIGN
//!   §3). Ids are minted once, here ([`new_column_id`],
//!   [`new_group_id`]), never derived from labels.
//! - **`required`, visibility and presentation are surface properties**
//!   (DESIGN §2.6) and have no place in these operations; the surface
//!   draft joins the procedure draft later.

use varve_core::{ColumnId, GroupId};
use varve_schema::{
    Arity, Cardinality, DepthPolicy, Element, Group, ScalarType, Schema, SchemaError, validate,
};

/// Where an element sits: under the root, or inside a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parent {
    Root,
    Group(GroupId),
}

/// Either kind of element, by id. Column and group ids are separate
/// namespaces in the kernel (a column and a group may share a string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementId {
    Column(ColumnId),
    Group(GroupId),
}

/// A slot in a parent's children, anchored on a **sibling id** rather
/// than an index: `before: Some(id)` inserts in front of that child,
/// `before: None` appends. Ids survive concurrent edits the way
/// positions do not — an index is only meaningful against the tree
/// the editor last saw — and the two cases reach both ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub parent: Parent,
    pub before: Option<ElementId>,
}

impl Placement {
    /// Append under the root.
    pub fn root() -> Self {
        Self {
            parent: Parent::Root,
            before: None,
        }
    }

    /// Append inside `group`.
    pub fn in_group(group: GroupId) -> Self {
        Self {
            parent: Parent::Group(group),
            before: None,
        }
    }

    /// Insert in front of `sibling` (which must be a child of the parent).
    pub fn before(mut self, sibling: ElementId) -> Self {
        self.before = Some(sibling);
        self
    }
}

/// Fields of a column an edit may change; `None` leaves a field alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColumnPatch {
    pub label: Option<String>,
    pub ty: Option<ScalarType>,
    pub arity: Option<Arity>,
}

/// Fields of a group an edit may change; `None` leaves a field alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupPatch {
    pub label: Option<String>,
    pub cardinality: Option<Cardinality>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("no group '{0}' to place the element in")]
    UnknownParent(GroupId),
    #[error("no such element: {0:?}")]
    UnknownElement(ElementId),
    /// The `before` anchor is not a child of the placement's parent
    /// (or, on a move, is the moved element itself).
    #[error("{anchor:?} is not a child of {parent:?}")]
    AnchorNotInParent { parent: Parent, anchor: ElementId },
    /// The edit produced a schema the kernel rejects (duplicate id,
    /// nesting beyond policy, …). The draft is unchanged.
    #[error("the edit leaves the schema invalid: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<SchemaError>),
}

/// A fresh column id. Opaque (UUID v4, hex) — labels are not ids.
pub fn new_column_id() -> ColumnId {
    ColumnId::new(uuid::Uuid::new_v4().simple().to_string())
}

/// A fresh group id. Opaque (UUID v4, hex).
pub fn new_group_id() -> GroupId {
    GroupId::new(uuid::Uuid::new_v4().simple().to_string())
}

/// Inserts `element` at `placement`.
pub fn add_element(
    schema: &mut Schema,
    placement: &Placement,
    element: Element,
) -> Result<(), EditError> {
    commit(schema, |s| insert(s, placement, element))
}

/// Changes a column's label, type, or arity; the id is untouched.
pub fn update_column(
    schema: &mut Schema,
    id: &ColumnId,
    patch: ColumnPatch,
) -> Result<(), EditError> {
    commit(schema, |s| {
        let column = match element_mut(s, &ElementId::Column(id.clone()))? {
            Element::Column(c) => c,
            Element::Group(_) => unreachable!("located by column id"),
        };
        if let Some(label) = patch.label {
            column.label = label;
        }
        if let Some(ty) = patch.ty {
            column.ty = ty;
        }
        if let Some(arity) = patch.arity {
            column.arity = arity;
        }
        Ok(())
    })
}

/// Changes a group's label or cardinality; the id and children are
/// untouched.
pub fn update_group(schema: &mut Schema, id: &GroupId, patch: GroupPatch) -> Result<(), EditError> {
    commit(schema, |s| {
        let group = match element_mut(s, &ElementId::Group(id.clone()))? {
            Element::Group(g) => g,
            Element::Column(_) => unreachable!("located by group id"),
        };
        if let Some(label) = patch.label {
            group.label = label;
        }
        if let Some(cardinality) = patch.cardinality {
            group.cardinality = cardinality;
        }
        Ok(())
    })
}

/// Moves an element (a group moves with its subtree) to `placement`.
/// Moving a group into itself or one of its descendants fails with
/// [`EditError::UnknownParent`], and anchoring an element before
/// itself with [`EditError::AnchorNotInParent`]: once the subtree is
/// lifted out, neither exists.
pub fn move_element(
    schema: &mut Schema,
    id: &ElementId,
    placement: &Placement,
) -> Result<(), EditError> {
    commit(schema, |s| {
        let element = take(s, id)?;
        insert(s, placement, element)
    })
}

/// Removes an element (a group with its subtree) and returns it.
pub fn remove_element(schema: &mut Schema, id: &ElementId) -> Result<Element, EditError> {
    let mut removed = None;
    commit(schema, |s| {
        removed = Some(take(s, id)?);
        Ok(())
    })?;
    Ok(removed.expect("set on success"))
}

/// Runs `edit` on a copy; validates; replaces `schema` only on success.
fn commit(
    schema: &mut Schema,
    edit: impl FnOnce(&mut Schema) -> Result<(), EditError>,
) -> Result<(), EditError> {
    let mut draft = schema.clone();
    edit(&mut draft)?;
    let errors = validate(&draft, DepthPolicy::default());
    if !errors.is_empty() {
        return Err(EditError::Invalid(errors));
    }
    *schema = draft;
    Ok(())
}

fn insert(schema: &mut Schema, placement: &Placement, element: Element) -> Result<(), EditError> {
    let children = children_mut(schema, &placement.parent)?;
    let position = match &placement.before {
        None => children.len(),
        Some(anchor) => children.iter().position(|e| is(e, anchor)).ok_or_else(|| {
            EditError::AnchorNotInParent {
                parent: placement.parent.clone(),
                anchor: anchor.clone(),
            }
        })?,
    };
    children.insert(position, element);
    Ok(())
}

fn take(schema: &mut Schema, id: &ElementId) -> Result<Element, EditError> {
    let path = locate(&schema.root, id).ok_or_else(|| EditError::UnknownElement(id.clone()))?;
    let (last, parents) = path.split_last().expect("a located path is never empty");
    Ok(descend(&mut schema.root, parents).remove(*last))
}

fn element_mut<'a>(schema: &'a mut Schema, id: &ElementId) -> Result<&'a mut Element, EditError> {
    let path = locate(&schema.root, id).ok_or_else(|| EditError::UnknownElement(id.clone()))?;
    let (last, parents) = path.split_last().expect("a located path is never empty");
    Ok(&mut descend(&mut schema.root, parents)[*last])
}

fn children_mut<'a>(
    schema: &'a mut Schema,
    parent: &Parent,
) -> Result<&'a mut Vec<Element>, EditError> {
    match parent {
        Parent::Root => Ok(&mut schema.root),
        Parent::Group(id) => match element_mut(schema, &ElementId::Group(id.clone())) {
            Ok(Element::Group(g)) => Ok(&mut g.children),
            _ => Err(EditError::UnknownParent(id.clone())),
        },
    }
}

/// Index path from the root to the element with `id`.
fn locate(elements: &[Element], id: &ElementId) -> Option<Vec<usize>> {
    for (i, element) in elements.iter().enumerate() {
        if is(element, id) {
            return Some(vec![i]);
        }
        if let Element::Group(Group { children, .. }) = element
            && let Some(mut path) = locate(children, id)
        {
            path.insert(0, i);
            return Some(path);
        }
    }
    None
}

fn is(element: &Element, id: &ElementId) -> bool {
    match (element, id) {
        (Element::Column(c), ElementId::Column(want)) => &c.id == want,
        (Element::Group(g), ElementId::Group(want)) => &g.id == want,
        _ => false,
    }
}

/// The children vector reached by following `path` through groups.
fn descend<'a>(root: &'a mut Vec<Element>, path: &[usize]) -> &'a mut Vec<Element> {
    let mut current = root;
    for &i in path {
        current = match &mut current[i] {
            Element::Group(g) => &mut g.children,
            Element::Column(_) => unreachable!("located paths pass through groups only"),
        };
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use varve_schema::Column;

    fn column(id: &str) -> Element {
        Element::Column(Column {
            id: ColumnId::new(id),
            label: id.to_uppercase(),
            ty: ScalarType::Text,
            arity: Arity::One,
        })
    }

    fn group(id: &str, cardinality: Cardinality, children: Vec<Element>) -> Element {
        Element::Group(Group {
            id: GroupId::new(id),
            label: id.to_uppercase(),
            cardinality,
            children,
            included_from: None,
        })
    }

    fn ids(elements: &[Element]) -> Vec<&str> {
        elements
            .iter()
            .map(|e| match e {
                Element::Column(c) => c.id.as_str(),
                Element::Group(g) => g.id.as_str(),
            })
            .collect()
    }

    fn cid(id: &str) -> ElementId {
        ElementId::Column(ColumnId::new(id))
    }

    fn gid(id: &str) -> ElementId {
        ElementId::Group(GroupId::new(id))
    }

    /// root: [a, g1(many)[b, g2(one)[c]], d]
    fn fixture() -> Schema {
        Schema {
            root: vec![
                column("a"),
                group(
                    "g1",
                    Cardinality::Many,
                    vec![
                        column("b"),
                        group("g2", Cardinality::One, vec![column("c")]),
                    ],
                ),
                column("d"),
            ],
            resolvers: vec![],
        }
    }

    fn children_of<'a>(schema: &'a Schema, id: &str) -> &'a [Element] {
        match &schema.root[locate(&schema.root, &gid(id)).unwrap()[0]] {
            Element::Group(g) if g.id.as_str() == id => &g.children,
            Element::Group(g) => match &g.children[1] {
                Element::Group(inner) => &inner.children,
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn add_at_root_and_in_group_before_anchors() {
        let mut s = fixture();
        add_element(&mut s, &Placement::root().before(cid("a")), column("z")).unwrap();
        assert_eq!(ids(&s.root), ["z", "a", "g1", "d"]);
        add_element(
            &mut s,
            &Placement::in_group(GroupId::new("g1")),
            column("y"),
        )
        .unwrap();
        assert_eq!(ids(children_of(&s, "g1")), ["b", "g2", "y"]);
        add_element(
            &mut s,
            &Placement::in_group(GroupId::new("g2")).before(cid("c")),
            column("x"),
        )
        .unwrap();
        assert_eq!(ids(children_of(&s, "g2")), ["x", "c"]);
        // Anchoring before a group works the same as before a column.
        add_element(&mut s, &Placement::root().before(gid("g1")), column("w")).unwrap();
        assert_eq!(ids(&s.root), ["z", "a", "w", "g1", "d"]);
    }

    #[test]
    fn add_rejects_bad_parent_anchor_and_duplicate_id() {
        let mut s = fixture();
        let before = s.clone();
        assert_eq!(
            add_element(
                &mut s,
                &Placement::in_group(GroupId::new("nope")),
                column("z")
            ),
            Err(EditError::UnknownParent(GroupId::new("nope")))
        );
        // `c` exists, but not as a child of the root.
        assert_eq!(
            add_element(&mut s, &Placement::root().before(cid("c")), column("z")),
            Err(EditError::AnchorNotInParent {
                parent: Parent::Root,
                anchor: cid("c")
            })
        );
        assert!(matches!(
            add_element(&mut s, &Placement::root(), column("a")),
            Err(EditError::Invalid(errors)) if errors == [SchemaError::DuplicateColumnId(ColumnId::new("a"))]
        ));
        // Depth policy: a `many` group inside the `many` g1 exceeds depth 1.
        assert!(matches!(
            add_element(
                &mut s,
                &Placement::in_group(GroupId::new("g1")),
                group("deep", Cardinality::Many, vec![])
            ),
            Err(EditError::Invalid(_))
        ));
        assert_eq!(s, before, "a rejected edit leaves the schema untouched");
    }

    #[test]
    fn update_column_and_group_keep_ids() {
        let mut s = fixture();
        update_column(
            &mut s,
            &ColumnId::new("c"),
            ColumnPatch {
                label: Some("Ville".into()),
                ty: Some(ScalarType::Integer(None)),
                arity: None,
            },
        )
        .unwrap();
        match &children_of(&s, "g2")[0] {
            Element::Column(c) => {
                assert_eq!(c.id, ColumnId::new("c"));
                assert_eq!(c.label, "Ville");
                assert_eq!(c.ty, ScalarType::Integer(None));
                assert_eq!(c.arity, Arity::One);
            }
            _ => panic!(),
        }
        update_group(
            &mut s,
            &GroupId::new("g2"),
            GroupPatch {
                label: None,
                cardinality: Some(Cardinality::Many),
            },
        )
        .unwrap_err(); // many inside many: depth policy
        assert_eq!(
            update_column(&mut s, &ColumnId::new("g1"), ColumnPatch::default()),
            Err(EditError::UnknownElement(cid("g1"))),
            "column and group ids are separate namespaces"
        );
    }

    #[test]
    fn move_across_levels_and_reorder() {
        let mut s = fixture();
        move_element(&mut s, &cid("c"), &Placement::root().before(cid("a"))).unwrap();
        assert_eq!(ids(&s.root), ["c", "a", "g1", "d"]);
        assert!(children_of(&s, "g2").is_empty());
        move_element(&mut s, &gid("g2"), &Placement::root()).unwrap();
        assert_eq!(ids(&s.root), ["c", "a", "g1", "d", "g2"]);
        assert_eq!(ids(children_of(&s, "g1")), ["b"]);
        // Reorder within the same parent, in both directions.
        move_element(&mut s, &cid("d"), &Placement::root().before(cid("c"))).unwrap();
        assert_eq!(ids(&s.root), ["d", "c", "a", "g1", "g2"]);
        move_element(&mut s, &cid("d"), &Placement::root().before(gid("g2"))).unwrap();
        assert_eq!(ids(&s.root), ["c", "a", "g1", "d", "g2"]);
        // An element cannot anchor before itself.
        assert_eq!(
            move_element(&mut s, &cid("d"), &Placement::root().before(cid("d"))),
            Err(EditError::AnchorNotInParent {
                parent: Parent::Root,
                anchor: cid("d")
            })
        );
    }

    #[test]
    fn move_into_own_subtree_fails_and_leaves_schema_untouched() {
        let mut s = fixture();
        let before = s.clone();
        assert_eq!(
            move_element(&mut s, &gid("g1"), &Placement::in_group(GroupId::new("g2"))),
            Err(EditError::UnknownParent(GroupId::new("g2")))
        );
        assert_eq!(
            move_element(&mut s, &gid("g1"), &Placement::in_group(GroupId::new("g1"))),
            Err(EditError::UnknownParent(GroupId::new("g1")))
        );
        assert_eq!(s, before);
    }

    #[test]
    fn remove_returns_the_subtree() {
        let mut s = fixture();
        let removed = remove_element(&mut s, &gid("g1")).unwrap();
        assert_eq!(ids(&s.root), ["a", "d"]);
        match removed {
            Element::Group(g) => assert_eq!(ids(&g.children), ["b", "g2"]),
            _ => panic!(),
        }
        assert_eq!(
            remove_element(&mut s, &cid("b")),
            Err(EditError::UnknownElement(cid("b")))
        );
    }

    #[test]
    fn fresh_ids_are_distinct() {
        assert_ne!(new_column_id(), new_column_id());
        assert_ne!(new_group_id(), new_group_id());
    }
}
