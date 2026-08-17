//! Block publication (§2.1, Q5): the schema-side half of a block —
//! shell + paired declarations — published with an identity and a
//! version, like a nomenclature. The surface-side defaults are
//! published beside it by the platform (`varve-surface::BlockDefaults`
//! references the version); the kernel gives the objects and the pin.

use std::collections::BTreeMap;

use varve_core::BlockId;
use varve_schema::{Block, BlockError, DepthPolicy};

#[derive(Debug, Clone, Default)]
pub struct BlockRegistry {
    /// Versions per block, version 1 first.
    versions: BTreeMap<BlockId, Vec<Block>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublishBlockError {
    #[error("block '{id}': {errors:?}")]
    Invalid { id: BlockId, errors: Vec<BlockError> },
    /// The registry numbers versions; a block declares the version it
    /// expects so a stale author fails loudly.
    #[error("block '{id}': expected version {expected}, next is {next}")]
    VersionMismatch { id: BlockId, expected: u32, next: u32 },
    /// Every version of a block keeps the shell's group id: that id is
    /// what every inclusion uses.
    #[error("block '{id}': shell group id changed between versions")]
    ShellIdChanged { id: BlockId },
}

impl BlockRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish the next version of `block.id`; `block.version` must be
    /// that next number. Returns the version.
    pub fn publish(&mut self, block: Block, policy: DepthPolicy) -> Result<u32, PublishBlockError> {
        let errors = block.validate(policy);
        if !errors.is_empty() {
            return Err(PublishBlockError::Invalid { id: block.id.clone(), errors });
        }
        let versions = self.versions.entry(block.id.clone()).or_default();
        let next = versions.len() as u32 + 1;
        if block.version != next {
            return Err(PublishBlockError::VersionMismatch {
                id: block.id.clone(),
                expected: block.version,
                next,
            });
        }
        if let Some(previous) = versions.last()
            && previous.group.id != block.group.id
        {
            return Err(PublishBlockError::ShellIdChanged { id: block.id.clone() });
        }
        versions.push(block);
        Ok(next)
    }

    pub fn get(&self, id: &BlockId, version: u32) -> Option<&Block> {
        self.versions.get(id)?.get(version.checked_sub(1)? as usize)
    }

    pub fn latest(&self, id: &BlockId) -> Option<&Block> {
        self.versions.get(id)?.last()
    }

    /// Every version of every block, for the wire.
    pub fn all(&self) -> impl Iterator<Item = &Block> {
        self.versions.values().flatten()
    }
}
