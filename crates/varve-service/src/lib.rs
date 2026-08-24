//! The choreography narrow waist (§13.2): transactional sequences
//! around the pure kernel, generic over the `varve-store` traits —
//! never an implementation. Hosts (the platform, tests) reach kernel
//! state through operations here or not at all; without this crate
//! every handler re-implements the sequence, and the invariants are
//! only as strong as the sloppiest one.
//!
//! First operation: **impact-gated publication** (§3,
//! design/platform.md P.4 *Publication*): fork-point check → kernel
//! validation (schema, then each surface against it) → `varve-impact`
//! classification against the lineage head → gate on the report →
//! append the publication event and put the surfaces. Atomicity
//! across those store calls is the host's (the platform scopes the
//! store over one database transaction); against `MemoryStore` the
//! calls land individually, which tests accept.
//!
//! Steps, not loops (§2.8): nothing here schedules or retries — a
//! host calls an operation and owns the transaction around it.

#![forbid(unsafe_code)]

use varve_core::RevisionId;
use varve_impact::{ChangeClass, ImpactReport};
use varve_revision::Publication;
use varve_schema::{DepthPolicy, NomenclatureTable, Schema, SchemaError, revision_id};
use varve_store::load::{LoadError, load_dag};
use varve_store::{LineageId, RevisionStore, StoreError, SurfaceStore};
use varve_surface::{Surface, SurfaceError};

/// A publication request: the schema and its compiled surfaces, both
/// already carrying the revision id the caller computed
/// (`varve_schema::revision_id`), plus the fork point the caller
/// authored against.
#[derive(Debug)]
pub struct PublishRevision {
    /// The revision DAG to publish into.
    pub lineage: LineageId,
    /// The head the caller forked from: `None` for a first
    /// publication. Must equal the lineage's current head, else
    /// [`PublishRevisionError::StaleBase`] — a conflict is detected,
    /// never merged (§2.9's spirit at the lineage level).
    pub base: Option<RevisionId>,
    /// The schema to publish.
    pub schema: Schema,
    /// The compiled surfaces, each naming the schema's revision id
    /// (validation re-checks the pairing).
    pub surfaces: Vec<Surface>,
    /// Accept a report whose [`ImpactReport::worst`] exceeds
    /// [`ChangeClass::Safe`]. Without it such a report is returned
    /// and nothing is written (§3: a lossy or breaking publication
    /// requires explicit confirmation carrying the report).
    pub confirm: bool,
}

/// What a publication answered.
#[derive(Debug)]
pub enum PublishOutcome {
    /// The event is appended and the surfaces stored.
    Published {
        revision: RevisionId,
        report: ImpactReport,
    },
    /// The report exceeds `Safe` and the request did not confirm:
    /// nothing was written.
    RequiresConfirmation { report: ImpactReport },
}

#[derive(Debug, thiserror::Error)]
pub enum PublishRevisionError {
    /// The lineage head moved since the caller forked: reload, rebase
    /// or discard, retry. `head` is the current head (`None` = the
    /// lineage is empty and the caller claimed a base).
    #[error("lineage head is {head:?}, publication was authored against {base:?}")]
    StaleBase {
        base: Option<RevisionId>,
        head: Option<RevisionId>,
    },
    /// The schema fails kernel validation — a host bug: authoring is
    /// where invalid schemas are refused.
    #[error("schema does not validate: {0:?}")]
    Schema(Vec<SchemaError>),
    /// A surface fails validation against the schema — a host bug in
    /// surface compilation.
    #[error("surface does not validate: {0:?}")]
    Surface(Vec<SurfaceError>),
    /// The impact classifier found no cast path — carried as an error
    /// because §3's table is total over legal schemas.
    #[error("impact classification failed: {0}")]
    Impact(#[from] varve_schema::CastError),
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Publishes a revision and its surfaces into a lineage, gated on the
/// impact report. See the crate docs for the sequence; the empty
/// lineage and the first publication fall out of one code path — the
/// head is `None` and the report classifies against the empty schema
/// (every column `Added`, free).
pub async fn publish_revision<S>(
    store: &S,
    request: PublishRevision,
) -> Result<PublishOutcome, PublishRevisionError>
where
    S: RevisionStore + SurfaceStore,
{
    let dag = load_dag(store, &request.lineage).await?;
    let head = dag.latest().cloned();
    if request.base != head {
        return Err(PublishRevisionError::StaleBase {
            base: request.base,
            head,
        });
    }

    let schema_errors = varve_schema::validate(&request.schema, DepthPolicy::default());
    if !schema_errors.is_empty() {
        return Err(PublishRevisionError::Schema(schema_errors));
    }
    let nomenclatures = NomenclatureTable::new();
    for surface in &request.surfaces {
        let surface_errors = varve_surface::validate(surface, &request.schema, &nomenclatures);
        if !surface_errors.is_empty() {
            return Err(PublishRevisionError::Surface(surface_errors));
        }
    }

    let empty = Schema::default();
    let from = head
        .as_ref()
        .and_then(|id| dag.get(id))
        .map(|published| &published.schema)
        .unwrap_or(&empty);
    let report = varve_impact::classify(from, &request.schema, &nomenclatures)?;
    if report.worst() > ChangeClass::Safe && !request.confirm {
        return Ok(PublishOutcome::RequiresConfirmation { report });
    }

    let revision = revision_id(&request.schema);
    let parents: Vec<RevisionId> = head.into_iter().collect();
    let index = dag.publications().len() as u64;
    store
        .append_publication(
            &request.lineage,
            index,
            &Publication {
                revision: revision.clone(),
                parents,
            },
            &request.schema,
        )
        .await?;
    for surface in &request.surfaces {
        store.put_surface(surface).await?;
    }
    Ok(PublishOutcome::Published { revision, report })
}
