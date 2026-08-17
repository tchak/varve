//! Tier 3 (§7): the revision DAG, publication, nomenclature
//! publication (with the §2.11 append-only rule the cast table leans
//! on), three-way schema merge, and aggregate revision construction
//! (§5.5).

#![forbid(unsafe_code)]

mod aggregate;
mod merge;
mod nomenclatures;

pub use aggregate::{
    AggregateColumn, AggregatePolicy, AggregateReport, AggregateRevision, aggregate,
};
pub use merge::{ConflictKind, MergeConflict, merge};
pub use nomenclatures::{NomenclatureRegistry, PublishNomenclatureError};

use std::collections::BTreeMap;

use varve_core::RevisionId;
use varve_schema::{Schema, revision_id};

/// The revision DAG of one schema lineage: published, immutable,
/// content-addressed revisions (§2.1). Records are never "on" a
/// revision (§2.9) — entries are authored against one, and lookups here
/// serve reading lenses, projection, and impact.
#[derive(Debug, Clone, Default)]
pub struct RevisionDag {
    revisions: BTreeMap<RevisionId, PublishedRevision>,
    /// Publication order, oldest first — the aggregate's input order.
    order: Vec<RevisionId>,
}

#[derive(Debug, Clone)]
pub struct PublishedRevision {
    pub schema: Schema,
    pub parents: Vec<RevisionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublishError {
    #[error("unknown parent revision '{0}'")]
    UnknownParent(RevisionId),
}

impl RevisionDag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a schema: its id is its canonical hash — identical
    /// schemas converge on identical ids on every instance (§2.13).
    /// Publishing the same schema twice is a no-op returning the same
    /// id.
    pub fn publish(
        &mut self,
        schema: Schema,
        parents: Vec<RevisionId>,
    ) -> Result<RevisionId, PublishError> {
        for parent in &parents {
            if !self.revisions.contains_key(parent) {
                return Err(PublishError::UnknownParent(parent.clone()));
            }
        }
        let id = revision_id(&schema);
        if !self.revisions.contains_key(&id) {
            self.order.push(id.clone());
            self.revisions
                .insert(id.clone(), PublishedRevision { schema, parents });
        }
        Ok(id)
    }

    pub fn get(&self, id: &RevisionId) -> Option<&PublishedRevision> {
        self.revisions.get(id)
    }

    pub fn latest(&self) -> Option<&RevisionId> {
        self.order.last()
    }

    /// Oldest-first publication history — the §5.5 aggregate input.
    pub fn history(&self) -> impl Iterator<Item = (&RevisionId, &Schema)> {
        self.order
            .iter()
            .map(|id| (id, &self.revisions[id].schema))
    }
}
