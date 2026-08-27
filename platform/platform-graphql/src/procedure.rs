//! `Procedure` (full, root only) and `ProcedureRef`. Catalog
//! scalars, the revision draft, and the lifecycle (G.9): the state
//! union (G.2 rule 5) with the event log on the full object, the
//! bare state enum on the Ref — published revisions and surfaces
//! still arrive with the kernel edge (P1).

use async_graphql::{Context, ID, Object};

use crate::error::internal;
use crate::organization::OrganizationRef;
use crate::procedure_event::{ProcedureEvent, procedure_event, procedure_events};
use crate::revision_draft::RevisionDraft;
use crate::session;

/// The full procedure. Visible to the owning organization's members
/// (G.6). Built from a row loaded **with its revision draft**
/// (`find_procedure_with_revision_draft`): the draft is deferred on
/// the catalog row, and only the full object shows it.
pub struct Procedure {
    pub procedure: platform_core::Procedure,
    pub organization: OrganizationRef,
}

#[Object]
impl Procedure {
    async fn id(&self) -> ID {
        ID::from(self.procedure.id)
    }

    async fn title(&self) -> &str {
        &self.procedure.title
    }

    /// Free text; empty when none.
    async fn description(&self) -> &str {
        &self.procedure.description
    }

    async fn created_at(&self) -> jiff::Timestamp {
        self.procedure.created_at
    }

    async fn updated_at(&self) -> jiff::Timestamp {
        self.procedure.updated_at
    }

    /// The lifecycle state (G.2 rule 5): a union of state-specific
    /// objects — facts live where they are meaningful, no nullable
    /// `since`.
    async fn state(&self) -> async_graphql::Result<ProcedureState> {
        procedure_state(&self.procedure)
    }

    /// The audit trail (G.9), oldest first: lifecycle transitions
    /// only, never authoring workflow — bounded by design.
    async fn events(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<ProcedureEvent>> {
        let (_, mut db) = session(ctx)?;
        procedure_events(&mut db, self.procedure.id).await
    }

    /// One trail entry (G.11.6) — the diff page's point lookup, so
    /// reading one publication's `report` never computes the others'.
    /// `null` for an id that is not this procedure's.
    async fn event(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> async_graphql::Result<Option<ProcedureEvent>> {
        let id = crate::parse_id(&id)?;
        let (_, mut db) = session(ctx)?;
        procedure_event(&mut db, self.procedure.id, id).await
    }

    /// The owning organization.
    async fn organization(&self) -> &OrganizationRef {
        &self.organization
    }

    /// All the procedure's case files, newest first, forward-only —
    /// the unbounded list, a connection (G.2.4, G.13). Reaching this
    /// object already required administering the procedure.
    async fn case_files(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> async_graphql::Result<crate::case_file::CaseFileConnection> {
        let (_, mut db) = session(ctx)?;
        let (limit, after) = crate::case_file::page_arguments(first, after)?;
        let page =
            platform_core::list_procedure_case_files(&mut db, self.procedure.id, after, limit)
                .await
                .map_err(internal)?;
        let procedure = ProcedureRef::of(&self.procedure, self.organization.clone());
        crate::case_file::connection_of(page, |_| Ok(procedure.clone()))
    }

    /// The draft of the next revision — *head until touched* (G.7
    /// virtual draft): the stored working buffer when one is in
    /// progress, otherwise the published head's tree (`base` naming
    /// the head), otherwise the empty tree. `inProgress` carries
    /// which; the first edit forks exactly this shape.
    async fn revision_draft(&self) -> async_graphql::Result<RevisionDraft> {
        if self.procedure.revision_draft.is_unloaded() {
            return Err(internal("procedure loaded without its revision draft"));
        }
        if self.procedure.preview.is_unloaded() {
            return Err(internal("procedure loaded without its preview"));
        }
        let platform_core::WorkingTree {
            tree,
            base,
            in_progress,
        } = platform_core::working_tree(&self.procedure).map_err(internal)?;
        // The preview bag as reads fold it (G.12): stale cells pruned
        // against the working tree's derived schema.
        let preview_values =
            platform_core::preview_values(&self.procedure, &tree.schema()).map_err(internal)?;
        Ok(RevisionDraft::new(
            self.procedure.id,
            base.as_deref(),
            tree,
            in_progress,
            preview_values,
        ))
    }
}

/// A procedure as lists name it: scalars plus its ancestor Ref. The
/// state rides as the bare enum (G.2 rule 5's parallel enum) — the
/// facts stay on the full object's union.
#[derive(Clone)]
pub struct ProcedureRef {
    id: uuid::Uuid,
    title: String,
    state: platform_core::ProcedureStateValue,
    organization: OrganizationRef,
}

impl ProcedureRef {
    pub fn new(procedure: platform_core::Procedure, organization: OrganizationRef) -> Self {
        Self::of(&procedure, organization)
    }

    /// Builds from a borrowed row — the case-file listings (G.13)
    /// name each row's procedure without consuming it.
    pub fn of(procedure: &platform_core::Procedure, organization: OrganizationRef) -> Self {
        Self {
            id: procedure.id,
            title: procedure.title.clone(),
            state: procedure.state,
            organization,
        }
    }
}

#[Object]
impl ProcedureRef {
    async fn id(&self) -> ID {
        ID::from(self.id)
    }

    async fn title(&self) -> &str {
        &self.title
    }

    /// The bare lifecycle state.
    async fn state(&self) -> ProcedureStateValue {
        self.state.into()
    }

    async fn organization(&self) -> &OrganizationRef {
        &self.organization
    }
}

/// The lifecycle state (G.2 rule 5): a union of state-specific
/// objects, subject-prefixed.
#[derive(async_graphql::Union)]
pub enum ProcedureState {
    Draft(ProcedureDraftState),
    Published(ProcedurePublishedState),
    Closed(ProcedureClosedState),
}

/// Never published. Its only fact is the row's creation time.
#[derive(async_graphql::SimpleObject)]
pub struct ProcedureDraftState {
    pub created_at: jiff::Timestamp,
}

/// Open for submissions.
#[derive(async_graphql::SimpleObject)]
pub struct ProcedurePublishedState {
    /// Since when the procedure is open — reset by reopening, so
    /// deliberately not `publishedAt`; revision publication dates
    /// live on revisions (platform P.4).
    pub since: jiff::Timestamp,
}

/// Closed to new submissions.
#[derive(async_graphql::SimpleObject)]
pub struct ProcedureClosedState {
    /// Since when the procedure is closed.
    pub since: jiff::Timestamp,
}

/// The bare state, for list rows and filters (G.2 rule 5's parallel
/// enum, generated from the platform-core discriminant).
#[derive(async_graphql::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[graphql(remote = "platform_core::ProcedureStateValue")]
pub enum ProcedureStateValue {
    Draft,
    Published,
    Closed,
}

/// The row's lifecycle columns as the union; a corrupt pair is an
/// internal error, never a guess.
pub(crate) fn procedure_state(
    procedure: &platform_core::Procedure,
) -> async_graphql::Result<ProcedureState> {
    Ok(
        match platform_core::current_state(procedure).map_err(internal)? {
            platform_core::ProcedureState::Draft => ProcedureState::Draft(ProcedureDraftState {
                created_at: procedure.created_at,
            }),
            platform_core::ProcedureState::Published { since } => {
                ProcedureState::Published(ProcedurePublishedState { since })
            }
            platform_core::ProcedureState::Closed { since } => {
                ProcedureState::Closed(ProcedureClosedState { since })
            }
        },
    )
}
