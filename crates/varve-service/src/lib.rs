//! The choreography narrow waist (§13.2): transactional sequences
//! around the pure kernel, generic over the `varve-store` traits —
//! never an implementation. Hosts (the platform, tests) reach kernel
//! state through operations here or not at all; without this crate
//! every handler re-implements the sequence, and the invariants are
//! only as strong as the sloppiest one.
//!
//! Two operations: **impact-gated publication** (§3,
//! design/platform.md P.4 *Publication*): fork-point check → kernel
//! validation (schema, then each surface against it) → `varve-impact`
//! classification against the lineage head → gate on the report →
//! append the publication event and put the surfaces. Atomicity
//! across those store calls is the host's (the platform scopes the
//! store over one database transaction); against `MemoryStore` the
//! calls land individually, which tests accept.
//!
//! Steps, not loops (§2.8): nothing here schedules or retries — a
//! host calls an operation and owns the transaction around it. The
//! second operation is the **surface-gated cell append**
//! ([`append_cells`]): one batch, one record-log entry (platform
//! P.4 *Case-file record log*, graphql.md G.14).

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

/// One write of an [`AppendCells`] batch: the kernel patch ops with
/// item placement as a **sibling anchor**, never an index (`before:
/// None` appends), and item ids minted by the operation — the host
/// API argument (design/graphql.md G.7.3, G.14).
#[derive(Debug, Clone)]
pub enum CellWrite {
    Set {
        column: varve_core::ColumnId,
        path: varve_core::RowPath,
        state: varve_value::CellState,
    },
    /// Back to absent — distinct from `Set(Empty)` (§2.4).
    Unset {
        column: varve_core::ColumnId,
        path: varve_core::RowPath,
    },
    AddItem {
        group: varve_core::GroupId,
        parent: varve_core::RowPath,
        before: Option<varve_core::ItemId>,
    },
    RemoveItem {
        group: varve_core::GroupId,
        parent: varve_core::RowPath,
        item: varve_core::ItemId,
    },
    Reorder {
        group: varve_core::GroupId,
        parent: varve_core::RowPath,
        order: Vec<varve_core::ItemId>,
    },
}

/// A surface-gated cell append (§13.2, platform P.4 *Case-file record
/// log*): one batch, one entry. The writer's surface is resolved by
/// the host (authorization is surface assignment, §2.9); the schema
/// and revision are the head the batch authors against.
#[derive(Debug)]
pub struct AppendCells<'a> {
    pub record: varve_core::RecordId,
    /// The surface the writer writes through: cell ops must target
    /// its writable columns, item ops its writable groups (§2.9
    /// *surfaces absorb writability*).
    pub surface: &'a Surface,
    /// The schema the entry authors against — the head revision's.
    pub schema: &'a Schema,
    /// Its id, stamped on the entry envelope (§2.9: the record is
    /// never "on" a revision; every entry names its lens).
    pub revision: varve_core::RevisionId,
    pub writes: Vec<CellWrite>,
    pub actor: varve_record::Actor,
    pub timestamp: varve_core::primitives::Instant,
}

/// What an accepted append answers: the new log version and the
/// folded values after the entry — computed once here so the host
/// does not refold to render the response.
#[derive(Debug)]
pub struct AppendOutcome {
    pub version: u64,
    pub values: varve_value::RecordValues,
}

