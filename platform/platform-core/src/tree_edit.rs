//! Element operations over the **authored tree** — the editing API a
//! procedure draft exposes (design/platform.md P.4, *The authored
//! tree is the draft's single source*). Pure: no IO, no database;
//! [`crate::procedure`] loads the draft, runs one of these, and
//! stores the result.
//!
//! The unit of editing is the **element** — column, group, section or
//! note — because groups and sections nest. Every operation is atomic
//! over the tree: it works on a copy, derives the kernel schema
//! ([`Tree::schema`]) and runs the kernel's structural validation
//! ([`varve_schema::validate`], default [`DepthPolicy`]) plus the
//! tree's own checks (unique node ids, DESIGN §2.6), and only then
//! replaces the original — a rejected edit leaves the draft
//! untouched.
//!
//! Consequences of the kernel model the editor must respect:
//!
//! - **Ids are identity** (DESIGN §3, §2.6): columns and groups keep
//!   their ids across every change, and sections and notes carry
//!   minted [`NodeId`]s — a retitle is a retitle, never a removal
//!   plus an addition. Ids are minted once, here, never derived from
//!   labels.
//! - **Audience is inherited** (P.4, amended 2026-08-24): the
//!   effective audience of an element is the narrowest along its
//!   ancestor path. Adding clamps the marker to the parent's
//!   effective audience; explicitly *widening* an element beyond its
//!   parent is the refused contradiction
//!   ([`EditError::AudienceWiderThanParent`]).

use varve_core::{ColumnId, GroupId, NodeId, OptionId};
use varve_schema::{Arity, Cardinality, DepthPolicy, ScalarType, SchemaError, validate};
use varve_surface::Format;

use crate::tree::{Audience, Tree, TreeElement};

/// Where an element sits: under the root, inside a group, or inside a
/// section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parent {
    Root,
    Group(GroupId),
    Section(NodeId),
}

/// Any kind of element, by id. Column, group and node ids are separate
/// namespaces in the kernel (two kinds may share a string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementId {
    Column(ColumnId),
    Group(GroupId),
    Section(NodeId),
    Note(NodeId),
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

    /// Append inside `section`.
    pub fn in_section(section: NodeId) -> Self {
        Self {
            parent: Parent::Section(section),
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
    pub required: Option<bool>,
    /// `Some(None)` clears the constraint; `None` leaves it — except
    /// through a type change away from text, which resets it (the
    /// arity precedent).
    pub format: Option<Option<Format>>,
    pub audience: Option<Audience>,
}

/// Fields of a group an edit may change; `None` leaves a field alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupPatch {
    pub label: Option<String>,
    pub cardinality: Option<Cardinality>,
    pub audience: Option<Audience>,
}

/// Fields of a section an edit may change; `None` leaves a field
/// alone. `help` is clearable: `Some(None)` removes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionPatch {
    pub title: Option<String>,
    pub help: Option<Option<String>>,
    pub audience: Option<Audience>,
}

/// Fields of a note an edit may change; `None` leaves a field alone.
/// `title` is clearable: `Some(None)` removes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotePatch {
    pub title: Option<Option<String>>,
    pub body: Option<String>,
    pub audience: Option<Audience>,
}

/// Whether a column of type `ty` may hold many values (arity `many`).
/// A **platform rule, not a kernel one**: the kernel lets any column
/// be list-valued (DESIGN §2.2), but in the whole DN corpus `many`
/// occurs only on attachments (multi-file), enums (multi-select) and
/// geometries (feature sets) — `corpus/M0-type-frequency.md` — so the
/// editor offers it there and nowhere else (design/platform.md P.4).
pub fn list_capable(ty: &ScalarType) -> bool {
    matches!(
        ty,
        ScalarType::Enum(_) | ScalarType::Attachment(_) | ScalarType::Geometry
    )
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("no parent {0:?} to place the element in")]
    UnknownParent(Parent),
    #[error("no such element: {0:?}")]
    UnknownElement(ElementId),
    /// The `before` anchor is not a child of the placement's parent
    /// (or, on a move, is the moved element itself).
    #[error("{anchor:?} is not a child of {parent:?}")]
    AnchorNotInParent { parent: Parent, anchor: ElementId },
    /// Arity `many` asked on a column whose type is not list-capable
    /// ([`list_capable`]).
    #[error("column '{0}' cannot hold many values: only choices, attachments and geometries can")]
    ArityNotOffered(ColumnId),
    /// A format constraint on a non-text column (§2.6: format is
    /// admissibility over text).
    #[error("column '{0}': format constraints apply to text columns only")]
    FormatNotOffered(ColumnId),
    /// A custom pattern the linear-time engine refuses (§2.6: no
    /// backtracking is a security property) — refused now, not a
    /// stored mistake surfacing at publication.
    #[error("column '{0}': invalid format pattern: {1}")]
    InvalidPattern(ColumnId, String),
    /// An element cannot be explicitly wider than its parent's
    /// effective audience (P.4: it would be pruned with the parent's
    /// subtree anyway — the marker would lie).
    #[error("{0:?} cannot be wider than its parent's audience")]
    AudienceWiderThanParent(ElementId),
    /// A section or note with this id already exists (§2.6: minted
    /// ids are identity).
    #[error("node id '{0}' appears more than once")]
    DuplicateNode(NodeId),
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

