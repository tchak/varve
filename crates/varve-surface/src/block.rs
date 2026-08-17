//! Published blocks (§2.1, Q5): a reusable group definition with its own
//! identity and version, referenced by inclusion. Two-sided by design —
//! a **schema shell** (the group + paired resolver declarations, what a
//! revision includes) and **surface defaults** (rules, prompts, formats,
//! write policy — what a surface ships). This crate is the one place
//! that sees both halves, so the assembled block lives here.
//!
//! Inclusion *pastes*: the shell becomes an ordinary group in the
//! schema and the defaults an ordinary group node in the surface, so
//! projection, impact, logic and admissibility never learn about blocks
//! — the same principle as inline nomenclatures (§2.12). Rules pin to
//! the block version: that is what "published" means.

use std::collections::BTreeMap;

use varve_core::canonical::{CanonicalValue, hash_plain};
use varve_core::{BlockId, ColumnId, GroupId};
use varve_logic::{sources, to_canonical};
use varve_schema::{
    Element, Group, NomenclatureTable, ResolverDeclaration, Schema, SchemaIndex,
    schema_canonical,
};

use crate::{ColumnNode, GroupNode, Node, Surface, SurfaceError, validate};

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub version: u32,
    /// Schema side: the group as it will appear in an including
    /// revision — its id is the group id every inclusion uses.
    pub shell: Group,
    /// Schema side: declarations paired with the block (§2.7 SIRET
    /// pattern). Their inputs and targets must be block columns.
    pub resolvers: Vec<ResolverDeclaration>,
    /// Surface side: the group node a surface includes — prompts,
    /// rules, formats, write policies over the block's own columns.
    pub defaults: GroupNode,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BlockError {
    #[error("block defaults name group '{0}', shell is group '{1}'")]
    GroupMismatch(GroupId, GroupId),
    /// Every default node must refer to a column of the shell — a block
    /// is self-contained.
    #[error("block defaults reference '{0}', which is not a block column")]
    ForeignColumn(ColumnId),
    /// A block rule must read only block columns: it must mean the same
    /// thing wherever the block is included.
    #[error("block rule on '{0}' reads '{1}', which is not a block column")]
    ForeignRuleSource(ColumnId, ColumnId),
    #[error("resolver '{0}' reads or writes a column outside the block")]
    ForeignResolverColumn(varve_core::ResolverId),
    #[error(transparent)]
    Surface(SurfaceError),
    #[error("schema shell: {0}")]
    Shell(varve_schema::SchemaError),
}

impl Block {
    /// The block's content address (plain regime, §2.13): both halves
    /// are identity-bearing — a changed default rule is a new version.
    pub fn content_id(&self) -> BlockId {
        let shell = Schema {
            root: vec![Element::Group(self.shell.clone())],
            resolvers: self.resolvers.clone(),
        };
        let defaults = defaults_canonical(&self.defaults);
        let value = CanonicalValue::Object(
            [
                ("shell".to_string(), schema_canonical(&shell)),
                ("defaults".to_string(), defaults),
                ("version".to_string(), CanonicalValue::Int(i64::from(self.version))),
            ]
            .into_iter()
            .collect(),
        );
        BlockId::new(hash_plain(&value).expect("blocks carry no floats").to_string())
    }

