//! Rehydration: stored events back into kernel objects, **with the
//! kernel's own checks re-run**. The store traits enforce only index
//! rules; everything content-shaped — chain linkage and commitments
//! (§2.13), content-addressed revision ids, block validation and
//! version numbering, the §2.11 append-only rule — re-runs here by
//! replaying events through the kernel constructors, exactly as the
//! wire reader re-verifies on import (§5). A row a backend lost or
//! altered surfaces as a typed error at the first load.

use varve_core::RecordId;
use varve_record::{ChainError, RecordLog};
use varve_revision::{
    BlockRegistry, NomenclatureRegistry, PublishBlockError, PublishError, PublishNomenclatureError,
    RevisionDag,
};
use varve_schema::DepthPolicy;

use crate::{BlockStore, LineageId, NomenclatureStore, RecordLogStore, RevisionStore, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The stored entries do not verify as a chain: storage-level
    /// tamper or loss (§2.13).
    #[error("record '{record}': {error}")]
    Chain { record: RecordId, error: ChainError },
    /// Replaying a publication event failed — the store holds an event
    /// the DAG refuses (e.g. an unknown parent).
    #[error("lineage '{lineage}', publication #{index}: {error}")]
    Publication {
        lineage: LineageId,
        index: usize,
        error: PublishError,
    },
    /// A stored schema no longer hashes to the revision id its
    /// publication event names: the object was altered at rest. The
    /// content address is the check (§2.13).
    #[error(
        "lineage '{lineage}', publication #{index}: stored schema hashes to a different revision id"
    )]
    RevisionIdMismatch { lineage: LineageId, index: usize },
    #[error("block replay: {0}")]
    Block(#[from] PublishBlockError),
    #[error("nomenclature replay: {0}")]
    Nomenclature(#[from] PublishNomenclatureError),
}

/// Load and verify a record's log. The one sanctioned way from stored
/// entries to a [`RecordLog`]: `from_entries` is documented
/// unvalidated, so the chain check is not optional here.
pub async fn load_log(
    store: &impl RecordLogStore,
    record: &RecordId,
) -> Result<RecordLog, LoadError> {
    let entries = store.entries(record, 0).await?;
    let log = RecordLog::from_entries(record.clone(), entries);
    log.verify_chain().map_err(|error| LoadError::Chain {
        record: record.clone(),
        error,
    })?;
    Ok(log)
}

/// Load a lineage's revision DAG by replaying its publication events.
/// Each replayed publication recomputes the schema's content address;
/// a mismatch with the stored event is tamper, not a variant reading.
pub async fn load_dag(
    store: &impl RevisionStore,
    lineage: &LineageId,
) -> Result<RevisionDag, LoadError> {
    let mut dag = RevisionDag::new();
    for (index, (publication, schema)) in store.publications(lineage).await?.into_iter().enumerate()
    {
        let id = dag
            .publish(schema, publication.parents.clone())
            .map_err(|error| LoadError::Publication {
                lineage: lineage.clone(),
                index,
                error,
            })?;
        if id != publication.revision {
            return Err(LoadError::RevisionIdMismatch {
                lineage: lineage.clone(),
                index,
            });
        }
    }
    Ok(dag)
}

/// Load the block registry by replaying every stored version through
/// [`BlockRegistry::publish`] — content validation and version
/// numbering re-run under `policy` (the instance's §2.3 depth policy,
/// the same one publication used).
pub async fn load_blocks(
    store: &impl BlockStore,
    policy: DepthPolicy,
) -> Result<BlockRegistry, LoadError> {
    let mut registry = BlockRegistry::new();
    for block in store.blocks().await? {
        registry.publish(block, policy)?;
    }
    Ok(registry)
}

/// Load the nomenclature registry by replay; the §2.11 append-only
/// rule re-runs on every version step.
pub async fn load_nomenclatures(
    store: &impl NomenclatureStore,
) -> Result<NomenclatureRegistry, LoadError> {
    let mut registry = NomenclatureRegistry::new();
    for (id, _version, rows) in store.nomenclatures().await? {
        registry.publish(id, rows)?;
    }
    Ok(registry)
}
