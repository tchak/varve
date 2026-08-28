//! The kernel half of a case file (P.4 *Case-file record log*,
//! graphql.md G.14): reading the record — the fold of its log through
//! the head publication — and writing it, one `updateCells` batch as
//! one entry through the `varve-service` append operation.
//!
//! Both paths resolve the **head publication** first: the schema by
//! revision id, the fixed surface pair by content hash from the
//! publication's surface map. Reads prune the fold's snapshot (stale
//! cells of an older revision are inert — the log keeps them, the
//! view does not) and restrict cells and items to what the applicant
//! surface presents: the only writing side at P0, and the guard that
//! reviewer-only cells never leak (G.14). Findings are computed on
//! the *unrestricted* pruned snapshot — admissibility of both
//! surfaces is what "can this submit" needs, and findings carry no
//! cell values.

use varve_core::primitives::Instant;
use varve_core::{GroupId, ItemId, RecordId, RevisionId, SurfaceId};
use varve_record::{Actor, ActorKind, EntrySalts};
use varve_schema::{NomenclatureTable, Schema};
use varve_service::{AppendCells, AppendCellsError, CellWrite, append_cells};
use varve_store::load::{LoadError, load_dag, load_log};
use varve_store::{LineageId, RecordLogStore, RevisionStore, StoreError, SurfaceStore};
use varve_surface::{Node, Surface, SurfaceError};
use varve_value::{RecordValues, prune};

use crate::case_file::CaseFile;
use crate::procedure::stored_now;
use crate::publish::SharedExecutor;
use crate::surfaces::{APPLICANT_SURFACE, REVIEWER_SURFACE, SurfaceFinding, surface_findings};

/// A case file's record as reads render it: the pruned, restricted
/// values and the surface-tagged findings.
#[derive(Debug)]
pub struct RecordRead {
    /// Cells and item lists, restricted to the applicant surface.
    pub values: RecordValues,
    /// Admissibility per stored surface, applicant and reviewer.
    pub findings: Vec<SurfaceFinding>,
}