/// A fresh presentation-node id for a section or note (DESIGN §2.6,
/// surface node identity). Opaque (UUID v4, hex).
pub fn new_node_id() -> NodeId {
    NodeId::new(uuid::Uuid::new_v4().simple().to_string())
}

/// A fresh inline-enum option id (DESIGN §2.12: synthesized by the
/// authoring tool). Opaque (UUID v4, hex).
pub fn new_option_id() -> OptionId {
    OptionId::new(uuid::Uuid::new_v4().simple().to_string())
}

/// Inserts `element` at `placement`. The element's audience is
/// clamped to the parent's effective audience (adding inside a
/// reviewer-only section yields a reviewer-only element; nothing to
/// refuse).
pub fn add_element(
    tree: &mut Tree,
    placement: &Placement,
    mut element: TreeElement,
) -> Result<(), EditError> {
    if let TreeElement::Column(c) = &element {
        if c.arity == Arity::Many && !list_capable(&c.ty) {
            return Err(EditError::ArityNotOffered(c.id.clone()));
        }
        check_format(c)?;
    }
    commit(tree, |t| {
        let parent_audience = effective_audience(t, &placement.parent)?;
        clamp_audience(&mut element, parent_audience);
        insert(t, placement, element)
    })
}

/// Changes a column's label, type, arity or audience; the id is
/// untouched.
pub fn update_column(tree: &mut Tree, id: &ColumnId, patch: ColumnPatch) -> Result<(), EditError> {
    let element_id = ElementId::Column(id.clone());
    commit(tree, |t| {
        check_audience(t, &element_id, patch.audience)?;
        let column = match element_mut(t, &element_id)? {
            TreeElement::Column(c) => c,
            _ => unreachable!("located by column id"),
        };
        if let Some(label) = patch.label {
            column.label = label;
        }
        if let Some(ty) = patch.ty {
            column.ty = ty;
        }
        match patch.arity {
            Some(arity) => column.arity = arity,
            // A type change to one that cannot hold many values takes
            // the arity back to `one` rather than failing: the kind is
            // what the editor asked for, the arity follows from it.
            None if !list_capable(&column.ty) => column.arity = Arity::One,
            None => {}
        }
        if column.arity == Arity::Many && !list_capable(&column.ty) {
            return Err(EditError::ArityNotOffered(column.id.clone()));
        }
        if let Some(required) = patch.required {
            column.required = required;
        }
        match patch.format {
            Some(format) => column.format = format,
            // A type change away from text takes the constraint with
            // it, like the arity: the kind is what the editor asked
            // for, the format cannot outlive it.
            None if !matches!(column.ty, ScalarType::Text) => column.format = None,
            None => {}
        }
        if let Some(audience) = patch.audience {
            column.audience = audience;
        }
        check_format(column)?;
        Ok(())
    })
}

/// Changes a group's label, cardinality or audience; the id and
/// children are untouched.
pub fn update_group(tree: &mut Tree, id: &GroupId, patch: GroupPatch) -> Result<(), EditError> {
    let element_id = ElementId::Group(id.clone());
    commit(tree, |t| {
        check_audience(t, &element_id, patch.audience)?;
        let group = match element_mut(t, &element_id)? {
            TreeElement::Group(g) => g,
            _ => unreachable!("located by group id"),
        };
        if let Some(label) = patch.label {
            group.label = label;
        }
        if let Some(cardinality) = patch.cardinality {
            group.cardinality = cardinality;
        }
        if let Some(audience) = patch.audience {
            group.audience = audience;
        }
        Ok(())
    })
}

