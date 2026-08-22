//! `Procedure` (full, root only) and `ProcedureRef`. Catalog scalars
//! only for now — revisions, surfaces, and the lifecycle arrive with
//! the kernel edge (P1).

use async_graphql::{ID, Object};

use crate::organization::OrganizationRef;

/// The full procedure. Visible to the owning organization's members
/// (G.6).
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