/// A refused batch. The first four are the host's `INVALID_WRITE`
/// family (design/graphql.md G.14): the client's mistake, nothing
/// stored. `Append` beyond `DoesNotApply` and the store's
/// `SeqConflict` are the concurrency answers.
#[derive(Debug, thiserror::Error)]
pub enum AppendCellsError {
    /// A cell op on a column the surface does not write (§2.9).
    #[error("column '{0}' is not writable through the surface")]
    NotWritable(varve_core::ColumnId),
    /// An item op on a group the surface does not write.
    #[error("group '{0}' is not writable through the surface")]
    GroupNotWritable(varve_core::GroupId),
    /// `AddItem` anchored on an item absent from its group's list.
    #[error("anchor item '{item}' is not in group '{group}'")]
    UnknownAnchor {
        group: varve_core::GroupId,
        item: varve_core::ItemId,
    },
    /// The ops do not apply to the current state, or the written
    /// values do not conform to the schema.
    #[error("{0}")]
    DoesNotApply(varve_value::ApplyError),
    /// The applied values do not fit the schema.
    #[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Conformance(Vec<varve_value::ConformanceError>),
    /// The kernel appender refused (salt counts, base version, a
    /// lifecycle op) — for a cells batch built here, a host bug.
    #[error(transparent)]
    Append(#[from] varve_record::AppendError),
    #[error(transparent)]
    Load(#[from] varve_store::load::LoadError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Appends one batch of cell writes to a record log as one entry
/// (§2.9, P.9 Q4): load and verify the log, gate every op on the
/// writer's surface, resolve sibling anchors and mint item ids
/// against the current fold, check the applied result conforms to
/// the schema, then `RecordLog::append` (which re-validates
/// applicability and the chain) and the conditional store append.
/// `salts` and `mint_item` are the Tier 5 randomness inputs, passed
/// in like timestamps (§2.13 decision 5); `salts` receives the op
/// count. Checkpoint frozen-set violations are deliberately not a
/// gate here — §2.9: a pure read over the log that append never
/// refuses on. Admissibility is likewise a read beside this
/// operation, never a gate (G.14).
pub async fn append_cells<S>(
    store: &S,
    request: AppendCells<'_>,
    salts: impl FnOnce(usize) -> varve_record::EntrySalts,
    mut mint_item: impl FnMut() -> varve_core::ItemId,
) -> Result<AppendOutcome, AppendCellsError>
where
    S: varve_store::RecordLogStore,
{
    let mut log = varve_store::load::load_log(store, &request.record).await?;
    let fold = log.fold().map_err(varve_record::AppendError::Unfoldable)?;

    // The surface gate (§2.9) before any translation: the refusal
    // names the offending element, not a downstream symptom.
    let writable_columns = request.surface.writable_columns();
    let writable_groups = request.surface.writable_groups();
    for write in &request.writes {
        match write {
            CellWrite::Set { column, .. } | CellWrite::Unset { column, .. } => {
                if !writable_columns.contains(column) {
                    return Err(AppendCellsError::NotWritable(column.clone()));
                }
            }
            CellWrite::AddItem { group, .. }
            | CellWrite::RemoveItem { group, .. }
            | CellWrite::Reorder { group, .. } => {
                if !writable_groups.contains(group) {
                    return Err(AppendCellsError::GroupNotWritable(group.clone()));
                }
            }
        }
    }

    // Translate against a working copy of the fold: sibling anchors
    // become indices, minted item ids land in the ops, and each op
    // must apply in batch order. The copy is pruned first (the
    // G.12/G.14 rule): stale cells of an older revision are inert —
    // without this, one removed column would refuse every later
    // batch as non-conforming. The log itself keeps them; pruning is
    // a property of this snapshot, never of history.
    let nomenclatures = NomenclatureTable::new();
    let mut values = fold.values.clone();
    varve_value::prune(&mut values, request.schema, &nomenclatures);
    let mut ops = Vec::with_capacity(request.writes.len());
    for write in request.writes {
        let op = match write {
            CellWrite::Set {
                column,
                path,
                state,
            } => varve_value::Op::Set {
                column,
                path,
                state,
            },
            CellWrite::Unset { column, path } => varve_value::Op::Unset { column, path },
            CellWrite::AddItem {
                group,
                parent,
                before,
            } => {
                let list = values.items.get(&varve_value::ItemsAddr {
                    group: group.clone(),
                    parent: parent.clone(),
                });
                let at = match &before {
                    Some(item) => list
                        .and_then(|l| l.iter().position(|i| i == item))
                        .ok_or_else(|| AppendCellsError::UnknownAnchor {
                            group: group.clone(),
                            item: item.clone(),
                        })?,
                    None => list.map_or(0, Vec::len),
                };
                varve_value::Op::AddItem {
                    group,
                    parent,
                    item: mint_item(),
                    at,
                }
            }
            CellWrite::RemoveItem {
                group,
                parent,
                item,
            } => varve_value::Op::RemoveItem {
                group,
                parent,
                item,
            },
            CellWrite::Reorder {
                group,
                parent,
                order,
            } => varve_value::Op::Reorder {
                group,
                parent,
                order,
            },
        };
        varve_value::apply(&mut values, &op).map_err(AppendCellsError::DoesNotApply)?;
        ops.push(op);
    }
    let errors = varve_value::check(&values, request.schema, &nomenclatures);
    if !errors.is_empty() {
        return Err(AppendCellsError::Conformance(errors));
    }

    let n = ops.len();
    // Server-side concurrency (platform P.4): read, fold and append
    // share one load, so the base is exact by construction.
    let base_version = log.version();
    let entry = log.append(varve_record::Draft {
        actor: request.actor,
        timestamp: request.timestamp,
        revision: request.revision,
        base_version,
        origin: varve_record::Origin::Entered,
        note: None,
        ops: ops.into_iter().map(varve_record::EntryOp::Cell).collect(),
        salts: salts(n),
    })?;
    store.append(&request.record, entry).await?;
    Ok(AppendOutcome {
        version: log.version(),
        values,
    })
}

/// A checkpoint append (§2.9, graphql.md G.15): one lifecycle entry —
/// the host builds the [`varve_record::Checkpoint`] (name, pinned
/// revision, expected resolutions, frozen sets filled from the
/// surface the checkpoint is taken through) and this operation lands
/// it in the log. Deliberately no admissibility gate here: whether a
/// checkpoint requires an admissible record is host policy (DN's
/// dépôt does, a future `returnToApplicant` will not); the kernel
/// only refuses what would poison the log — a checkpoint expecting
/// what is not pending ([`varve_record::AppendError`]).
#[derive(Debug)]
pub struct AppendCheckpoint {
    pub record: varve_core::RecordId,
    pub checkpoint: varve_record::Checkpoint,
    /// The entry envelope's authored-against revision — normally the
    /// checkpoint's own `reading_revision`.
    pub revision: varve_core::RevisionId,
    pub actor: varve_record::Actor,
    pub timestamp: varve_core::primitives::Instant,
}

/// A refused checkpoint append.
#[derive(Debug, thiserror::Error)]
pub enum AppendCheckpointError {
    /// The kernel appender refused: an unfoldable log, or a
    /// checkpoint whose expectations the fold does not hold.
    #[error(transparent)]
    Append(#[from] varve_record::AppendError),
    #[error(transparent)]
    Load(#[from] varve_store::load::LoadError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Appends one checkpoint entry. `salts` is the Tier 5 randomness
/// input (§2.13 decision 5), receiving the op count (always 1).
pub async fn append_checkpoint<S>(
    store: &S,
    request: AppendCheckpoint,
    salts: impl FnOnce(usize) -> varve_record::EntrySalts,
) -> Result<u64, AppendCheckpointError>
where
    S: varve_store::RecordLogStore,
{
    let mut log = varve_store::load::load_log(store, &request.record).await?;
    let base_version = log.version();
    let entry = log.append(varve_record::Draft {
        actor: request.actor,
        timestamp: request.timestamp,
        revision: request.revision,
        base_version,
        origin: varve_record::Origin::Entered,
        note: None,
        ops: vec![varve_record::EntryOp::Checkpoint(request.checkpoint)],
        salts: salts(1),
    })?;
    store.append(&request.record, entry).await?;
    Ok(log.version())
}