/// Changes a section's title, help or audience; the id and children
/// are untouched.
pub fn update_section(tree: &mut Tree, id: &NodeId, patch: SectionPatch) -> Result<(), EditError> {
    let element_id = ElementId::Section(id.clone());
    commit(tree, |t| {
        check_audience(t, &element_id, patch.audience)?;
        let section = match element_mut(t, &element_id)? {
            TreeElement::Section(s) => s,
            _ => unreachable!("located by section id"),
        };
        if let Some(title) = patch.title {
            section.title = title;
        }
        if let Some(help) = patch.help {
            section.help = help;
        }
        if let Some(audience) = patch.audience {
            section.audience = audience;
        }
        Ok(())
    })
}

/// Changes a note's title, body or audience; the id is untouched.
pub fn update_note(tree: &mut Tree, id: &NodeId, patch: NotePatch) -> Result<(), EditError> {
    let element_id = ElementId::Note(id.clone());
    commit(tree, |t| {
        check_audience(t, &element_id, patch.audience)?;
        let note = match element_mut(t, &element_id)? {
            TreeElement::Note(n) => n,
            _ => unreachable!("located by note id"),
        };
        if let Some(title) = patch.title {
            note.title = title;
        }
        if let Some(body) = patch.body {
            note.body = body;
        }
        if let Some(audience) = patch.audience {
            note.audience = audience;
        }
        Ok(())
    })
}

/// Moves an element (a group or section moves with its subtree) to
/// `placement`. The markers are untouched: audience is inherited, so
/// landing inside a reviewer-only parent narrows the subtree without
/// rewriting it. Moving a container into itself or a descendant fails
/// with [`EditError::UnknownParent`], and anchoring an element before
/// itself with [`EditError::AnchorNotInParent`]: once the subtree is
/// lifted out, neither exists.
pub fn move_element(
    tree: &mut Tree,
    id: &ElementId,
    placement: &Placement,
) -> Result<(), EditError> {
    commit(tree, |t| {
        let element = take(t, id)?;
        insert(t, placement, element)
    })
}

/// Removes an element (a group or section with its subtree) and
/// returns it.
pub fn remove_element(tree: &mut Tree, id: &ElementId) -> Result<TreeElement, EditError> {
    let mut removed = None;
    commit(tree, |t| {
        removed = Some(take(t, id)?);
        Ok(())
    })?;
    Ok(removed.expect("set on success"))
}

/// Runs `edit` on a copy; validates the derived schema and the
/// tree's own invariants; replaces `tree` only on success.
fn commit(
    tree: &mut Tree,
    edit: impl FnOnce(&mut Tree) -> Result<(), EditError>,
) -> Result<(), EditError> {
    let mut draft = tree.clone();
    edit(&mut draft)?;
    let errors = validate(&draft.schema(), DepthPolicy::default());
    if !errors.is_empty() {
        return Err(EditError::Invalid(errors));
    }
    check_unique_nodes(&draft)?;
    *tree = draft;
    Ok(())
}

/// §2.6: minted node ids are identity — a repeat would make two
/// presentation nodes one. (Column and group duplicates are caught by
/// the kernel on the derived schema.)
fn check_unique_nodes(tree: &Tree) -> Result<(), EditError> {
    fn walk(
        elements: &[TreeElement],
        seen: &mut std::collections::BTreeSet<NodeId>,
    ) -> Result<(), EditError> {
        for element in elements {
            match element {
                TreeElement::Section(s) => {
                    if !seen.insert(s.id.clone()) {
                        return Err(EditError::DuplicateNode(s.id.clone()));
                    }
                    walk(&s.children, seen)?;
                }
                TreeElement::Note(n) => {
                    if !seen.insert(n.id.clone()) {
                        return Err(EditError::DuplicateNode(n.id.clone()));
                    }
                }
                TreeElement::Group(g) => walk(&g.children, seen)?,
                TreeElement::Column(_) => {}
            }
        }
        Ok(())
    }
    walk(&tree.elements, &mut std::collections::BTreeSet::new())
}

/// §2.6's two format backstops: text-only, and patterns the
/// linear-time engine accepts ([`Format::verify`]).
fn check_format(column: &crate::tree::TreeColumn) -> Result<(), EditError> {
    let Some(format) = &column.format else {
        return Ok(());
    };
    if !matches!(column.ty, ScalarType::Text) {
        return Err(EditError::FormatNotOffered(column.id.clone()));
    }
    if let Err(reason) = format.verify() {
        return Err(EditError::InvalidPattern(column.id.clone(), reason));
    }
    Ok(())
}

