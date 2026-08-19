//! The **surface-side** half of a published block (§2.1, Q5): the
//! defaults a block ships for the surfaces that present it — prompts,
//! visibility and requiredness rules, formats, write policies over its
//! own columns. It references the schema-side `varve_schema::Block` by
//! `(id, version)`; publishing block version N means publishing shell N
//! and defaults N together, which is a platform act — the kernel gives
//! the two objects and the pin.
//!
//! Inclusion pastes: the defaults become an ordinary group node of the
//! including surface, so admissibility and reachability never learn
//! about blocks — the same principle as the schema-side paste
//! (`Block::include_into`, which records provenance on the group).
//! Defaults travel as `block_defaults` wire lines whose body this crate
//! encodes (`canon`, §5).

use varve_core::{ColumnId, GroupId};
use varve_logic::sources;
use varve_schema::{Block, BlockRef, Element, NomenclatureTable, Schema};

use crate::{ColumnNode, GroupNode, Node, Surface, SurfaceError, validate};

#[derive(Debug, Clone, PartialEq)]
pub struct BlockDefaults {
    /// The schema-side block these defaults belong to.
    pub block: BlockRef,
    /// The group node a surface includes — prompts, rules, formats,
    /// write policies over the block's own columns.
    pub node: GroupNode,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BlockDefaultsError {
    #[error("defaults are for block '{0}' v{1}, given block '{2}' v{3}")]
    WrongBlock(varve_core::BlockId, u32, varve_core::BlockId, u32),
    #[error("defaults name group '{0}', the block's shell is group '{1}'")]
    GroupMismatch(GroupId, GroupId),
    /// Every default node must refer to a column of the shell — a block
    /// is self-contained.
    #[error("defaults reference '{0}', which is not a block column")]
    ForeignColumn(ColumnId),
    /// A block rule must read only block columns: it must mean the same
    /// thing wherever the block is included.
    #[error("rule on '{0}' reads '{1}', which is not a block column")]
    ForeignRuleSource(ColumnId, ColumnId),
    #[error(transparent)]
    Surface(SurfaceError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IncludeError {
    #[error("no group '{0}' to include into")]
    UnknownContainer(GroupId),
    #[error("the surface already presents group '{0}'")]
    DuplicateGroup(GroupId),
}

impl BlockDefaults {
    /// Self-containment against the block they belong to: same block and
    /// version, same group id, only block columns referenced, rules read
    /// only block columns, and the pair typechecks as a surface over the
    /// shell would.
    pub fn validate(
        &self,
        block: &Block,
        nomenclatures: &NomenclatureTable,
    ) -> Vec<BlockDefaultsError> {
        let mut errors = Vec::new();
        if self.block != block.reference() {
            errors.push(BlockDefaultsError::WrongBlock(
                self.block.id.clone(),
                self.block.version,
                block.id.clone(),
                block.version,
            ));
        }
        if self.node.group != block.group.id {
            errors.push(BlockDefaultsError::GroupMismatch(
                self.node.group.clone(),
                block.group.id.clone(),
            ));
        }
        let owned: std::collections::BTreeSet<ColumnId> = block.columns().into_iter().collect();
        for entry in column_nodes(&self.node) {
            if !owned.contains(&entry.column) {
                errors.push(BlockDefaultsError::ForeignColumn(entry.column.clone()));
            }
            for rule in [&entry.visibility, &entry.required].into_iter().flatten() {
                for source in sources(rule) {
                    if !owned.contains(&source) {
                        errors.push(BlockDefaultsError::ForeignRuleSource(
                            entry.column.clone(),
                            source,
                        ));
                    }
                }
            }
        }
        let schema = Schema {
            root: vec![Element::Group(block.group.clone())],
            resolvers: block.resolvers.clone(),
        };
        let surface = Surface {
            id: varve_core::SurfaceId::new("block-defaults"),
            revision: varve_schema::revision_id(&schema),
            nodes: vec![Node::Group(self.node.clone())],
            ineligibility: None,
        };
        errors.extend(
            validate(&surface, &schema, nomenclatures)
                .into_iter()
                .map(BlockDefaultsError::Surface),
        );
        errors
    }

    /// Include the defaults into `surface` at `container` (root, or a
    /// group node): paste the group node. Checked before anything is
    /// touched — an error leaves `surface` unchanged. The schema-side
    /// half is included separately (`Block::include_into`).
    pub fn include_into(
        &self,
        surface: &mut Surface,
        container: Option<&GroupId>,
    ) -> Result<(), IncludeError> {
        if let Some(c) = container
            && !has_group_node(&surface.nodes, c)
        {
            return Err(IncludeError::UnknownContainer(c.clone()));
        }
        if has_group_node(&surface.nodes, &self.node.group) {
            return Err(IncludeError::DuplicateGroup(self.node.group.clone()));
        }
        let node = Node::Group(self.node.clone());
        match container {
            None => surface.nodes.push(node),
            Some(group) => {
                let pushed = push_into_node_group(&mut surface.nodes, group, node);
                debug_assert!(pushed, "container existence checked above");
            }
        }
        Ok(())
    }
}

fn has_group_node(nodes: &[Node], group: &GroupId) -> bool {
    nodes.iter().any(|n| match n {
        Node::Group(g) => g.group == *group || has_group_node(&g.children, group),
        Node::Section(s) => has_group_node(&s.children, group),
        _ => false,
    })
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
