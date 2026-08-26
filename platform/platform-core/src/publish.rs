//! The publication use case (P.4 *Publication*): the one place the
//! `varve-service` operation composes with its platform side effects
//! — the lifecycle transition, the draft consumed, `latest_publication`
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
use varve_core::{ColumnId, PublicationId, RevisionId};
use varve_impact::ImpactReport;
use varve_schema::{NomenclatureRef, ScalarType, revision_id};
use varve_service::{PublishOutcome, PublishRevision, PublishRevisionError};
use varve_store::{LineageId, RevisionStore, SurfaceStore};
use varve_surface::SurfaceReport;

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

/// What publication answered. Both answers carry both halves of the
/// impact story (§3.1) — the schema report with the labels naming
/// its entries (G.11.5), and the surface report — the use case being
/// the one place schemas and surfaces are all in hand.
#[derive(Debug)]
pub enum PublishProcedureOutcome {
    /// Published: the kernel writes and every platform side effect
    /// are on the caller's transaction — commit it. The procedure row
    /// is updated in place.
    Published {
        publication: PublicationId,
        revision: RevisionId,
        report: ImpactReport,
        surface_report: SurfaceReport,
        labels: ColumnLabels,
    },
    /// The worst class of the report pair exceeds `Safe` and the
    /// request did not confirm: nothing was written anywhere.
    RequiresConfirmation {
        report: ImpactReport,
        surface_report: SurfaceReport,
        labels: ColumnLabels,
    },
}

/// Column and group labels for naming report entries (G.11.5): the
/// next schema's, the base schema filling in what the next no longer
/// holds — a `Removed` entry is named by the schema it was removed
/// *from*. Groups joined for the surface report's entries (§3.1: a
/// group prompt change is named by the group's schema label).
#[derive(Debug, Default)]
pub struct ColumnLabels {
    columns: std::collections::HashMap<ColumnId, String>,
    groups: std::collections::HashMap<varve_core::GroupId, String>,
}

impl ColumnLabels {
    /// Resolves labels from the two sides of a classification; the
    /// next schema wins where both hold an element.
    pub fn resolve(base: Option<&varve_schema::Schema>, next: &varve_schema::Schema) -> Self {
        fn collect(
            elements: &[varve_schema::Element],
            columns: &mut std::collections::HashMap<ColumnId, String>,
            groups: &mut std::collections::HashMap<varve_core::GroupId, String>,
        ) {
            for element in elements {
                match element {
                    varve_schema::Element::Column(c) => {
                        columns.insert(c.id.clone(), c.label.clone());
                    }
                    varve_schema::Element::Group(g) => {
                        groups.insert(g.id.clone(), g.label.clone());
                        collect(&g.children, columns, groups);
                    }
                }
            }
        }
        let mut columns = std::collections::HashMap::new();
        let mut groups = std::collections::HashMap::new();
        if let Some(base) = base {
            collect(&base.root, &mut columns, &mut groups);
        }
        collect(&next.root, &mut columns, &mut groups);
        Self { columns, groups }
    }

    /// The label for a column, if either schema held it.
    pub fn get(&self, id: &ColumnId) -> Option<&str> {
        self.columns.get(id).map(String::as_str)
    }

    /// The label for a group, if either schema held it.
    pub fn get_group(&self, id: &varve_core::GroupId) -> Option<&str> {
        self.groups.get(id).map(String::as_str)
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
    /// The draft forked from a publication that is no longer the
    /// lineage head: another administrator published since — a schema
    /// change or a surface-only one alike (§2.13 decision 9). Discard
    /// or rebase the draft.
    #[error("the draft's base is no longer the published head")]
    StaleDraft,
    /// The draft changes nothing — same schema, same surfaces as the
    /// head (§2.13 decision 9's refused no-op).
    #[error("the draft changes nothing — nothing to publish")]
    NothingToPublish,
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
/// reopen), consume the draft, set `latest_publication`, and log the
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
    let lineage = LineageId::new(procedure.id.to_string());

    // The base *schema*, for naming report entries: publications
    // carry their schema, and the base is a publication id (§2.13
    // decision 9). An unknown base falls through — the kernel's
    // stale-fork check answers it.
    let base_schema = match &base {
        Some(id) => store
            .publications(&lineage)
            .await
            .map_err(|error| PublishProcedureError::Kernel(error.into()))?
            .into_iter()
            .find(|(publication, _)| publication.id().as_str() == id)
            .map(|(_, schema)| schema),
        None => None,
    };
    let labels = ColumnLabels::resolve(base_schema.as_ref(), &schema);

    let outcome = varve_service::publish_revision(
        store,
        PublishRevision {
            lineage,
            base: base.as_deref().map(PublicationId::new),
            schema,
            surfaces: surfaces.into_vec(),
            confirm,
        },
    )
    .await
    .map_err(|error| match error {
        PublishRevisionError::StaleBase { .. } => PublishProcedureError::StaleDraft,
        PublishRevisionError::NothingToPublish => PublishProcedureError::NothingToPublish,
        other => PublishProcedureError::Kernel(other),
    })?;

    let (publication, report, surface_report) = match outcome {
        PublishOutcome::RequiresConfirmation {
            report,
            surface_report,
        } => {
            return Ok(PublishProcedureOutcome::RequiresConfirmation {
                report,
                surface_report,
                labels,
            });
        }
        PublishOutcome::Published {
            publication,
            report,
            surface_report,
            ..
        } => (publication, report, surface_report),
    };

    let state = ProcedureState::from_columns(procedure.state, procedure.state_since)?
        .publish(crate::procedure::stored_now());
    let (value, since) = state.columns();
    let facts = PublishedFacts {
        publication: publication.as_str().to_owned(),
        base,
    };
    let mut guard = exec.lock().await;
    procedure
        .update()
        .state(value)
        .state_since(since)
        .latest_publication(Some(publication.as_str().to_owned()))
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
        publication,
        revision,
        report,
        surface_report,
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

/// The surface half of the pair (§3.1), computable at any time from
/// a draft's compiled pair against the base publication's stored
/// surfaces — empty `base` (a first publication) reports the initial
/// surface list, the same one-code-path rule as [`draft_report`].
pub fn draft_surface_report(
    base: &[varve_surface::Surface],
    tree: &Tree,
) -> varve_surface::SurfaceReport {
    let schema = tree.schema();
    let revision = revision_id(&schema);
    varve_surface::diff_sets(base, &compile_surfaces(tree, &revision).into_vec())
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
