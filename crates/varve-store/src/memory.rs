//! The in-memory reference implementation: the trait contract made
//! executable, for tests (kernel and platform alike) and for
//! `varve-service` development before a production store exists. Not
//! durable, deliberately — it is a semantics oracle, the way
//! `object_store`'s in-memory backend serves `varve-files`.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Mutex;

use varve_core::{BlockId, NomenclatureId, RecordId, RevisionId, SurfaceId};
use varve_record::Entry;
use varve_revision::Publication;
use varve_schema::{Block, OptionRow, Schema};
use varve_surface::{BlockDefaults, Surface};

use crate::{
    BlockStore, LineageId, NomenclatureStore, RecordLogStore, RevisionStore, StoreError,
    SurfaceStore,
};

#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    logs: BTreeMap<RecordId, Vec<Entry>>,
    /// Content-addressed revision objects, shared across lineages.
    schemas: BTreeMap<RevisionId, Schema>,
    publications: BTreeMap<LineageId, Vec<Publication>>,
    blocks: BTreeMap<BlockId, Vec<Block>>,
    block_defaults: BTreeMap<(BlockId, u32), BlockDefaults>,
    nomenclatures: BTreeMap<NomenclatureId, Vec<Vec<OptionRow>>>,
    surfaces: BTreeMap<(RevisionId, SurfaceId), Surface>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("no panics while holding the lock")
    }
}

impl RecordLogStore for MemoryStore {
    async fn append(&self, record: &RecordId, entry: &Entry) -> Result<(), StoreError> {
        let mut inner = self.lock();
        let next = inner.logs.get(record).map_or(0, |log| log.len() as u64);
        if entry.envelope.seq != next {
            return Err(StoreError::SeqConflict {
                record: record.clone(),
                next,
                got: entry.envelope.seq,
            });
        }
        inner
            .logs
            .entry(record.clone())
            .or_default()
            .push(entry.clone());
        Ok(())
    }

    async fn entries(&self, record: &RecordId, from: u64) -> Result<Vec<Entry>, StoreError> {
        let inner = self.lock();
        Ok(inner
            .logs
            .get(record)
            .map(|log| log.iter().skip(from as usize).cloned().collect())
            .unwrap_or_default())
    }

    async fn version(&self, record: &RecordId) -> Result<u64, StoreError> {
        let inner = self.lock();
        Ok(inner.logs.get(record).map_or(0, |log| log.len() as u64))
    }

    async fn records(
        &self,
        after: Option<&RecordId>,
        limit: usize,
    ) -> Result<Vec<RecordId>, StoreError> {
        let inner = self.lock();
        let lower = match after {
            Some(id) => Bound::Excluded(id.clone()),
            None => Bound::Unbounded,
        };
        Ok(inner
            .logs
            .range((lower, Bound::Unbounded))
            .take(limit)
            .map(|(id, _)| id.clone())
            .collect())
    }
}

impl RevisionStore for MemoryStore {
    async fn append_publication(
        &self,
        lineage: &LineageId,
        index: u64,
        publication: &Publication,
        schema: &Schema,
    ) -> Result<(), StoreError> {
        let mut inner = self.lock();
        let next = inner
            .publications
            .get(lineage)
            .map_or(0, |log| log.len() as u64);
        if index != next {
            return Err(StoreError::PublicationConflict {
                lineage: lineage.clone(),
                next,
                got: index,
            });
        }
        // Object write is idempotent by content address: a revert or a
        // cross-lineage convergence lands on the existing object.
        inner
            .schemas
            .entry(publication.revision.clone())
            .or_insert_with(|| schema.clone());
        inner
            .publications
            .entry(lineage.clone())
            .or_default()
            .push(publication.clone());
        Ok(())
    }