    /// The columns the block owns (any depth within the shell).
    pub fn columns(&self) -> Vec<ColumnId> {
        fn walk(elements: &[Element], out: &mut Vec<ColumnId>) {
            for el in elements {
                match el {
                    Element::Column(c) => out.push(c.id.clone()),
                    Element::Group(g) => walk(&g.children, out),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.shell.children, &mut out);
        out
    }

    /// Self-containment and internal consistency: the two halves agree,
    /// defaults and rules stay inside the block, resolvers stay inside
    /// the block, and everything typechecks at the block's own scope.
    pub fn validate(&self, nomenclatures: &NomenclatureTable) -> Vec<BlockError> {
        let mut errors = Vec::new();
        if self.defaults.group != self.shell.id {
            errors.push(BlockError::GroupMismatch(
                self.defaults.group.clone(),
                self.shell.id.clone(),
            ));
        }
        let owned: std::collections::BTreeSet<ColumnId> =
            self.columns().into_iter().collect();

        // Defaults reference only block columns; rules read only block
        // columns.
        for entry in column_nodes(&self.defaults) {
            if !owned.contains(&entry.column) {
                errors.push(BlockError::ForeignColumn(entry.column.clone()));
            }
            for rule in [&entry.visibility, &entry.required].into_iter().flatten() {
                for source in sources(rule) {
                    if !owned.contains(&source) {
                        errors.push(BlockError::ForeignRuleSource(
                            entry.column.clone(),
                            source,
                        ));
                    }
                }
            }
        }
        for decl in &self.resolvers {
            let inside = decl.input.iter().all(|(c, _)| owned.contains(c))
                && decl.mapping.iter().all(|m| owned.contains(&m.target));
            if !inside {
                errors.push(BlockError::ForeignResolverColumn(decl.id.clone()));
            }
        }

        // The assembled pair validates as a schema + surface would.
        let schema = Schema {
            root: vec![Element::Group(self.shell.clone())],
            resolvers: self.resolvers.clone(),
        };
        for e in varve_schema::validate(&schema, varve_schema::DepthPolicy::default()) {
            errors.push(BlockError::Shell(e));
        }
        let surface = Surface {
            id: varve_core::SurfaceId::new("block-defaults"),
            revision: varve_core::RevisionId::new("block"),
            nodes: vec![Node::Group(self.defaults.clone())],
            ineligibility: None,
        };
        for e in validate(&surface, &schema, nomenclatures) {
            errors.push(BlockError::Surface(e));
        }
        errors
    }

    /// Include the block: paste the shell into `schema` and the defaults
    /// into `surface`, both at the given container (root or a group).
    /// After inclusion nothing downstream knows a block was involved.
    pub fn include(
        &self,
        schema: &mut Schema,
        surface: &mut Surface,
        container: Option<&GroupId>,
    ) -> Result<(), IncludeError> {
        let element = Element::Group(self.shell.clone());
        match container {
            None => schema.root.push(element),
            Some(group) => {
                if !push_into_group(&mut schema.root, group, element) {
                    return Err(IncludeError::UnknownContainer(group.clone()));
                }
            }
        }
        schema.resolvers.extend(self.resolvers.iter().cloned());
        let node = Node::Group(self.defaults.clone());
        match container {
            None => surface.nodes.push(node),
            Some(group) => {
                if !push_into_node_group(&mut surface.nodes, group, node) {
                    return Err(IncludeError::UnknownContainer(group.clone()));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IncludeError {
    #[error("no group '{0}' to include into")]
    UnknownContainer(GroupId),
}

fn push_into_group(elements: &mut [Element], group: &GroupId, element: Element) -> bool {
    for el in elements.iter_mut() {
        if let Element::Group(g) = el {
            if g.id == *group {
                g.children.push(element);
                return true;
            }
            if push_into_group(&mut g.children, group, element.clone()) {
                return true;
            }
        }
    }
    false
}

fn push_into_node_group(nodes: &mut [Node], group: &GroupId, node: Node) -> bool {
    for n in nodes.iter_mut() {
        match n {
            Node::Group(g) => {
                if g.group == *group {
                    g.children.push(node);
                    return true;
                }
                if push_into_node_group(&mut g.children, group, node.clone()) {
                    return true;
                }
            }
            Node::Section(s) => {
                if push_into_node_group(&mut s.children, group, node.clone()) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn column_nodes(group: &GroupNode) -> Vec<&ColumnNode> {
    fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a ColumnNode>) {
        for node in nodes {
            match node {
                Node::Column(c) => out.push(c),
                Node::Group(g) => walk(&g.children, out),
                Node::Section(s) => walk(&s.children, out),
                Node::Note(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&group.children, &mut out);
    out
}

/// Canonical form of the surface defaults — identity-bearing content
/// (a changed rule or prompt is a new block version).
fn defaults_canonical(group: &GroupNode) -> CanonicalValue {
    fn node(n: &Node) -> CanonicalValue {
        let obj = |pairs: Vec<(&str, CanonicalValue)>| {
            CanonicalValue::Object(
                pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<BTreeMap<_, _>>(),
            )
        };
        let s = |v: &str| CanonicalValue::String(v.to_string());
        let opt = |v: &Option<String>| match v {
            None => CanonicalValue::Null,
            Some(t) => s(t),
        };
        let rule = |r: &Option<varve_logic::Expr>| match r {
            None => CanonicalValue::Null,
            Some(e) => to_canonical(e),
        };
        match n {
            Node::Column(c) => obj(vec![
                ("column", s(c.column.as_str())),
                ("prompt", opt(&c.prompt)),
                ("help", opt(&c.help)),
                ("visibility", rule(&c.visibility)),
                ("required", rule(&c.required)),
                ("writable", CanonicalValue::Bool(c.write.writable)),
                ("override_derived", CanonicalValue::Bool(c.write.override_derived)),
                (
                    "format",
                    match &c.format {
                        None => CanonicalValue::Null,
                        Some(f) => s(&format!("{f:?}")),
                    },
                ),
            ]),
            Node::Group(g) => obj(vec![
                ("group", s(g.group.as_str())),
                ("prompt", opt(&g.prompt)),
                ("visibility", rule(&g.visibility)),
                ("children", CanonicalValue::Array(g.children.iter().map(node).collect())),
            ]),
            Node::Section(sec) => obj(vec![
                ("section", s(&sec.title)),
                ("help", opt(&sec.help)),
                ("visibility", rule(&sec.visibility)),
                ("children", CanonicalValue::Array(sec.children.iter().map(node).collect())),
            ]),
            Node::Note(note) => obj(vec![("note", s(&note.body)), ("title", opt(&note.title))]),
        }
    }
    node(&Node::Group(group.clone()))
}

/// The columns of a schema that came from block shells are just
/// columns; this helper is for tooling that wants to know which — a
/// block registry maps `SchemaIndex` groups back to block ids.
pub fn included_blocks<'a>(
    schema: &Schema,
    registry: &'a [Block],
) -> Vec<(&'a Block, GroupId)> {
    let index = SchemaIndex::build(schema);
    registry
        .iter()
        .filter(|b| index.groups.contains_key(&b.shell.id))
        .map(|b| (b, b.shell.id.clone()))
        .collect()
}
