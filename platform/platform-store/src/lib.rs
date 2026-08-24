//! The Toasty implementation of the `varve-store` traits (P.3):
//! kernel tables as `pub(crate)` models, so kernel objects are never
//! touched at the store level from platform code — structurally.
//!
//! [`PlatformStore`] is a **scoped value, not a service** (P.3, was
//! P.9 Q10): a use case opens one toasty transaction, shares its
//! executor behind an async mutex, constructs the store over that
//! mutex, runs platform writes and kernel writes against the same
//! transaction, and commits once. The `varve-store` traits take
//! `&self` while every toasty `exec` wants `&mut dyn Executor`; the
//! mutex bridges the two, serializing kernel-store calls within one
//! use case — which is the store's whole concurrency story here: the
//! caller's transaction isolates across use cases, the append
//! index rules detect (never merge) lost races, and the primary keys
//! backstop the one race the count-check cannot see.
//!
//! Stored bytes are the wire's (§13.2: the natural storage row is
//! the wire line): schemas as `varve_wire::schema_bytes`, surfaces
//! as the JCS bytes of `varve_surface::canon::surface_canonical` —
//! decode failures surface as [`StoreError::Corrupt`], never as a
//! silent repair, matching the loaders' §2.13 posture.
//!
//! Scope: `RevisionStore` + `SurfaceStore` (publication, P.4). The
//! record-log, block, and nomenclature tables land with case files.

#![forbid(unsafe_code)]

mod rows;

use toasty::Executor;
use tokio::sync::Mutex;
use varve_core::{RevisionId, SurfaceId};
use varve_revision::Publication;
use varve_schema::Schema;
use varve_store::{LineageId, RevisionStore, StoreError, SurfaceStore};
use varve_surface::Surface;

use crate::rows::{PublicationRow, RevisionRow, SurfaceRow};

/// Every migration of the kernel tables, embedded at compile time
/// from `toasty/`. Applied beside the platform set by
/// `platform_core::connect_with`.
pub static MIGRATIONS: toasty::migration::MigrationSet = toasty::embed_migrations!();

/// This crate's models, for registration into the one shared
/// [`toasty::Db`] (P.3: one database, one migration table, each
/// crate owning its files).
pub fn models() -> toasty::ModelSet {
    toasty::models!(crate::*)
}

/// The executor one use case shares between its platform writes and
/// this store: the open transaction, behind the mutex that bridges
/// `&self` trait methods to `&mut dyn Executor` execs.
pub type SharedExecutor<'t> = Mutex<&'t mut dyn Executor>;

/// The scoped store. Construct it over a [`SharedExecutor`] wrapping
/// an open transaction; drop it before committing.
pub struct PlatformStore<'t> {
    exec: &'t SharedExecutor<'t>,
}

impl<'t> PlatformStore<'t> {
    pub fn new(exec: &'t SharedExecutor<'t>) -> Self {
        Self { exec }
    }
}

fn backend(err: toasty::Error) -> StoreError {
    StoreError::Backend(err.to_string())
}

impl RevisionStore for PlatformStore<'_> {
    async fn append_publication(
        &self,
        lineage: &LineageId,
        index: u64,
        publication: &Publication,
        schema: &Schema,
    ) -> Result<(), StoreError> {
        let mut guard = self.exec.lock().await;
        let next = PublicationRow::filter_by_lineage(lineage.as_str())
            .count()
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        if index != next {
            return Err(StoreError::PublicationConflict {
                lineage: lineage.clone(),
                next,
                got: index,
            });
        }
        // Object write is idempotent by content address (§2.13): a
        // revert or a cross-lineage convergence lands on the existing
        // object.
        let existing = RevisionRow::filter_by_revision(publication.revision.as_str())
            .first()
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        if existing.is_none() {
            RevisionRow::create()
                .revision(publication.revision.as_str())
                .schema(varve_wire::schema_bytes(schema))
                .exec(&mut **guard)
                .await
                .map_err(backend)?;
        }
        PublicationRow::create()
            .lineage(lineage.as_str())
            .index(index)
            .revision(publication.revision.as_str())
            .parents(
                publication
                    .parents
                    .iter()
                    .map(|p| p.as_str().to_owned())
                    .collect::<Vec<_>>(),
            )
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn publications(
        &self,
        lineage: &LineageId,
    ) -> Result<Vec<(Publication, Schema)>, StoreError> {
        let mut guard = self.exec.lock().await;
        let rows = PublicationRow::filter_by_lineage(lineage.as_str())
            .order_by(PublicationRow::fields().index().asc())
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let object = RevisionRow::filter_by_revision(&row.revision)
                .first()
                .exec(&mut **guard)
                .await
                .map_err(backend)?
                .ok_or_else(|| {
                    StoreError::Corrupt(format!(
                        "publication of '{}' has no stored schema object",
                        row.revision
                    ))
                })?;
            out.push((
                Publication {
                    revision: RevisionId::new(row.revision.clone()),
                    parents: row.parents.iter().map(RevisionId::new).collect(),
                },
                decode_schema(&row.revision, &object.schema)?,
            ));
        }
        Ok(out)
    }

    async fn schema(&self, id: &RevisionId) -> Result<Option<Schema>, StoreError> {
        let mut guard = self.exec.lock().await;
        let row = RevisionRow::filter_by_revision(id.as_str())
            .first()
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        row.map(|r| decode_schema(id.as_str(), &r.schema))
            .transpose()
    }
}

impl SurfaceStore for PlatformStore<'_> {
    async fn put_surface(&self, surface: &Surface) -> Result<(), StoreError> {
        let body =
            varve_core::canonical::canonical_bytes(&varve_surface::surface_canonical(surface))
                .map_err(|e| StoreError::Corrupt(format!("surface does not encode: {e}")))?;
        let mut guard = self.exec.lock().await;
        // Upsert (the trait contract): re-authored while drafted.
        SurfaceRow::upsert_by_revision_and_surface(surface.revision.as_str(), surface.id.as_str())
            .body(body)
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn surface(
        &self,
        revision: &RevisionId,
        id: &SurfaceId,
    ) -> Result<Option<Surface>, StoreError> {
        let mut guard = self.exec.lock().await;
        let row = SurfaceRow::filter_by_revision_and_surface(revision.as_str(), id.as_str())
            .first()
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        row.map(|r| decode_surface(&r.body)).transpose()
    }

    async fn surfaces(&self, revision: &RevisionId) -> Result<Vec<Surface>, StoreError> {
        let mut guard = self.exec.lock().await;
        let rows = SurfaceRow::filter_by_revision(revision.as_str())
            .order_by(SurfaceRow::fields().surface().asc())
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        rows.iter().map(|r| decode_surface(&r.body)).collect()
    }
}

fn decode_schema(revision: &str, bytes: &[u8]) -> Result<Schema, StoreError> {
    varve_wire::schema_from_bytes(bytes).map_err(|e| {
        StoreError::Corrupt(format!("stored schema of '{revision}' is unreadable: {e}"))
    })
}

fn decode_surface(bytes: &[u8]) -> Result<Surface, StoreError> {
    let value = varve_wire::canonical_from_bytes(bytes)
        .map_err(|e| StoreError::Corrupt(format!("stored surface is unreadable: {e}")))?;
    varve_surface::surface_from(&value)
        .map_err(|e| StoreError::Corrupt(format!("stored surface is unreadable: {e}")))
}
