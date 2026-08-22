//! `Team` (full, root only) and `TeamRef`.

use async_graphql::{Context, ID, Object};

use crate::member::Member;
use crate::organization::OrganizationRef;
use crate::session;

/// The full team: scalars, the owning organization as a Ref, members.
/// Visible to the organization's members and to the team's own
/// members (G.6).
pub struct Team {
    pub team: platform_core::Team,
    pub organization: OrganizationRef,
}

#[Object]
impl Team {
    async fn id(&self) -> ID {
        ID::from(self.team.id)
    }

    async fn name(&self) -> &str {
        &self.team.name
    }

    async fn created_at(&self) -> jiff::Timestamp {
        self.team.created_at
    }

    async fn updated_at(&self) -> jiff::Timestamp {
        self.team.updated_at
    }

    /// The owning organization.
    async fn organization(&self) -> &OrganizationRef {
        &self.organization
    }

    /// The reviewers (P.4: team membership *is* being a reviewer).
    async fn members(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Member>> {
        let (_, mut db) = session(ctx)?;
        let members = platform_core::list_team_members(&mut db, self.team.id).await?;
        Ok(members.into_iter().map(Member).collect())
    }
}

/// A team as lists name it: scalars plus its ancestor Ref.
pub struct TeamRef {
    id: uuid::Uuid,
    name: String,
    organization: OrganizationRef,
}

impl TeamRef {
    pub fn new(team: platform_core::Team, organization: OrganizationRef) -> Self {
        Self {
            id: team.id,
            name: team.name,
            organization,
        }
    }
}

#[Object]
impl TeamRef {
    async fn id(&self) -> ID {
        ID::from(self.id)
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn organization(&self) -> &OrganizationRef {
        &self.organization
    }
}
