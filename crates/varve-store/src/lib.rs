//! Tier 5 (§7, §13.2): async persistence traits for kernel objects —
//! the revision / block / nomenclature registries, surfaces, and
//! record logs.
//!
//! The traits speak **typed kernel objects** and per-method atomicity;
//! how an implementation lays them out is its own affair (§13.2 notes
//! the natural shape: event-store rows of canonical wire bytes plus
//! content-addressed registries). Every persistent kernel object is
//! either **append-only under a next-index rule** — log entries by
//! `seq`, publication events by index, block and nomenclature versions
//! by number — or **content-addressed** (revision objects). The
//! conditional append doubles as the optimistic-concurrency guard: the
//! caller names the index it built against, the store refuses any
//! other, and §2.9's detect-don't-merge needs nothing more from
//! storage.
//!
//! The store trusts nothing it returns: like the wire reader (§5),
//! **the loader enforces**. [`load`] rehydrates registries and logs by
//! replaying events through the kernel constructors, so chain
//! verification (§2.13), content-address recomputation, and registry
//! invariants (§2.11 append-only, block version numbering) re-run on
//! every load — a tampered row is caught at the first read, never
//! folded into state.
//!
//! Deliberately absent, with their §10 homes: the typed-query entry
//! point and read-model materializations (§13.3, Q18/Q19 — decided
//! with the first reviewer table); the snapshot cache (Q11 residual —
//! pointless until `varve-record` can fold *from* a snapshot);
//! resolution-instance and blob-reference enumerations across records
//! (`pending_resolutions`, `referenced_blobs` — read-model rows of
//! Q18's family, per §13.6; per-record they are already derived from
//! the log).

#![forbid(unsafe_code)]

use std::fmt;

use varve_core::{BlockId, NomenclatureId, RecordId, RevisionId, SurfaceId};
use varve_record::Entry;
use varve_revision::Publication;
use varve_schema::{Block, OptionRow, Schema};
use varve_surface::{BlockDefaults, Surface};

pub mod load;
mod memory;

pub use memory::MemoryStore;

/// Names one schema lineage — one revision DAG (§2.1) — in a store
/// holding many. The kernel has no "procedure": this is a storage
/// scoping key the host mints (the platform maps its procedure to it,
/// PLATFORM.md P.4), which is why it lives here and not in
/// `varve-core`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineageId(String);

impl LineageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LineageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The record log has moved since the caller folded it: `got` is
    /// the seq the entry was minted with, `next` the seq the store
    /// would accept. Reload, refold, re-mint (§2.9 — a conflict is
    /// detected, never merged).
    #[error("record '{record}': appended seq {got}, store expects {next}")]
    SeqConflict {
        record: RecordId,
        next: u64,
        got: u64,
    },
    /// Same shape for publication events: the lineage has publications
    /// the caller has not seen.
    #[error("lineage '{lineage}': appended publication #{got}, store expects #{next}")]
    PublicationConflict {
        lineage: LineageId,
        next: u64,
        got: u64,
    },
    /// The registry numbers block versions (§2.1); the store holds the
    /// same line.
    #[error("block '{id}': appended version {got}, store expects {next}")]
    BlockVersionConflict { id: BlockId, next: u32, got: u32 },
    #[error("nomenclature '{id}': appended version {got}, store expects {next}")]
    NomenclatureVersionConflict {
        id: NomenclatureId,
        next: u32,
        got: u32,
    },
    /// Block defaults are published once per (block, version) — a
    /// publication, not a document to edit.
    #[error("block '{block}' v{version}: defaults already stored")]
    DefaultsExist { block: BlockId, version: u32 },
    /// The backend answered but what it holds is not what was written
    /// (missing object for a stored event, undecodable row, …).
    #[error("corrupt store: {0}")]
    Corrupt(String),
    /// The backend failed (connection, transaction, IO). Stringly for
    /// now — the trait must not name any backend's error type; revisit
    /// with the first production implementation (Q19).
    #[error("backend: {0}")]
    Backend(String),
}

impl StoreError {
    pub fn backend(err: impl fmt::Display) -> Self {
        Self::Backend(err.to_string())
    }
}