/// A read the store cannot serve honestly — every variant is
/// corruption or a wiring bug (`INTERNAL` at the API), never a
/// client mistake: a case file only exists on a published procedure.
#[derive(Debug, thiserror::Error)]
pub enum RecordReadError {
    #[error("procedure '{0}' has no publication, yet carries a case file")]
    NoPublication(uuid::Uuid),
    #[error("the head publication names no '{0}' surface")]
    MissingSurface(&'static str),
    #[error("the head publication's schema object is missing")]
    MissingSchema,
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Surface(#[from] SurfaceError),
    #[error("the log does not fold: {0}")]
    Fold(#[from] varve_record::FoldError),
}

/// Failure modes of [`update_case_file_cells`].
#[derive(Debug, thiserror::Error)]
pub enum UpdateCellsError {
    /// The `INVALID_WRITE` family (G.14): the batch is the client's
    /// mistake — surface refusal, bad anchor, inapplicable op,
    /// non-conforming values. Nothing stored.
    #[error(transparent)]
    Write(AppendCellsError),
    /// Resolving the head or the log failed — `INTERNAL`.
    #[error(transparent)]
    Read(#[from] RecordReadError),
    /// The row guard or the store refused the race (`CONFLICT`):
    /// re-read and retry.
    #[error("the case file changed since it was read; re-read and retry")]
    Conflict,
    #[error("database error: {0}")]
    Db(toasty::Error),
    /// Salt generation failed (OS randomness).
    #[error("salt generation failed: {0}")]
    Random(getrandom::Error),
}

/// The head publication's reading lens: schema, revision id, and the
/// fixed surface pair (P.4), resolved once per operation.
struct HeadContext {
    schema: Schema,
    revision: RevisionId,
    applicant: Surface,
    reviewer: Surface,
}

async fn head_context<S>(
    store: &S,
    procedure_id: uuid::Uuid,
) -> Result<HeadContext, RecordReadError>
where
    S: RevisionStore + SurfaceStore,
{
    let lineage = LineageId::new(procedure_id.to_string());
    let dag = load_dag(store, &lineage).await?;
    let Some((_, publication)) = dag.head() else {
        return Err(RecordReadError::NoPublication(procedure_id));
    };
    let revision = publication.revision.clone();
    let schema = dag
        .get(&revision)
        .map(|published| published.schema.clone())
        .ok_or(RecordReadError::MissingSchema)?;
    let fetch = async |name: &'static str| -> Result<Surface, RecordReadError> {
        let hash = publication
            .surfaces
            .get(&SurfaceId::new(name))
            .ok_or(RecordReadError::MissingSurface(name))?;
        store
            .surface(hash)
            .await?
            .ok_or_else(|| StoreError::Corrupt(format!("surface '{hash}' is not stored")).into())
    };
    let applicant = fetch(APPLICANT_SURFACE).await?;
    let reviewer = fetch(REVIEWER_SURFACE).await?;
    Ok(HeadContext {
        schema,
        revision,
        applicant,
        reviewer,
    })
}

/// The record of `case_file`, read through the head publication —
/// see the module docs for what is pruned, restricted and evaluated.
pub async fn case_file_record<S>(
    store: &S,
    case_file: &CaseFile,
) -> Result<RecordRead, RecordReadError>
where
    S: RevisionStore + SurfaceStore + RecordLogStore,
{
    let head = head_context(store, case_file.procedure_id).await?;
    let record = RecordId::new(case_file.record_id.to_string());
    let log = load_log(store, &record).await?;
    let fold = log.fold()?;
    let pending = fold.pending_set();
    let mut values = fold.values;
    prune(&mut values, &head.schema, &NomenclatureTable::new());
    let findings = surface_findings(
        &[
            (APPLICANT_SURFACE, &head.applicant),
            (REVIEWER_SURFACE, &head.reviewer),
        ],
        &head.schema,
        &values,
        &pending,
    )?;
    restrict(&mut values, &head.applicant);
    Ok(RecordRead { values, findings })
}

/// Applies one `updateCells` batch (G.14): the row is touched first —
/// the `#[version]` guard serializes concurrent batches into a clean
/// `CONFLICT` (and stamps `updated_at`, the activity read model) —
/// then the surface-gated append runs against the store scoped over
/// the same transaction. The caller owns the transaction (the
/// publication pattern): a refused batch rolls the row touch back
/// with everything else. On success the case file is updated in
/// place and the post-append read returned.
pub async fn update_case_file_cells<S>(
    exec: &SharedExecutor<'_>,
    store: &S,
    case_file: &mut CaseFile,
    actor_account_id: uuid::Uuid,
    writes: Vec<CellWrite>,
) -> Result<RecordRead, UpdateCellsError>
where
    S: RevisionStore + SurfaceStore + RecordLogStore,
{
    let head = head_context(store, case_file.procedure_id).await?;
    {
        let mut guard = exec.lock().await;
        // A no-change assignment: toasty emits the UPDATE (bumping
        // `updated_at` and checking `version`) only for a non-empty
        // assignment list.
        let state = case_file.state;
        case_file
            .update()
            .state(state)
            .exec(&mut **guard)
            .await
            .map_err(|e| {
                if e.is_condition_failed() {
                    UpdateCellsError::Conflict
                } else {
                    UpdateCellsError::Db(e)
                }
            })?;
    }
    let outcome = append_cells(
        store,
        AppendCells {
            record: RecordId::new(case_file.record_id.to_string()),
            surface: &head.applicant,
            schema: &head.schema,
            revision: head.revision.clone(),
            writes,
            actor: Actor {
                // The §2.13 pseudonymous reference (P.4): the
                // id→person mapping is the accounts table.
                id: actor_account_id.to_string(),
                kind: ActorKind::Human,
            },
            timestamp: Instant::parse(&stored_now().to_string())
                .expect("a jiff timestamp prints as strict RFC 3339"),
        },
        random_salts()?,
        mint_item_id,
    )
    .await
    .map_err(|error| match error {
        AppendCellsError::NotWritable(_)
        | AppendCellsError::GroupNotWritable(_)
        | AppendCellsError::UnknownAnchor { .. }
        | AppendCellsError::DoesNotApply(_)
        | AppendCellsError::Conformance(_) => UpdateCellsError::Write(error),
        AppendCellsError::Store(StoreError::SeqConflict { .. }) => UpdateCellsError::Conflict,
        AppendCellsError::Load(e) => UpdateCellsError::Read(e.into()),
        AppendCellsError::Store(e) => UpdateCellsError::Read(e.into()),
        AppendCellsError::Append(e) => {
            UpdateCellsError::Read(RecordReadError::Fold(match e {
                varve_record::AppendError::Unfoldable(f) => f,
                // Salt counts, base version, lifecycle ops: built
                // here, so a refusal is a wiring bug — surfaced as
                // the read-side corruption it is.
                other => {
                    return UpdateCellsError::Read(RecordReadError::Store(StoreError::Corrupt(
                        format!("kernel appender refused a built entry: {other}"),
                    )));
                }
            }))
        }
    })?;
    let mut values = outcome.values;
    let pending = varve_logic::PendingSet::default();
    let findings = surface_findings(
        &[
            (APPLICANT_SURFACE, &head.applicant),
            (REVIEWER_SURFACE, &head.reviewer),
        ],
        &head.schema,
        &values,
        &pending,
    )
    .map_err(RecordReadError::from)?;
    restrict(&mut values, &head.applicant);
    Ok(RecordRead { values, findings })
}

/// Restricts a snapshot to what `surface` presents: cells to its
/// columns, item lists to its groups (G.14 — the applicant lens).
fn restrict(values: &mut RecordValues, surface: &Surface) {
    let columns = surface.columns();
    let groups = surface_groups(surface);
    values
        .cells
        .retain(|addr, _| columns.contains(&addr.column));
    values.items.retain(|addr, _| groups.contains(&addr.group));
}

/// Every group the surface carries — presence, not writability
/// (a read-only group's rows are still shown).
fn surface_groups(surface: &Surface) -> std::collections::BTreeSet<GroupId> {
    fn walk(nodes: &[Node], out: &mut std::collections::BTreeSet<GroupId>) {
        for node in nodes {
            match node {
                Node::Group(g) => {
                    out.insert(g.group.clone());
                    walk(&g.children, out);
                }
                Node::Section(s) => walk(&s.children, out),
                Node::Column(_) | Node::Note(_) => {}
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(&surface.nodes, &mut out);
    out
}

/// Fresh OS randomness for one entry's salts (§2.13 decision 5:
/// random inputs from Tier 5, destroyed with the content they
/// commit). Drawn eagerly so a randomness failure refuses before
/// anything is written.
fn random_salts() -> Result<impl FnOnce(usize) -> EntrySalts, UpdateCellsError> {
    fn salt() -> Result<varve_core::canonical::Salt, getrandom::Error> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)?;
        Ok(varve_core::canonical::Salt(bytes))
    }
    // One meta salt now; op salts on demand would hide a failure
    // mid-append, so pre-draw generously is not an option — instead
    // the closure draws and a failure panics, which the eager meta
    // draw above makes as unlikely as the platform's other token
    // mints (`getrandom` failing after succeeding is an OS-level
    // catastrophe).
    let meta = salt().map_err(UpdateCellsError::Random)?;
    Ok(move |n: usize| EntrySalts {
        meta,
        ops: (0..n)
            .map(|_| salt().expect("OS randomness failed mid-entry"))
            .collect(),
    })
}

/// Mints a row identity (DESIGN §2.4), server-side like every id.
fn mint_item_id() -> ItemId {
    ItemId::new(uuid::Uuid::new_v4().simple().to_string())
}

/// Failure modes of [`submit_case_file`].
#[derive(Debug, thiserror::Error)]
pub enum SubmitCaseFileError {
    /// The state machine refuses (`INVALID_TRANSITION`): only a
    /// draft submits.
    #[error(transparent)]
    Transition(#[from] crate::case_file::CaseFileTransitionError),
    /// The record is not admissible through the applicant surface
    /// (`INADMISSIBLE`, G.15): the one place admissibility gates.
    /// The findings themselves are readable on the case file.
    #[error("{0} finding(s) on the applicant surface; fix the form and retry")]
    Inadmissible(usize),
    /// The stored state columns disagree.
    #[error(transparent)]
    Corrupt(#[from] crate::case_file::CorruptCaseFileState),
    /// Resolving the head or the log failed — `INTERNAL`.
    #[error(transparent)]
    Read(#[from] RecordReadError),
    /// The row guard refused the race (`CONFLICT`): re-read, retry.
    #[error("the case file changed since it was read; re-read and retry")]
    Conflict,
    #[error("database error: {0}")]
    Db(toasty::Error),
    /// Salt generation failed (OS randomness).
    #[error("salt generation failed: {0}")]
    Random(getrandom::Error),
}

/// Submits a case file (G.15, the dépôt): gate on admissibility of
/// the applicant surface (pending set from the fold — §2.8: pending
/// resolutions excuse what they cover), transition the state
/// machine, mirror the columns under the row's version guard, and
/// append the `submitted` checkpoint — reading revision pinned to
/// the head (§2.9's `pinned` default), nothing frozen (Q12: dépôt
/// does not lock the applicant form), nothing expected until
/// resolvers. One transaction, the caller's (the `updateCells`
/// pattern); the checkpoint entry is the authoritative fact and the
/// columns its read model (P.9 Q3). On success the case file is
/// updated in place.
pub async fn submit_case_file<S>(
    exec: &SharedExecutor<'_>,
    store: &S,
    case_file: &mut CaseFile,
    actor_account_id: uuid::Uuid,
) -> Result<(), SubmitCaseFileError>
where
    S: RevisionStore + SurfaceStore + RecordLogStore,
{
    let submitted = crate::case_file::current_case_file_state(case_file)?.submit(stored_now())?;

    let head = head_context(store, case_file.procedure_id).await?;
    let record = RecordId::new(case_file.record_id.to_string());
    let log = load_log(store, &record)
        .await
        .map_err(RecordReadError::from)?;
    let fold = log.fold().map_err(RecordReadError::from)?;
    let pending = fold.pending_set();
    let mut values = fold.values;
    prune(&mut values, &head.schema, &NomenclatureTable::new());
    let findings = surface_findings(
        &[(APPLICANT_SURFACE, &head.applicant)],
        &head.schema,
        &values,
        &pending,
    )
    .map_err(RecordReadError::from)?;
    if !findings.is_empty() {
        return Err(SubmitCaseFileError::Inadmissible(findings.len()));
    }

    let (state, state_since) = submitted.columns();
    {
        let mut guard = exec.lock().await;
        case_file
            .update()
            .state(state)
            .state_since(state_since)
            .exec(&mut **guard)
            .await
            .map_err(|e| {
                if e.is_condition_failed() {
                    SubmitCaseFileError::Conflict
                } else {
                    SubmitCaseFileError::Db(e)
                }
            })?;
    }

    varve_service::append_checkpoint(
        store,
        varve_service::AppendCheckpoint {
            record,
            checkpoint: varve_record::Checkpoint {
                name: "submitted".into(),
                reading_revision: head.revision.clone(),
                expected: vec![],
                frozen_columns: Default::default(),
                frozen_groups: Default::default(),
            },
            revision: head.revision,
            actor: Actor {
                id: actor_account_id.to_string(),
                kind: ActorKind::Human,
            },
            timestamp: Instant::parse(&stored_now().to_string())
                .expect("a jiff timestamp prints as strict RFC 3339"),
        },
        random_salts().map_err(|e| match e {
            UpdateCellsError::Random(e) => SubmitCaseFileError::Random(e),
            _ => unreachable!("random_salts fails only on randomness"),
        })?,
    )
    .await
    .map_err(|error| match error {
        varve_service::AppendCheckpointError::Store(StoreError::SeqConflict { .. }) => {
            SubmitCaseFileError::Conflict
        }
        varve_service::AppendCheckpointError::Load(e) => SubmitCaseFileError::Read(e.into()),
        varve_service::AppendCheckpointError::Store(e) => SubmitCaseFileError::Read(e.into()),
        varve_service::AppendCheckpointError::Append(e) => {
            SubmitCaseFileError::Read(RecordReadError::Store(StoreError::Corrupt(format!(
                "kernel appender refused a built checkpoint: {e}"
            ))))
        }
    })?;
    Ok(())
}
