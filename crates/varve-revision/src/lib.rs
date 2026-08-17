//! Tier 3 (§7): the revision DAG, publication, nomenclature
//! publication (with the §2.11 append-only rule the cast table leans
//! on), block publication (§2.1), three-way schema merge, and aggregate
//! revision construction (§5.5).

#![forbid(unsafe_code)]

mod aggregate;
mod blocks;
mod merge;
mod nomenclatures;

pub use aggregate::{
    AggregateColumn, AggregatePolicy, AggregateReport, AggregateRevision, aggregate,
};
pub use merge::{MergeConflict, merge};
pub use blocks::{BlockRegistry, PublishBlockError};
pub use nomenclatures::{NomenclatureRegistry, PublishNomenclatureError};

use std::collections::BTreeMap;

use varve_core::RevisionId;
use varve_schema::{Schema, revision_id};

/// The revision DAG of one schema lineage (§2.1): **objects** —
/// immutable, content-addressed revisions — plus a **publication log**
/// of events. The object is identity; the log is history: publishing a
/// schema whose object already exists (a revert to an earlier revision)
/// adds no object but records the event and moves `latest`. Records are
/// never "on" a revision (§2.9) — entries are authored against one, and
/// lookups here serve reading lenses, projection, and impact.
#[derive(Debug, Clone, Default)]
pub struct RevisionDag {
    revisions: BTreeMap<RevisionId, PublishedRevision>,
    /// Publication events, oldest first — the aggregate's input order.
    /// An id may recur (revert).
    log: Vec<Publication>,
}

#[derive(Debug, Clone)]
pub struct PublishedRevision {
    pub schema: Schema,
    /// Parents at first publication — the object's place in the DAG.
    pub parents: Vec<RevisionId>,
}

/// One publication event: which object became current, following which
/// revisions. A revert is an event whose object predates its parents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub revision: RevisionId,
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
    /// Publishing a schema whose revision already exists creates no new
    /// object but is still an event: it becomes `latest` again (a
    /// revert), with the parents given here recorded on the event.
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
        self.revisions
            .entry(id.clone())
            .or_insert_with(|| PublishedRevision { schema, parents: parents.clone() });
        self.log.push(Publication { revision: id.clone(), parents });
        Ok(id)
    }

    pub fn get(&self, id: &RevisionId) -> Option<&PublishedRevision> {
        self.revisions.get(id)
    }

    /// The current revision: the last one published (which may be an
    /// earlier object, after a revert).
    pub fn latest(&self) -> Option<&RevisionId> {
        self.log.last().map(|p| &p.revision)
    }

    /// The publication events, oldest first.
    pub fn publications(&self) -> &[Publication] {
        &self.log
    }

    /// Oldest-first publication history — the §5.5 aggregate input. An
    /// id recurs when it was re-published; the aggregate is over
    /// objects, so callers that want distinct revisions dedup by id.
    pub fn history(&self) -> impl Iterator<Item = (&RevisionId, &Schema)> {
        self.log
            .iter()
            .map(|p| (&p.revision, &self.revisions[&p.revision].schema))
    }
}
