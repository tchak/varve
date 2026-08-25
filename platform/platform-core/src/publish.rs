//! The publication use case (P.4 *Publication*): the one place the
//! `varve-service` operation composes with its platform side effects
//! — the lifecycle transition, the draft consumed, `latest_revision`
//! maintained, the `published` event with its facts — all against
//! one transaction. Generic over the `varve-store` traits: tests run
//! it over `MemoryStore`, production scopes `platform-store` over
//! the same transaction the platform writes ride (the caller opens
//! it, shares its executor behind the async mutex, and commits after
//! this returns `Published`).
//!
//! Publication-time policy that is *platform*, not kernel: a choice
//! with no options is a legal draft state the kernel accepts, and
//! this is where it is refused (G.7); the draft's `base` against the
//! lineage head is the stale-fork check (P.9 Q15, settled).

use toasty::Executor;
use tokio::sync::Mutex;
use varve_core::{ColumnId, RevisionId};
use varve_impact::ImpactReport;
use varve_schema::{NomenclatureRef, ScalarType, revision_id};
use varve_service::{PublishOutcome, PublishRevision, PublishRevisionError};
use varve_store::{LineageId, RevisionStore, SurfaceStore};

use crate::procedure::Procedure;
use crate::procedure_event::{ProcedureEventKind, PublishedFacts, append_procedure_event};
use crate::procedure_state::{CorruptState, ProcedureState};
use crate::surfaces::compile_surfaces;
use crate::tree::{Tree, TreeDecodeError, TreeElement};

/// The executor a use case shares between its platform writes and
/// the kernel store scoped over the same transaction (P.3; the type
/// `platform-store` names `SharedExecutor` — spelled out here so
/// this crate depends on traits only).
pub type SharedExecutor<'t> = Mutex<&'t mut dyn Executor>;

/// What publication answered. Both answers carry the report *and*
/// the labels naming its entries (G.11.5): the use case is the one
/// place both schemas are in hand.
#[derive(Debug)]
pub enum PublishProcedureOutcome {
    /// Published: the kernel writes and every platform side effect
    /// are on the caller's transaction — commit it. The procedure row
    /// is updated in place.
    Published {
        revision: RevisionId,
        report: ImpactReport,
        labels: ColumnLabels,
    },
    /// The report exceeds `Safe` and the request did not confirm:
    /// nothing was written anywhere.
    RequiresConfirmation {
        report: ImpactReport,
        labels: ColumnLabels,
    },
}

/// Column labels for naming report entries (G.11.5): the next
/// schema's, the base schema filling in the columns the next no
/// longer holds — a `Removed` entry is named by the schema it was
/// removed *from*.
#[derive(Debug, Default)]
pub struct ColumnLabels(std::collections::HashMap<ColumnId, String>);

impl ColumnLabels {
    /// Resolves labels from the two sides of a classification; the
    /// next schema wins where both hold a column.
    pub fn resolve(base: Option<&varve_schema::Schema>, next: &varve_schema::Schema) -> Self {
        fn collect(
            elements: &[varve_schema::Element],
            into: &mut std::collections::HashMap<ColumnId, String>,
        ) {
            for element in elements {
                match element {
                    varve_schema::Element::Column(c) => {
                        into.insert(c.id.clone(), c.label.clone());
                    }
                    varve_schema::Element::Group(g) => collect(&g.children, into),
                }
            }
        }
        let mut labels = std::collections::HashMap::new();
        if let Some(base) = base {
            collect(&base.root, &mut labels);
        }
        collect(&next.root, &mut labels);
        Self(labels)
    }

