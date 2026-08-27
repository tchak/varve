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
//! as the JCS bytes of `varve_surface::canon::surface_canonical`
//! keyed by their content hash (§2.13 decision 9 — immutable, a
//! divergent re-put refused), publications as the JCS bytes of
//! `varve_revision::publication_canonical` — decode failures surface
//! as [`StoreError::Corrupt`], never as a silent repair, matching
//! the loaders' §2.13 posture.
//!
//! Scope: `RevisionStore` + `SurfaceStore` (publication, P.4) and
//! `RecordLogStore` (case files, P.4 *Case-file record log*): entry
//! rows split into the §2.13 halves — envelope bytes beside the
//! erasable content + salts — so §2.10 redaction is a column update
//! under a chain that still verifies. The block and nomenclature
//! tables land with the first block.

#![forbid(unsafe_code)]

mod rows;

use std::collections::BTreeMap;

use toasty::Executor;
use tokio::sync::Mutex;
use varve_core::canonical::{CanonicalValue, ContentHash, canonical_bytes};
use varve_core::{RecordId, RevisionId};
use varve_record::Entry;
use varve_revision::Publication;
use varve_schema::Schema;
use varve_store::{LineageId, RecordLogStore, RevisionStore, StoreError, SurfaceStore};
use varve_surface::Surface;

use crate::rows::{PublicationRow, RecordEntryRow, RevisionRow, SurfaceRow};

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
        let body = varve_core::canonical::canonical_bytes(&varve_revision::publication_canonical(
            publication,
        ))
        .map_err(|e| StoreError::Corrupt(format!("publication does not encode: {e}")))?;
        PublicationRow::create()
            .lineage(lineage.as_str())
            .index(index)
            .revision(publication.revision.as_str())
            .body(body)
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
            let publication = decode_publication(&row.body)?;
            if publication.revision.as_str() != row.revision {
                return Err(StoreError::Corrupt(format!(
                    "publication row of '{}' carries a body naming '{}'",
                    row.revision, publication.revision
                )));
            }
            out.push((publication, decode_schema(&row.revision, &object.schema)?));
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
    async fn put_surface(&self, surface: &Surface) -> Result<ContentHash, StoreError> {
        let hash = surface.content_hash();
        let body =
            varve_core::canonical::canonical_bytes(&varve_surface::surface_canonical(surface))
                .map_err(|e| StoreError::Corrupt(format!("surface does not encode: {e}")))?;
        let mut guard = self.exec.lock().await;
        // Immutable (§2.13 decision 9): idempotent under the content
        // hash, a divergent row under it refused, never replaced.
        let existing = SurfaceRow::filter_by_hash(hash.to_string())
            .first()
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        match existing {
            Some(row) if row.body == body => {}
            Some(_) => return Err(StoreError::SurfaceMismatch { hash }),
            None => {
                SurfaceRow::create()
                    .hash(hash.to_string())
                    .revision(surface.revision.as_str())
                    .body(body)
                    .exec(&mut **guard)
                    .await
                    .map_err(backend)?;
            }
        }
        Ok(hash)
    }

    async fn surface(&self, hash: &ContentHash) -> Result<Option<Surface>, StoreError> {
        let mut guard = self.exec.lock().await;
        let row = SurfaceRow::filter_by_hash(hash.to_string())
            .first()
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        row.map(|r| decode_surface(&r.body)).transpose()
    }
}

/// The envelope fields of `varve_record::canon::entry_canonical` —
/// the plaintext half that survives redaction (§2.13 decision 8).
/// Everything else in the flat canonical object (ops, origin, note,
/// salts) is the erasable half. `entry_from` re-validates the
/// partition on read: a missing or extra key fails decoding.
const ENVELOPE_KEYS: [&str; 8] = [
    "seq",
    "prev",
    "actor",
    "actor_kind",
    "timestamp",
    "revision",
    "base_version",
    "content_hash",
];

/// Splits one entry into the two stored halves, each JCS bytes of an
/// object holding its share of `entry_canonical`'s fields.
fn encode_entry(entry: &Entry) -> Result<(Vec<u8>, Vec<u8>), StoreError> {
    let CanonicalValue::Object(fields) = varve_record::canon::entry_canonical(entry) else {
        return Err(StoreError::Corrupt(
            "entry_canonical is not an object".into(),
        ));
    };
    let (envelope, content): (BTreeMap<_, _>, BTreeMap<_, _>) = fields
        .into_iter()
        .partition(|(key, _)| ENVELOPE_KEYS.contains(&key.as_str()));
    let bytes = |half: BTreeMap<String, CanonicalValue>| {
        canonical_bytes(&CanonicalValue::Object(half))
            .map_err(|e| StoreError::Corrupt(format!("entry does not encode: {e}")))
    };
    Ok((bytes(envelope)?, bytes(content)?))
}

