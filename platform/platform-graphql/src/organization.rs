//! `Organization` (full, root only) and `OrganizationRef`.

use async_graphql::{Context, ID, Object};

use crate::member::Member;
use crate::procedure::ProcedureRef;
use crate::session;
use crate::team::TeamRef;

/// The full organization: scalars, bounded child lists as Refs,
/// members, counts. Only a member sees it (G.6).
pub struct Organization(pub platform_core::Organization);

#[Object]
impl Organization {
    async fn id(&self) -> ID {
        ID::from(self.0.id)
    }

    /// Normalized URL/API handle, unique across the platform.
    async fn slug(&self) -> &str {
        &self.0.slug
    }

    async fn name(&self) -> &str {
        &self.0.name
    }

    async fn created_at(&self) -> jiff::Timestamp {
        self.0.created_at
    }

    async fn updated_at(&self) -> jiff::Timestamp {
        self.0.updated_at
    }

    /// The organization's teams, oldest first (bounded, G.2.4).
    async fn teams(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<TeamRef>> {
        let (_, mut db) = session(ctx)?;
        let teams = platform_core::list_organization_teams(&mut db, self.0.id).await?;
        Ok(teams
            .into_iter()
            .map(|team| TeamRef::new(team, OrganizationRef::from(&self.0)))
            .collect())
    }

    /// The procedures the organization owns, oldest first.
    async fn procedures(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<ProcedureRef>> {
        let (_, mut db) = session(ctx)?;
        let procedures = platform_core::list_organization_procedures(&mut db, self.0.id).await?;
        Ok(procedures
            .into_iter()
            .map(|procedure| ProcedureRef::new(procedure, OrganizationRef::from(&self.0)))
            .collect())
    }

    /// The members — every one administers every procedure the
    /// organization owns (P.4).
    async fn members(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Member>> {
        let (_, mut db) = session(ctx)?;
        let members = platform_core::list_organization_members(&mut db, self.0.id).await?;
        Ok(members.into_iter().map(Member).collect())
    }

    /// Counts without fetching the lists (G.2.6).
    async fn counts(&self, ctx: &Context<'_>) -> async_graphql::Result<OrganizationCounts> {
        let (_, mut db) = session(ctx)?;
        Ok(OrganizationCounts {
            procedures: platform_core::count_organization_procedures(&mut db, self.0.id).await?,
            teams: platform_core::count_organization_teams(&mut db, self.0.id).await?,
            members: platform_core::count_organization_members(&mut db, self.0.id).await?,
        })
    }
}

/// `organization.counts`.
#[derive(async_graphql::SimpleObject)]
pub struct OrganizationCounts {
    pub procedures: u64,
    pub teams: u64,
    pub members: u64,
}

/// An organization as lists and parents name it: scalars only.
#[derive(Clone)]
pub struct OrganizationRef {
    id: uuid::Uuid,
    slug: String,
    name: String,
}

impl From<&platform_core::Organization> for OrganizationRef {
    fn from(organization: &platform_core::Organization) -> Self {
        Self {
            id: organization.id,
            slug: organization.slug.clone(),
            name: organization.name.clone(),
        }
    }
}

#[Object]
impl OrganizationRef {
    async fn id(&self) -> ID {
        ID::from(self.id)
    }

    async fn slug(&self) -> &str {
        &self.slug
    }

    async fn name(&self) -> &str {
        &self.name
    }
}