/// The effective audience at `parent`: the narrowest along its path
/// (the root is `All`). Public: the API layer defaults a new
/// column's requiredness by it (G.7 *Required on columns*).
pub fn effective_audience(tree: &Tree, parent: &Parent) -> Result<Audience, EditError> {
    let id = match parent {
        Parent::Root => return Ok(Audience::All),
        Parent::Group(id) => ElementId::Group(id.clone()),
        Parent::Section(id) => ElementId::Section(id.clone()),
    };
    let path =
        locate(&tree.elements, &id).ok_or_else(|| EditError::UnknownParent(parent.clone()))?;
    Ok(effective_along(&tree.elements, &path))
}

/// The narrowest audience over the elements the index path enters.
fn effective_along(elements: &[TreeElement], path: &[usize]) -> Audience {
    let mut audience = Audience::All;
    let mut current = elements;
    for &i in path {
        audience = audience.narrowest(current[i].audience());
        current = match &current[i] {
            TreeElement::Group(g) => &g.children,
            TreeElement::Section(s) => &s.children,
            _ => &[],
        };
    }
    audience
}

/// Refuses a patch that would make `id` explicitly wider than its
/// parent's effective audience (P.4: the marker would lie — the
/// element is pruned with the parent's subtree regardless).
fn check_audience(
    tree: &Tree,
    id: &ElementId,
    requested: Option<Audience>,
) -> Result<(), EditError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    let path = locate(&tree.elements, id).ok_or_else(|| EditError::UnknownElement(id.clone()))?;
    let (_, ancestors) = path.split_last().expect("a located path is never empty");
    if requested.wider_than(effective_along(&tree.elements, ancestors)) {
        return Err(EditError::AudienceWiderThanParent(id.clone()));
    }
    Ok(())
}

fn clamp_audience(element: &mut TreeElement, parent: Audience) {
    let clamped = element.audience().narrowest(parent);
    match element {
        TreeElement::Column(c) => c.audience = clamped,
        TreeElement::Group(g) => g.audience = clamped,
        TreeElement::Section(s) => s.audience = clamped,
        TreeElement::Note(n) => n.audience = clamped,
    }
}

fn insert(tree: &mut Tree, placement: &Placement, element: TreeElement) -> Result<(), EditError> {
    let children = children_mut(tree, &placement.parent)?;
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

fn take(tree: &mut Tree, id: &ElementId) -> Result<TreeElement, EditError> {
    let path = locate(&tree.elements, id).ok_or_else(|| EditError::UnknownElement(id.clone()))?;
    let (last, parents) = path.split_last().expect("a located path is never empty");
    Ok(descend(&mut tree.elements, parents).remove(*last))
}

fn element_mut<'a>(tree: &'a mut Tree, id: &ElementId) -> Result<&'a mut TreeElement, EditError> {
    let path = locate(&tree.elements, id).ok_or_else(|| EditError::UnknownElement(id.clone()))?;
    let (last, parents) = path.split_last().expect("a located path is never empty");
    Ok(&mut descend(&mut tree.elements, parents)[*last])
}

fn children_mut<'a>(
    tree: &'a mut Tree,
    parent: &Parent,
) -> Result<&'a mut Vec<TreeElement>, EditError> {
    match parent {
        Parent::Root => Ok(&mut tree.elements),
        Parent::Group(id) => match element_mut(tree, &ElementId::Group(id.clone())) {
            Ok(TreeElement::Group(g)) => Ok(&mut g.children),
            _ => Err(EditError::UnknownParent(parent.clone())),
        },
        Parent::Section(id) => match element_mut(tree, &ElementId::Section(id.clone())) {
            Ok(TreeElement::Section(s)) => Ok(&mut s.children),
            _ => Err(EditError::UnknownParent(parent.clone())),
        },
    }
}

/// Index path from the root to the element with `id`.
fn locate(elements: &[TreeElement], id: &ElementId) -> Option<Vec<usize>> {
    for (i, element) in elements.iter().enumerate() {
        if is(element, id) {
            return Some(vec![i]);
        }
        let children = match element {
            TreeElement::Group(g) => Some(&g.children),
            TreeElement::Section(s) => Some(&s.children),
            _ => None,
        };
        if let Some(children) = children
            && let Some(mut path) = locate(children, id)
        {
            path.insert(0, i);
            return Some(path);
        }
    }
    None
}

