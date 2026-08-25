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

use std::collections::BTreeMap;

use varve_core::canonical::ContentHash;
use varve_core::{PublicationId, RevisionId, SurfaceId};
use varve_impact::{ChangeClass, ImpactReport};
use varve_revision::Publication;
use varve_schema::{DepthPolicy, NomenclatureTable, Schema, SchemaError, revision_id};
use varve_store::load::{LoadError, load_dag};
use varve_store::{LineageId, RevisionStore, StoreError, SurfaceStore};
use varve_surface::{Surface, SurfaceError, SurfaceReport};

/// A publication request: the schema and its compiled surfaces, both
/// already carrying the revision id the caller computed
/// (`varve_schema::revision_id`), plus the fork point the caller
/// authored against.
#[derive(Debug)]
pub struct PublishRevision {
    /// The revision DAG to publish into.
    pub lineage: LineageId,
    /// The head the caller forked from — a **publication id** (§2.13
    /// decision 9: two surface-only publications from one revision are
    /// distinct forks, which revision ids cannot tell apart). `None`
    /// for a first publication. Must equal the lineage's current head,
    /// else [`PublishRevisionError::StaleBase`] — a conflict is
    /// detected, never merged (§2.9's spirit at the lineage level).
    pub base: Option<PublicationId>,
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

/// What a publication answered. Both arms carry both halves of the
/// impact story (§3.1): the schema report and the surface report,
/// composed here — the gate takes the worst class of the pair.
#[derive(Debug)]
pub enum PublishOutcome {
    /// The event is appended and the surfaces stored.
    Published {
        /// The event's content address (§2.13 decision 9) — what a
        /// host records as its new head and fork anchor.
        publication: PublicationId,
        revision: RevisionId,
        report: ImpactReport,
        surface_report: SurfaceReport,
    },
    /// The worst class of the pair exceeds `Safe` and the request did
    /// not confirm: nothing was written.
    RequiresConfirmation {
        report: ImpactReport,
        surface_report: SurfaceReport,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PublishRevisionError {
    /// The lineage head moved since the caller forked: reload, rebase
    /// or discard, retry. `head` is the current head (`None` = the
    /// lineage is empty and the caller claimed a base).
    #[error("lineage head is {head:?}, publication was authored against {base:?}")]
    StaleBase {
        base: Option<PublicationId>,
        head: Option<PublicationId>,
    },
    /// Same revision, same surface set as the head (§2.13 decision 9):
    /// a refused no-op — an append-only history has no meaning for a
    /// node identical to its parent.
    #[error("the publication is identical to the head — nothing to publish")]
    NothingToPublish,
    /// Two request surfaces share an id — a host bug: the surface set
    /// is a map (§2.13 decision 9).
    #[error("two surfaces share the id '{0}'")]
    DuplicateSurface(SurfaceId),
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
    let mut surfaces: BTreeMap<SurfaceId, ContentHash> = BTreeMap::new();
    for surface in &request.surfaces {
        let surface_errors = varve_surface::validate(surface, &request.schema, &nomenclatures);
        if !surface_errors.is_empty() {
            return Err(PublishRevisionError::Surface(surface_errors));
        }
        if surfaces
            .insert(surface.id.clone(), surface.content_hash())
            .is_some()
        {
            return Err(PublishRevisionError::DuplicateSurface(surface.id.clone()));
        }
    }

    let revision = revision_id(&request.schema);
    // §2.13 decision 9: identical to the head — revision and surface
    // set both — is a refused no-op, checked before the report so the
    // caller hears "nothing to publish", not "safe".
    if let Some((_, head_publication)) = dag.head()
        && head_publication.revision == revision
        && head_publication.surfaces == surfaces
    {
        return Err(PublishRevisionError::NothingToPublish);
    }

    let empty = Schema::default();
    let from = dag
        .head()
        .and_then(|(_, p)| dag.get(&p.revision))
        .map(|published| &published.schema)
        .unwrap_or(&empty);
    let report = varve_impact::classify(from, &request.schema, &nomenclatures)?;

    // The surface half (§3.1): the head publication's surface set,
    // resolved by content hash, against the request's. A hash the
    // store does not hold is corruption, surfaced.
    let mut base_surfaces = Vec::new();
    if let Some((_, head_publication)) = dag.head() {
        for hash in head_publication.surfaces.values() {
            let stored = store.surface(hash).await?.ok_or_else(|| {
                StoreError::Corrupt(format!(
                    "the head publication names surface '{hash}', which the store does not hold"
                ))
            })?;
            base_surfaces.push(stored);
        }
    }
    let surface_report = varve_surface::diff_sets(&base_surfaces, &request.surfaces);

    // The gate takes the worst class of the pair (§3.1): a
    // requiredness tightening requires the same confirmation a lossy
    // cast does.
    if report.worst().max(surface_report.worst()) > ChangeClass::Safe && !request.confirm {
        return Ok(PublishOutcome::RequiresConfirmation {
            report,
            surface_report,
        });
    }

    // The publication first — its append writes the revision object
    // the surfaces reference (platform-store keeps that as a foreign
    // key) — then the surfaces (idempotent, content-addressed).
    // Atomicity across the calls is the host's transaction (crate
    // docs); a torn MemoryStore sequence leaves a publication whose
    // hashes surface as missing at first read, never silent.
    let publication = Publication {
        revision: revision.clone(),
        parents: head.into_iter().collect(),
        surfaces,
    };
    let id = publication.id();
    let index = dag.publications().len() as u64;
    store
        .append_publication(&request.lineage, index, &publication, &request.schema)
        .await?;
    for surface in &request.surfaces {
        store.put_surface(surface).await?;
    }
    Ok(PublishOutcome::Published {
        publication: id,
        revision,
        report,
        surface_report,
    })
}
