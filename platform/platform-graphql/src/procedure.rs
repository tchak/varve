//! `Procedure` (full, root only) and `ProcedureRef`. Catalog scalars
//! and the revision draft — published revisions, surfaces, and the
//! lifecycle arrive with the kernel edge (P1).

use async_graphql::{ID, Object};

use crate::error::internal;
use crate::organization::OrganizationRef;
use crate::revision_draft::RevisionDraft;

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

    /// The owning organization.
    async fn organization(&self) -> &OrganizationRef {
        &self.organization
    }

    /// The draft of the next revision; `null` when none is in
    /// progress.
    async fn revision_draft(&self) -> async_graphql::Result<Option<RevisionDraft>> {
        if self.procedure.revision_draft.is_unloaded() {
            return Err(internal("procedure loaded without its revision draft"));
        }
        let tree = platform_core::revision_draft_tree(&self.procedure).map_err(internal)?;
        Ok(tree.map(|tree| {
            let base = self
                .procedure
                .revision_draft
                .get()
                .as_ref()
                .and_then(|draft| draft.base.as_deref());
            RevisionDraft::new(base, &tree)
        }))
    }
}

/// A procedure as lists name it: scalars plus its ancestor Ref.
pub struct ProcedureRef {
    id: uuid::Uuid,
    title: String,
    organization: OrganizationRef,
}

impl ProcedureRef {
    pub fn new(procedure: platform_core::Procedure, organization: OrganizationRef) -> Self {
        Self {
            id: procedure.id,
            title: procedure.title,
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

    async fn organization(&self) -> &OrganizationRef {
        &self.organization
    }
}