fn is(element: &TreeElement, id: &ElementId) -> bool {
    match (element, id) {
        (TreeElement::Column(c), ElementId::Column(want)) => &c.id == want,
        (TreeElement::Group(g), ElementId::Group(want)) => &g.id == want,
        (TreeElement::Section(s), ElementId::Section(want)) => &s.id == want,
        (TreeElement::Note(n), ElementId::Note(want)) => &n.id == want,
        _ => false,
    }
}

/// The children vector reached by following `path` through containers.
fn descend<'a>(root: &'a mut Vec<TreeElement>, path: &[usize]) -> &'a mut Vec<TreeElement> {
    let mut current = root;
    for &i in path {
        current = match &mut current[i] {
            TreeElement::Group(g) => &mut g.children,
            TreeElement::Section(s) => &mut s.children,
            _ => unreachable!("located paths pass through containers only"),
        };
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{TreeColumn, TreeGroup, TreeNote, TreeSection};

    fn column(id: &str) -> TreeElement {
        TreeElement::Column(TreeColumn {
            id: ColumnId::new(id),
            label: id.to_uppercase(),
            ty: ScalarType::Text,
            arity: Arity::One,
            format: None,
            required: true,
            audience: Audience::All,
        })
    }

    fn group(id: &str, cardinality: Cardinality, children: Vec<TreeElement>) -> TreeElement {
        TreeElement::Group(TreeGroup {
            id: GroupId::new(id),
            label: id.to_uppercase(),
            cardinality,
            audience: Audience::All,
            children,
        })
    }

    fn section(id: &str, children: Vec<TreeElement>) -> TreeElement {
        TreeElement::Section(TreeSection {
            id: NodeId::new(id),
            title: id.to_uppercase(),
            help: None,
            audience: Audience::All,
            children,
        })
    }

    fn note(id: &str) -> TreeElement {
        TreeElement::Note(TreeNote {
            id: NodeId::new(id),
            title: None,
            body: "…".into(),
            audience: Audience::All,
        })
    }

    fn ids(elements: &[TreeElement]) -> Vec<&str> {
        elements
            .iter()
            .map(|e| match e {
                TreeElement::Column(c) => c.id.as_str(),
                TreeElement::Group(g) => g.id.as_str(),
                TreeElement::Section(s) => s.id.as_str(),
                TreeElement::Note(n) => n.id.as_str(),
            })
            .collect()
    }

    fn cid(id: &str) -> ElementId {
        ElementId::Column(ColumnId::new(id))
    }

    fn gid(id: &str) -> ElementId {
        ElementId::Group(GroupId::new(id))
    }

    fn sid(id: &str) -> ElementId {
        ElementId::Section(NodeId::new(id))
    }

    fn nid(id: &str) -> ElementId {
        ElementId::Note(NodeId::new(id))
    }

    /// root: [a, g1(many)[b, g2(one)[c]], s1[n1, d]]
    fn fixture() -> Tree {
        Tree {
            elements: vec![
                column("a"),
                group(
                    "g1",
                    Cardinality::Many,
                    vec![
                        column("b"),
                        group("g2", Cardinality::One, vec![column("c")]),
                    ],
                ),
                section("s1", vec![note("n1"), column("d")]),
            ],
        }
    }

    fn children_of<'a>(tree: &'a Tree, id: &ElementId) -> &'a [TreeElement] {
        fn find<'a>(elements: &'a [TreeElement], id: &ElementId) -> Option<&'a [TreeElement]> {
            for element in elements {
                let (matches, children) = match element {
                    TreeElement::Group(g) => (is(element, id), Some(g.children.as_slice())),
                    TreeElement::Section(s) => (is(element, id), Some(s.children.as_slice())),
                    _ => (false, None),
                };
                if let Some(children) = children {
                    if matches {
                        return Some(children);
                    }
                    if let Some(found) = find(children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }
        find(&tree.elements, id).expect("container exists")
    }

    #[test]
    fn add_into_groups_and_sections_before_anchors() {
        let mut t = fixture();
        add_element(&mut t, &Placement::root().before(cid("a")), column("z")).unwrap();
        assert_eq!(ids(&t.elements), ["z", "a", "g1", "s1"]);
        add_element(
            &mut t,
            &Placement::in_group(GroupId::new("g1")),
            column("y"),
        )
        .unwrap();
        assert_eq!(ids(children_of(&t, &gid("g1"))), ["b", "g2", "y"]);
        // A section is a parent like a group; a note is a sibling like
        // any other.
        add_element(
            &mut t,
            &Placement::in_section(NodeId::new("s1")).before(cid("d")),
            column("x"),
        )
        .unwrap();
        assert_eq!(ids(children_of(&t, &sid("s1"))), ["n1", "x", "d"]);
        add_element(&mut t, &Placement::root().before(sid("s1")), note("n2")).unwrap();
        assert_eq!(ids(&t.elements), ["z", "a", "g1", "n2", "s1"]);
        // Sections nest (kernel-mirroring), and groups may hold
        // sections.
        add_element(
            &mut t,
            &Placement::in_section(NodeId::new("s1")),
            section("s2", vec![]),
        )
        .unwrap();
        add_element(
            &mut t,
            &Placement::in_group(GroupId::new("g1")),
            section("s3", vec![]),
        )
        .unwrap();
        assert_eq!(ids(children_of(&t, &sid("s1"))), ["n1", "x", "d", "s2"]);
    }

    #[test]
    fn add_rejects_bad_parent_anchor_and_duplicates() {
        let mut t = fixture();
        let before = t.clone();
        assert_eq!(
            add_element(
                &mut t,
                &Placement::in_group(GroupId::new("nope")),
                column("z")
            ),
            Err(EditError::UnknownParent(Parent::Group(GroupId::new(
                "nope"
            ))))
        );
        assert_eq!(
            add_element(
                &mut t,
                &Placement::in_section(NodeId::new("nope")),
                column("z")
            ),
            Err(EditError::UnknownParent(Parent::Section(NodeId::new(
                "nope"
            ))))
        );
        // `c` exists, but not as a child of the root.
        assert_eq!(
            add_element(&mut t, &Placement::root().before(cid("c")), column("z")),
            Err(EditError::AnchorNotInParent {
                parent: Parent::Root,
                anchor: cid("c")
            })
        );
        // Duplicate column id: caught by the kernel on the derived
        // schema. Duplicate node id: caught by the tree.
        assert!(matches!(
            add_element(&mut t, &Placement::root(), column("a")),
            Err(EditError::Invalid(errors))
                if errors == [SchemaError::DuplicateColumnId(ColumnId::new("a"))]
        ));
        assert_eq!(
            add_element(&mut t, &Placement::root(), note("n1")),
            Err(EditError::DuplicateNode(NodeId::new("n1")))
        );
        assert_eq!(
            add_element(&mut t, &Placement::root(), section("s1", vec![])),
            Err(EditError::DuplicateNode(NodeId::new("s1")))
        );
        // Depth policy: a `many` group inside the `many` g1 exceeds
        // depth 1 — even hidden inside a section.
        assert!(matches!(
            add_element(
                &mut t,
                &Placement::in_group(GroupId::new("g1")),
                section("s9", vec![group("deep", Cardinality::Many, vec![])])
            ),
            Err(EditError::Invalid(_))
        ));
        assert_eq!(t, before, "a rejected edit leaves the tree untouched");
    }

    #[test]
    fn update_each_kind_keeps_ids() {
        let mut t = fixture();
        update_column(
            &mut t,
            &ColumnId::new("c"),
            ColumnPatch {
                label: Some("Ville".into()),
                ty: Some(ScalarType::Integer(None)),
                ..Default::default()
            },
        )
        .unwrap();
        match &children_of(&t, &gid("g2"))[0] {
            TreeElement::Column(c) => {
                assert_eq!(c.id, ColumnId::new("c"));
                assert_eq!(c.label, "Ville");
                assert_eq!(c.ty, ScalarType::Integer(None));
            }
            _ => panic!(),
        }
        update_section(
            &mut t,
            &NodeId::new("s1"),
            SectionPatch {
                title: Some("Identité".into()),
                help: Some(Some("Vos informations".into())),
                ..Default::default()
            },
        )
        .unwrap();
        update_note(
            &mut t,
            &NodeId::new("n1"),
            NotePatch {
                title: Some(Some("Attention".into())),
                body: Some("Pensez au SIRET.".into()),
                ..Default::default()
            },
        )
        .unwrap();
        match &t.elements[2] {
            TreeElement::Section(s) => {
                assert_eq!(s.title, "Identité");
                assert_eq!(s.help.as_deref(), Some("Vos informations"));
                match &s.children[0] {
                    TreeElement::Note(n) => {
                        assert_eq!(n.title.as_deref(), Some("Attention"));
                        assert_eq!(n.body, "Pensez au SIRET.");
                    }
                    _ => panic!(),
                }
            }
            _ => panic!(),
        }
        // Clearing the clearables.
        update_section(
            &mut t,
            &NodeId::new("s1"),
            SectionPatch {
                help: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        match &t.elements[2] {
            TreeElement::Section(s) => assert_eq!(s.help, None),
            _ => panic!(),
        }
        update_group(
            &mut t,
            &GroupId::new("g2"),
            GroupPatch {
                cardinality: Some(Cardinality::Many),
                ..Default::default()
            },
        )
        .unwrap_err(); // many inside many: depth policy
        assert_eq!(
            update_column(&mut t, &ColumnId::new("g1"), ColumnPatch::default()),
            Err(EditError::UnknownElement(cid("g1"))),
            "kinds are separate namespaces"
        );
        assert_eq!(
            update_note(&mut t, &NodeId::new("s1"), NotePatch::default()),
            Err(EditError::UnknownElement(nid("s1"))),
            "a section is not a note, even under one id namespace"
        );
    }

    #[test]
    fn audience_clamps_on_add_and_refuses_widening() {
        let mut t = fixture();
        update_section(
            &mut t,
            &NodeId::new("s1"),
            SectionPatch {
                audience: Some(Audience::Reviewer),
                ..Default::default()
            },
        )
        .unwrap();
        // Adding inside the reviewer-only section clamps: nothing to
        // refuse, the element is reviewer-only.
        add_element(
            &mut t,
            &Placement::in_section(NodeId::new("s1")),
            column("e"),
        )
        .unwrap();
        match children_of(&t, &sid("s1")).last().unwrap() {
            TreeElement::Column(c) => assert_eq!(c.audience, Audience::Reviewer),
            _ => panic!(),
        }
        // Explicitly widening an element beyond its parent is the
        // refused contradiction (P.4).
        assert_eq!(
            update_column(
                &mut t,
                &ColumnId::new("e"),
                ColumnPatch {
                    audience: Some(Audience::All),
                    ..Default::default()
                }
            ),
            Err(EditError::AudienceWiderThanParent(cid("e")))
        );
        // Narrowing anywhere is fine; widening back at the root too.
        update_column(
            &mut t,
            &ColumnId::new("a"),
            ColumnPatch {
                audience: Some(Audience::Reviewer),
                ..Default::default()
            },
        )
        .unwrap();
        update_column(
            &mut t,
            &ColumnId::new("a"),
            ColumnPatch {
                audience: Some(Audience::All),
                ..Default::default()
            },
        )
        .unwrap();
        // Moving a public element into the reviewer-only section is
        // allowed — audience is inherited, markers untouched.
        move_element(&mut t, &cid("a"), &Placement::in_section(NodeId::new("s1"))).unwrap();
        match children_of(&t, &sid("s1")).last().unwrap() {
            TreeElement::Column(c) => assert_eq!(c.audience, Audience::All),
            _ => panic!(),
        }
    }

    #[test]
    fn move_across_levels_and_reorder() {
        let mut t = fixture();
        move_element(&mut t, &cid("c"), &Placement::root().before(cid("a"))).unwrap();
        assert_eq!(ids(&t.elements), ["c", "a", "g1", "s1"]);
        move_element(
            &mut t,
            &gid("g2"),
            &Placement::in_section(NodeId::new("s1")),
        )
        .unwrap();
        assert_eq!(ids(children_of(&t, &sid("s1"))), ["n1", "d", "g2"]);
        move_element(&mut t, &nid("n1"), &Placement::root()).unwrap();
        assert_eq!(ids(&t.elements), ["c", "a", "g1", "s1", "n1"]);
        // Reorder within the same parent, in both directions.
        move_element(&mut t, &cid("a"), &Placement::root().before(cid("c"))).unwrap();
        assert_eq!(ids(&t.elements), ["a", "c", "g1", "s1", "n1"]);
        // An element cannot anchor before itself.
        assert_eq!(
            move_element(&mut t, &cid("a"), &Placement::root().before(cid("a"))),
            Err(EditError::AnchorNotInParent {
                parent: Parent::Root,
                anchor: cid("a")
            })
        );
        // A section cannot move into its own subtree.
        let before = t.clone();
        assert_eq!(
            move_element(&mut t, &sid("s1"), &Placement::in_group(GroupId::new("g2"))),
            Err(EditError::UnknownParent(Parent::Group(GroupId::new("g2"))))
        );
        assert_eq!(t, before);
    }

    #[test]
    fn remove_returns_the_subtree() {
        let mut t = fixture();
        let removed = remove_element(&mut t, &sid("s1")).unwrap();
        assert_eq!(ids(&t.elements), ["a", "g1"]);
        match removed {
            TreeElement::Section(s) => assert_eq!(ids(&s.children), ["n1", "d"]),
            _ => panic!(),
        }
        assert_eq!(
            remove_element(&mut t, &nid("n1")),
            Err(EditError::UnknownElement(nid("n1")))
        );
    }

    #[test]
    fn arity_many_is_offered_on_list_capable_types_only() {
        let mut t = fixture();
        let many_text = TreeElement::Column(TreeColumn {
            id: ColumnId::new("z"),
            label: "Z".into(),
            ty: ScalarType::Text,
            arity: Arity::Many,
            format: None,
            required: true,
            audience: Audience::All,
        });
        assert_eq!(
            add_element(&mut t, &Placement::root(), many_text),
            Err(EditError::ArityNotOffered(ColumnId::new("z")))
        );
        let many_files = TreeElement::Column(TreeColumn {
            id: ColumnId::new("z"),
            label: "Z".into(),
            ty: ScalarType::Attachment(Default::default()),
            arity: Arity::Many,
            format: None,
            required: true,
            audience: Audience::All,
        });
        add_element(&mut t, &Placement::root(), many_files).unwrap();
        assert_eq!(
            update_column(
                &mut t,
                &ColumnId::new("a"),
                ColumnPatch {
                    arity: Some(Arity::Many),
                    ..Default::default()
                }
            ),
            Err(EditError::ArityNotOffered(ColumnId::new("a")))
        );
        // A type change away from a list-capable type takes the arity
        // back to one.
        update_column(
            &mut t,
            &ColumnId::new("z"),
            ColumnPatch {
                ty: Some(ScalarType::Text),
                ..Default::default()
            },
        )
        .unwrap();
        match t.elements.last().unwrap() {
            TreeElement::Column(c) => assert_eq!(c.arity, Arity::One),
            _ => panic!(),
        }
    }

    #[test]
    fn format_is_offered_on_text_only_and_patterns_verify() {
        let mut t = fixture();
        update_column(
            &mut t,
            &ColumnId::new("a"),
            ColumnPatch {
                format: Some(Some(Format::Email)),
                ..Default::default()
            },
        )
        .unwrap();
        // A bad pattern is refused now, the draft untouched.
        let before = t.clone();
        assert!(matches!(
            update_column(
                &mut t,
                &ColumnId::new("a"),
                ColumnPatch {
                    format: Some(Some(Format::Regex("(?=x)".into()))),
                    ..Default::default()
                }
            ),
            Err(EditError::InvalidPattern(c, _)) if c == ColumnId::new("a")
        ));
        assert_eq!(t, before);
        // Format on a non-text column is refused…
        assert_eq!(
            update_column(
                &mut t,
                &ColumnId::new("a"),
                ColumnPatch {
                    ty: Some(ScalarType::Integer(None)),
                    format: Some(Some(Format::Phone)),
                    ..Default::default()
                }
            ),
            Err(EditError::FormatNotOffered(ColumnId::new("a")))
        );
        // …and a type change away from text resets it silently, the
        // arity precedent.
        update_column(
            &mut t,
            &ColumnId::new("a"),
            ColumnPatch {
                ty: Some(ScalarType::Integer(None)),
                ..Default::default()
            },
        )
        .unwrap();
        match &t.elements[0] {
            TreeElement::Column(c) => assert_eq!(c.format, None),
            _ => panic!(),
        }
        // Adding checks the same backstops.
        let mut fresh = TreeColumn {
            id: ColumnId::new("z"),
            label: "Z".into(),
            ty: ScalarType::Boolean,
            arity: Arity::One,
            required: true,
            format: Some(Format::Iban),
            audience: Audience::All,
        };
        assert_eq!(
            add_element(
                &mut t,
                &Placement::root(),
                TreeElement::Column(fresh.clone())
            ),
            Err(EditError::FormatNotOffered(ColumnId::new("z")))
        );
        fresh.ty = ScalarType::Text;
        add_element(&mut t, &Placement::root(), TreeElement::Column(fresh)).unwrap();
    }

    #[test]
    fn fresh_ids_are_distinct() {
        assert_ne!(new_column_id(), new_column_id());
        assert_ne!(new_group_id(), new_group_id());
        assert_ne!(new_node_id(), new_node_id());
    }
}