/// Record logs (§2.9): the event store proper. Rows are entries keyed
/// `(record, seq)`; the store checks **only** the next-seq rule —
/// chain linkage, commitments, and foldability are the kernel's to
/// enforce, on append via [`varve_record::RecordLog::append`] and on
/// load via [`load::load_log`].
pub trait RecordLogStore: Send + Sync {
    /// Conditional append: accepted iff `entry.envelope.seq` equals
    /// the stored version (= entry count), else
    /// [`StoreError::SeqConflict`]. A record is created by its first
    /// entry; there is no separate create.
    fn append(
        &self,
        record: &RecordId,
        entry: &Entry,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Entries with `seq >= from`, ascending. Unknown record or
    /// past-the-end `from` → empty, HTTP-range style.
    fn entries(
        &self,
        record: &RecordId,
        from: u64,
    ) -> impl Future<Output = Result<Vec<Entry>, StoreError>> + Send;

    /// The stored version: entry count = the seq the next entry must
    /// carry. Unknown record → 0.
    fn version(&self, record: &RecordId) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// A page of known record ids, ascending by id order, strictly
    /// after `after` (`None` starts from the beginning). Serves the
    /// §13.6 audit sweep (recompute `referenced_blobs` over all logs);
    /// listing *for humans* is the read model's job (Q18), not this.
    fn records(
        &self,
        after: Option<&RecordId>,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<RecordId>, StoreError>> + Send;
}

/// Revision DAGs (§2.1): a **publication event log per lineage** plus
/// **content-addressed revision objects shared across lineages** —
/// identical schemas converge on one object (§2.13; 19.7% of the DN
/// corpus dedups, `corpus/M3-round-trip.md`).
pub trait RevisionStore: Send + Sync {
    /// Append publication event `index` (0-based; must equal the
    /// stored event count, else [`StoreError::PublicationConflict`])
    /// together with its schema object, atomically. The object write
    /// is idempotent by revision id — re-publication (a revert, §2.1)
    /// and cross-lineage convergence both land on the existing object.
    fn append_publication(
        &self,
        lineage: &LineageId,
        index: u64,
        publication: &Publication,
        schema: &Schema,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The lineage's publication events oldest-first, each joined with
    /// its schema object. Unknown lineage → empty.
    fn publications(
        &self,
        lineage: &LineageId,
    ) -> impl Future<Output = Result<Vec<(Publication, Schema)>, StoreError>> + Send;

    /// Point lookup of a revision object — the reading-lens fetch
    /// (§2.9): folding needs the schema an entry was authored against
    /// without loading a whole DAG.
    fn schema(
        &self,
        id: &RevisionId,
    ) -> impl Future<Output = Result<Option<Schema>, StoreError>> + Send;
}

/// The block registry (§2.1, Q5/Q13): schema-side halves as numbered
/// versions per block, surface-side defaults published beside them.
pub trait BlockStore: Send + Sync {
    /// Append the next version of `block.id`; `block.version` must be
    /// the stored count + 1, else [`StoreError::BlockVersionConflict`].
    /// Content validation is the registry's, re-run on load.
    fn append_block(&self, block: &Block) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Every version of every block, version order within a block.
    fn blocks(&self) -> impl Future<Output = Result<Vec<Block>, StoreError>> + Send;

    /// Publish the surface-side half for `(defaults.block.id,
    /// defaults.block.version)`: at most one, first write wins, a
    /// duplicate is [`StoreError::DefaultsExist`].
    fn put_block_defaults(
        &self,
        defaults: &BlockDefaults,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn block_defaults(
        &self,
        block: &BlockId,
        version: u32,
    ) -> impl Future<Output = Result<Option<BlockDefaults>, StoreError>> + Send;
}

/// The nomenclature registry (§2.12): numbered versions of option
/// rows. The §2.11 append-only rule (a version never removes ids) is
/// content, so it is the registry's — enforced at publish and re-run
/// on load, not here.
pub trait NomenclatureStore: Send + Sync {
    /// Append version `version` of `id`; must be the stored count + 1,
    /// else [`StoreError::NomenclatureVersionConflict`].
    fn append_nomenclature(
        &self,
        id: &NomenclatureId,
        version: u32,
        rows: &[OptionRow],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Every version of every nomenclature, version order within one.
    fn nomenclatures(
        &self,
    ) -> impl Future<Output = Result<Vec<(NomenclatureId, u32, Vec<OptionRow>)>, StoreError>> + Send;
}

/// Surfaces (§2.1, §2.6), keyed `(surface.revision, surface.id)`.
/// Upsert: a surface is authored and re-authored while its procedure
/// is drafted; freezing surfaces once their revision serves live
/// records is service policy, not a storage property.
pub trait SurfaceStore: Send + Sync {
    fn put_surface(&self, surface: &Surface)
    -> impl Future<Output = Result<(), StoreError>> + Send;

    fn surface(
        &self,
        revision: &RevisionId,
        id: &SurfaceId,
    ) -> impl Future<Output = Result<Option<Surface>, StoreError>> + Send;

    /// All surfaces of a revision, ascending by surface id.
    fn surfaces(
        &self,
        revision: &RevisionId,
    ) -> impl Future<Output = Result<Vec<Surface>, StoreError>> + Send;
}