/// Rejoins the halves and decodes through the kernel codec, which
/// enforces the exact key set and the op↔salt pairing.
fn decode_entry(
    record: &str,
    seq: u64,
    envelope: &[u8],
    content: &[u8],
) -> Result<Entry, StoreError> {
    let corrupt = |e: &dyn std::fmt::Display| {
        StoreError::Corrupt(format!("entry {seq} of '{record}' is unreadable: {e}"))
    };
    let mut fields = match varve_wire::canonical_from_bytes(envelope).map_err(|e| corrupt(&e))? {
        CanonicalValue::Object(fields) => fields,
        _ => return Err(corrupt(&"envelope is not an object")),
    };
    match varve_wire::canonical_from_bytes(content).map_err(|e| corrupt(&e))? {
        CanonicalValue::Object(rest) => fields.extend(rest),
        _ => return Err(corrupt(&"content is not an object")),
    }
    let entry = varve_record::canon::entry_from(&CanonicalValue::Object(fields))
        .map_err(|e| corrupt(&e))?;
    if entry.envelope.seq != seq {
        return Err(corrupt(&format!(
            "row seq {seq} carries an entry claiming seq {}",
            entry.envelope.seq
        )));
    }
    Ok(entry)
}

impl RecordLogStore for PlatformStore<'_> {
    async fn append(&self, record: &RecordId, entry: &Entry) -> Result<(), StoreError> {
        let (envelope, content) = encode_entry(entry)?;
        let mut guard = self.exec.lock().await;
        let next = RecordEntryRow::filter_by_record(record.as_str())
            .count()
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        if entry.envelope.seq != next {
            return Err(StoreError::SeqConflict {
                record: record.clone(),
                next,
                got: entry.envelope.seq,
            });
        }
        RecordEntryRow::create()
            .record(record.as_str())
            .seq(entry.envelope.seq)
            .envelope(envelope)
            .content(content)
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn entries(&self, record: &RecordId, from: u64) -> Result<Vec<Entry>, StoreError> {
        let mut guard = self.exec.lock().await;
        let rows = RecordEntryRow::filter_by_record(record.as_str())
            .filter(RecordEntryRow::fields().seq().ge(from))
            .order_by(RecordEntryRow::fields().seq().asc())
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        rows.iter()
            .map(|row| decode_entry(record.as_str(), row.seq, &row.envelope, &row.content))
            .collect()
    }

    async fn version(&self, record: &RecordId) -> Result<u64, StoreError> {
        let mut guard = self.exec.lock().await;
        RecordEntryRow::filter_by_record(record.as_str())
            .count()
            .exec(&mut **guard)
            .await
            .map_err(backend)
    }

    async fn records(
        &self,
        after: Option<&RecordId>,
        limit: usize,
    ) -> Result<Vec<RecordId>, StoreError> {
        let mut guard = self.exec.lock().await;
        // One row per record: every record has an entry at seq 0 (a
        // record is created by its first entry). An audit-sweep read
        // (§13.6), not a hot path — no index beyond the primary key.
        let mut filter = RecordEntryRow::fields().seq().eq(0u64);
        if let Some(after) = after {
            filter = filter.and(RecordEntryRow::fields().record().gt(after.as_str()));
        }
        let rows = RecordEntryRow::filter(filter)
            .order_by(RecordEntryRow::fields().record().asc())
            .limit(limit)
            .exec(&mut **guard)
            .await
            .map_err(backend)?;
        Ok(rows
            .into_iter()
            .map(|row| RecordId::new(row.record))
            .collect())
    }
}

fn decode_schema(revision: &str, bytes: &[u8]) -> Result<Schema, StoreError> {
    varve_wire::schema_from_bytes(bytes).map_err(|e| {
        StoreError::Corrupt(format!("stored schema of '{revision}' is unreadable: {e}"))
    })
}

fn decode_publication(bytes: &[u8]) -> Result<Publication, StoreError> {
    let value = varve_wire::canonical_from_bytes(bytes)
        .map_err(|e| StoreError::Corrupt(format!("stored publication is unreadable: {e}")))?;
    varve_revision::publication_from(&value)
        .map_err(|e| StoreError::Corrupt(format!("stored publication is unreadable: {e}")))
}

fn decode_surface(bytes: &[u8]) -> Result<Surface, StoreError> {
    let value = varve_wire::canonical_from_bytes(bytes)
        .map_err(|e| StoreError::Corrupt(format!("stored surface is unreadable: {e}")))?;
    varve_surface::surface_from(&value)
        .map_err(|e| StoreError::Corrupt(format!("stored surface is unreadable: {e}")))
}