    /// The label for a column, if either schema held it.
    pub fn get(&self, id: &ColumnId) -> Option<&str> {
        self.0.get(id).map(String::as_str)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PublishProcedureError {
    /// No revision draft in progress: nothing to publish.
    #[error("no revision draft in progress")]
    NoDraft,
    /// A choice with no options — a legal draft state, refused at
    /// publication (G.7).
    #[error("column '{0}' is a choice with no options")]
    EmptyEnum(ColumnId),
    /// The draft forked from a revision that is no longer the lineage
    /// head: another administrator published since. Discard or rebase
    /// the draft.
    #[error("the draft's base is no longer the published head")]
    StaleDraft,
    /// The stored draft no longer decodes (see
    /// [`crate::procedure::RevisionDraftError::Corrupt`]).
    #[error("stored draft is unreadable: {0}")]
    CorruptDraft(#[from] TreeDecodeError),
    /// The stored lifecycle state is unreadable.
    #[error(transparent)]
    CorruptState(#[from] CorruptState),
    /// The kernel edge refused — schema or surface validation, or a
    /// store failure. Authoring validates every edit, so validation
    /// failures here are wiring bugs, not user errors.
    #[error(transparent)]
    Kernel(varve_service::PublishRevisionError),
    /// Database failure on the platform side, including the
    /// optimistic-concurrency conflict (`condition_failed`) when the
    /// row changed since it was loaded.
    #[error(transparent)]
    Db(#[from] toasty::Error),
}

/// Publishes the procedure's revision draft: derive the schema from
/// the authored tree, compile the surface pair, run the impact-gated
/// kernel operation, and — when it publishes — transition the
/// lifecycle (`Published` from any state; from `Closed` this is the
/// reopen), consume the draft, set `latest_revision`, and log the
/// `published` event with its facts. The procedure must come from
/// `find_procedure_with_revision_draft`; on success it is updated in
/// place.
pub async fn publish_procedure<S>(
    exec: &SharedExecutor<'_>,
    store: &S,
    procedure: &mut Procedure,
    actor_account_id: uuid::Uuid,
    confirm: bool,
) -> Result<PublishProcedureOutcome, PublishProcedureError>
where
    S: RevisionStore + SurfaceStore,
{
    let (tree, base) = match procedure.revision_draft.get() {
        Some(draft) => (draft.tree.decode()?, draft.base.clone()),
        None => return Err(PublishProcedureError::NoDraft),
    };
    refuse_empty_enums(&tree)?;

    let schema = tree.schema();
    let revision = revision_id(&schema);
    let surfaces = compile_surfaces(&tree, &revision);

    let base_schema = match &base {
        Some(id) => store
            .schema(&RevisionId::new(id))
            .await
            .map_err(|error| PublishProcedureError::Kernel(error.into()))?,
        None => None,
    };
    let labels = ColumnLabels::resolve(base_schema.as_ref(), &schema);

    let outcome = varve_service::publish_revision(
        store,
        PublishRevision {
            lineage: LineageId::new(procedure.id.to_string()),
            base: base.as_deref().map(RevisionId::new),
            schema,
            surfaces: surfaces.into_vec(),
            confirm,
        },
    )
    .await
    .map_err(|error| match error {
        PublishRevisionError::StaleBase { .. } => PublishProcedureError::StaleDraft,
        other => PublishProcedureError::Kernel(other),
    })?;

    let report = match outcome {
        PublishOutcome::RequiresConfirmation { report } => {
            return Ok(PublishProcedureOutcome::RequiresConfirmation { report, labels });
        }
        PublishOutcome::Published { report, .. } => report,
    };

    let state = ProcedureState::from_columns(procedure.state, procedure.state_since)?
        .publish(jiff::Timestamp::now());
    let (value, since) = state.columns();
    let facts = PublishedFacts {
        revision: revision.as_str().to_owned(),
        base,
    };
    let mut guard = exec.lock().await;
    procedure
        .update()
        .state(value)
        .state_since(since)
        .latest_revision(Some(revision.as_str().to_owned()))
        .revision_draft(None)
        .published_tree(Some(crate::procedure::TreeBytes::encode(&tree)))
        .exec(&mut **guard)
        .await?;
    append_procedure_event(
        &mut **guard,
        procedure.id,
        Some(actor_account_id),
        ProcedureEventKind::Published,
        Some(&facts),
    )
    .await?;
    Ok(PublishProcedureOutcome::Published {
        revision,
        report,
        labels,
    })
}

/// The report `publish_procedure` would gate on, computable at any
/// time from a draft's derived schema against its base's (G.10,
/// *RevisionDraft.report*): the editor shows impact live without
/// attempting a publication. `None` base classifies against the
/// empty schema — the same one-code-path rule publication uses.
pub fn draft_report(
    base: Option<&varve_schema::Schema>,
    next: &varve_schema::Schema,
) -> Result<ImpactReport, varve_schema::CastError> {
    let empty = varve_schema::Schema::default();
    varve_impact::classify(
        base.unwrap_or(&empty),
        next,
        &varve_schema::NomenclatureTable::new(),
    )
}

/// G.7: the editor never seeds an option, an empty choice is a draft
/// state, and publication is where it is refused.
fn refuse_empty_enums(tree: &Tree) -> Result<(), PublishProcedureError> {
    fn walk(elements: &[TreeElement]) -> Result<(), PublishProcedureError> {
        for element in elements {
            match element {
                TreeElement::Column(c) => {
                    if let ScalarType::Enum(NomenclatureRef::Inline(rows)) = &c.ty
                        && rows.is_empty()
                    {
                        return Err(PublishProcedureError::EmptyEnum(c.id.clone()));
                    }
                }
                TreeElement::Group(g) => walk(&g.children)?,
                TreeElement::Section(s) => walk(&s.children)?,
                TreeElement::Note(_) => {}
            }
        }
        Ok(())
    }
    walk(&tree.elements)
}
