//! Published blocks (§2.1, Q5): a reusable group definition with its
//! own identity and version, referenced by inclusion. This is the
//! **schema-side** half — the shell (a group) and its paired resolver
//! declarations (§2.7) — hashed plain like a nomenclature (§2.13) and
//! carried on the wire as a `block` line. The surface-side half (rule,
//! prompt, format and write-policy defaults) is `varve-surface`'s
//! `BlockDefaults`, which references a block by `(id, version)`.
//!
//! Inclusion **pastes with provenance**: the shell becomes an ordinary
//! group of the including schema, carrying `included_from = (id,
//! version)`. Everything downstream — projection, logic, conformance,
//! impact — keeps seeing plain groups; the revision knows what it
//! included, so rules can pin to a block version and the impact report
//! can name a block bump.

use std::collections::HashSet;

use varve_core::canonical::ContentHash;
use varve_core::{BlockId, ColumnId, GroupId, ResolverId};

use crate::canon::block_hash;
use crate::{
    BlockRef, DepthPolicy, Element, Group, ResolverDeclaration, Schema, SchemaError, SchemaIndex,
    validate,
};

/// The schema-side half of a published block.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub version: u32,
    /// The shell: the group as it will appear in every including
    /// revision — its id is the group id every inclusion uses.
    pub group: Group,
    /// Declarations paired with the block (§2.7 SIRET pattern). Their
    /// inputs and targets must be block columns.
    pub resolvers: Vec<ResolverDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlockError {
    /// The shell, standing alone as a schema, does not validate.
    #[error("block shell: {0}")]
    Shell(SchemaError),
    /// A block's shell is not itself included from another block.
    #[error("block shell carries inclusion provenance")]
    ShellHasProvenance,
    /// A block must mean the same thing wherever it is included: its
    /// resolvers read and write block columns only.
    #[error("resolver '{0}' reads or writes a column outside the block")]
    ForeignResolverColumn(ResolverId),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IncludeError {
    #[error("no group '{0}' to include into")]
    UnknownContainer(GroupId),
    #[error("the schema already has a group '{0}'")]
    DuplicateGroup(GroupId),
    #[error("the schema already has a column '{0}'")]
    DuplicateColumn(ColumnId),
    #[error("the schema already declares resolver '{0}'")]
    DuplicateResolver(ResolverId),
}

impl Block {
    pub fn reference(&self) -> BlockRef {
        BlockRef {
            id: self.id.clone(),
            version: self.version,
        }
    }

    /// The block's content address (plain regime, §2.13).
    pub fn content_hash(&self) -> ContentHash {
        block_hash(self)
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
        walk(&self.group.children, &mut out);
        out
    }

    /// Publication-time checks: the shell validates as a schema of its
    /// own under `policy`, carries no provenance, and its resolvers stay
    /// inside the block.
    pub fn validate(&self, policy: DepthPolicy) -> Vec<BlockError> {
        let mut errors = Vec::new();
        if self.group.included_from.is_some() {
            errors.push(BlockError::ShellHasProvenance);
        }
        let standalone = Schema {
            root: vec![Element::Group(self.group.clone())],
            resolvers: self.resolvers.clone(),
        };
        errors.extend(
            validate(&standalone, policy)
                .into_iter()
                .map(BlockError::Shell),
        );
        let own: HashSet<ColumnId> = self.columns().into_iter().collect();
        for r in &self.resolvers {
            let inputs = r.input.iter().map(|(c, _)| c);
            let targets = r.mapping.iter().map(|m| &m.target);
            if inputs.chain(targets).any(|c| !own.contains(c)) {
                errors.push(BlockError::ForeignResolverColumn(r.id.clone()));
            }
        }
        errors
    }

    /// Include the block into `schema` at `container` (root, or a
    /// group): paste the shell with `included_from` set, append the
    /// paired declarations. Checked before anything is touched — an
    /// error leaves `schema` unchanged.
    pub fn include_into(
        &self,
        schema: &mut Schema,
        container: Option<&GroupId>,
    ) -> Result<(), IncludeError> {
        let index = SchemaIndex::build(schema);
        if let Some(c) = container
            && !index.groups.contains_key(c)
        {
            return Err(IncludeError::UnknownContainer(c.clone()));
        }
        if index.groups.contains_key(&self.group.id) {
            return Err(IncludeError::DuplicateGroup(self.group.id.clone()));
        }
        // Nested groups of the shell must not collide either.
        let mut nested = Vec::new();
        collect_groups(&self.group.children, &mut nested);
        if let Some(g) = nested.into_iter().find(|g| index.groups.contains_key(g)) {
            return Err(IncludeError::DuplicateGroup(g));
        }
        if let Some(c) = self
            .columns()
            .into_iter()
            .find(|c| index.columns.contains_key(c))
        {
            return Err(IncludeError::DuplicateColumn(c));
        }
        // A declaration's identity is (anchor, id) — §10 Q17: two SIRET
        // blocks both bring insee-sirene, anchored at their own groups.
        if let Some(r) = self.resolvers.iter().find(|r| {
            schema
                .resolvers
                .iter()
                .any(|s| s.id == r.id && s.anchor == r.anchor)
        }) {
            return Err(IncludeError::DuplicateResolver(r.id.clone()));
        }

        let mut shell = self.group.clone();
        shell.included_from = Some(self.reference());
        let element = Element::Group(shell);
        match container {
            None => schema.root.push(element),
            Some(group) => {
                let pushed = push_into_group(&mut schema.root, group, element);
                debug_assert!(pushed, "container existence checked above");
            }
        }
        schema.resolvers.extend(self.resolvers.iter().cloned());
        Ok(())
    }
}

fn collect_groups(elements: &[Element], out: &mut Vec<GroupId>) {
    for el in elements {
        if let Element::Group(g) = el {
            out.push(g.id.clone());
            collect_groups(&g.children, out);
        }
    }
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

/// The blocks a schema includes, by provenance: `(group id, block ref)`
/// for every group pasted from a block, in document order.
pub fn included_blocks(schema: &Schema) -> Vec<(GroupId, BlockRef)> {
    fn walk(elements: &[Element], out: &mut Vec<(GroupId, BlockRef)>) {
        for el in elements {
            if let Element::Group(g) = el {
                if let Some(b) = &g.included_from {
                    out.push((g.id.clone(), b.clone()));
                }
                walk(&g.children, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&schema.root, &mut out);
    out
}