    async fn publications(
        &self,
        lineage: &LineageId,
    ) -> Result<Vec<(Publication, Schema)>, StoreError> {
        let inner = self.lock();
        let Some(log) = inner.publications.get(lineage) else {
            return Ok(Vec::new());
        };
        log.iter()
            .map(|p| {
                let schema = inner.schemas.get(&p.revision).cloned().ok_or_else(|| {
                    StoreError::Corrupt(format!(
                        "publication of '{}' has no stored schema object",
                        p.revision
                    ))
                })?;
                Ok((p.clone(), schema))
            })
            .collect()
    }

    async fn schema(&self, id: &RevisionId) -> Result<Option<Schema>, StoreError> {
        let inner = self.lock();
        Ok(inner.schemas.get(id).cloned())
    }
}

impl BlockStore for MemoryStore {
    async fn append_block(&self, block: &Block) -> Result<(), StoreError> {
        let mut inner = self.lock();
        let next = inner.blocks.get(&block.id).map_or(0, |v| v.len() as u32) + 1;
        if block.version != next {
            return Err(StoreError::BlockVersionConflict {
                id: block.id.clone(),
                next,
                got: block.version,
            });
        }
        inner
            .blocks
            .entry(block.id.clone())
            .or_default()
            .push(block.clone());
        Ok(())
    }

    async fn blocks(&self) -> Result<Vec<Block>, StoreError> {
        let inner = self.lock();
        Ok(inner.blocks.values().flatten().cloned().collect())
    }

    async fn put_block_defaults(&self, defaults: &BlockDefaults) -> Result<(), StoreError> {
        let mut inner = self.lock();
        let key = (defaults.block.id.clone(), defaults.block.version);
        if inner.block_defaults.contains_key(&key) {
            return Err(StoreError::DefaultsExist {
                block: key.0,
                version: key.1,
            });
        }
        inner.block_defaults.insert(key, defaults.clone());
        Ok(())
    }

    async fn block_defaults(
        &self,
        block: &BlockId,
        version: u32,
    ) -> Result<Option<BlockDefaults>, StoreError> {
        let inner = self.lock();
        Ok(inner.block_defaults.get(&(block.clone(), version)).cloned())
    }
}

impl NomenclatureStore for MemoryStore {
    async fn append_nomenclature(
        &self,
        id: &NomenclatureId,
        version: u32,
        rows: &[OptionRow],
    ) -> Result<(), StoreError> {
        let mut inner = self.lock();
        let next = inner.nomenclatures.get(id).map_or(0, |v| v.len() as u32) + 1;
        if version != next {
            return Err(StoreError::NomenclatureVersionConflict {
                id: id.clone(),
                next,
                got: version,
            });
        }
        inner
            .nomenclatures
            .entry(id.clone())
            .or_default()
            .push(rows.to_vec());
        Ok(())
    }

    async fn nomenclatures(
        &self,
    ) -> Result<Vec<(NomenclatureId, u32, Vec<OptionRow>)>, StoreError> {
        let inner = self.lock();
        Ok(inner
            .nomenclatures
            .iter()
            .flat_map(|(id, versions)| {
                versions
                    .iter()
                    .enumerate()
                    .map(|(i, rows)| (id.clone(), i as u32 + 1, rows.clone()))
            })
            .collect())
    }
}

impl SurfaceStore for MemoryStore {
    async fn put_surface(&self, surface: &Surface) -> Result<(), StoreError> {
        let mut inner = self.lock();
        inner.surfaces.insert(
            (surface.revision.clone(), surface.id.clone()),
            surface.clone(),
        );
        Ok(())
    }

    async fn surface(
        &self,
        revision: &RevisionId,
        id: &SurfaceId,
    ) -> Result<Option<Surface>, StoreError> {
        let inner = self.lock();
        Ok(inner.surfaces.get(&(revision.clone(), id.clone())).cloned())
    }

    async fn surfaces(&self, revision: &RevisionId) -> Result<Vec<Surface>, StoreError> {
        let inner = self.lock();
        Ok(inner
            .surfaces
            .range((revision.clone(), SurfaceId::new(""))..)
            .take_while(|((rev, _), _)| rev == revision)
            .map(|(_, s)| s.clone())
            .collect())
    }
}
