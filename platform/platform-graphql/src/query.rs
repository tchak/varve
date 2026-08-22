//! The query root: `viewer` plus the G.6 root lookups and lists.
//!
//! Every lookup resolves visibility **once, here** (G.2 rule 1:
//! "one surface resolution per root object" — membership is P0's
//! stand-in for surfaces) and answers `null` for absent-or-invisible.

use std::collections::HashMap;

use async_graphql::{Context, ID, Object};
use platform_core::Principal;

use crate::organization::{Organization, OrganizationRef};
use crate::procedure::{Procedure, ProcedureRef};
use crate::team::Team;
use crate::{parse_id, session};

/// The query root.
pub struct Query;

#[Object]
impl Query {
    /// The authenticated account the request executes as.
    async fn viewer<'a>(&self, ctx: &Context<'a>) -> async_graphql::Result<Viewer<'a>> {
        let (principal, _) = session(ctx)?;
        Ok(Viewer { principal })
    }

    /// An organization the viewer is a member of; `null` otherwise.
    async fn organization(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> async_graphql::Result<Option<Organization>> {
        let (principal, mut db) = session(ctx)?;
        let id = parse_id(&id)?;
        if !platform_core::is_organization_member(&mut db, id, principal.account_id).await? {
            return Ok(None);
        }
        Ok(platform_core::find_organization(&mut db, id)
            .await?
            .map(Organization))
    }

    /// The organizations the viewer is a member of, oldest first.
    async fn organizations(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<OrganizationRef>> {
        let (principal, mut db) = session(ctx)?;
        let organizations =
            platform_core::list_account_organizations(&mut db, principal.account_id).await?;
        Ok(organizations.iter().map(OrganizationRef::from).collect())
    }

    /// A team of an organization the viewer is a member of, or a team
    /// the viewer reviews for; `null` otherwise.
    async fn team(&self, ctx: &Context<'_>, id: ID) -> async_graphql::Result<Option<Team>> {
        let (principal, mut db) = session(ctx)?;
        let id = parse_id(&id)?;
        let Some(team) = platform_core::find_team(&mut db, id).await? else {
            return Ok(None);
        };
        let visible = platform_core::is_team_member(&mut db, team.id, principal.account_id).await?
            || platform_core::is_organization_member(
                &mut db,
                team.organization_id,
                principal.account_id,
            )
            .await?;
        if !visible {
            return Ok(None);
        }
        let Some(organization) =
            platform_core::find_organization(&mut db, team.organization_id).await?
        else {
            return Ok(None);
        };
        Ok(Some(Team {
            team,
            organization: OrganizationRef::from(&organization),
        }))
    }

    /// A procedure owned by an organization the viewer is a member
    /// of; `null` otherwise.
    async fn procedure(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> async_graphql::Result<Option<Procedure>> {
        let (principal, mut db) = session(ctx)?;
        let id = parse_id(&id)?;
        let Some(procedure) = platform_core::find_procedure(&mut db, id).await? else {
            return Ok(None);
        };
        if !platform_core::is_organization_member(
            &mut db,
            procedure.organization_id,
            principal.account_id,
        )
        .await?
        {
            return Ok(None);
        }
        let Some(organization) =
            platform_core::find_organization(&mut db, procedure.organization_id).await?
        else {
            return Ok(None);
        };
        Ok(Some(Procedure {
            procedure,
            organization: OrganizationRef::from(&organization),
        }))
    }

    /// Every procedure the viewer administers — those owned by the
    /// viewer's organizations — oldest first.
    async fn procedures(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<ProcedureRef>> {
        let (principal, mut db) = session(ctx)?;
        // The viewer's organizations once, then one join in memory:
        // no query per procedure.
        let organizations: HashMap<uuid::Uuid, OrganizationRef> =
            platform_core::list_account_organizations(&mut db, principal.account_id)
                .await?
                .iter()
                .map(|o| (o.id, OrganizationRef::from(o)))
                .collect();
        let procedures =
            platform_core::list_account_procedures(&mut db, principal.account_id).await?;
        Ok(procedures
            .into_iter()
            .filter_map(|procedure| {
                let organization = organizations.get(&procedure.organization_id)?.clone();
                Some(ProcedureRef::new(procedure, organization))
            })
            .collect())
    }
}

/// The principal as the schema exposes it (P0: the account-level
/// core — id, email, locale preference).
pub struct Viewer<'a> {
    principal: &'a Principal,
}

#[Object]
impl Viewer<'_> {
    /// The account id.
    async fn account_id(&self) -> ID {
        ID::from(self.principal.account_id)
    }

    /// The account's normalized email.
    async fn email(&self) -> &str {
        &self.principal.email
    }

    /// The account's locale preference, when one was chosen.
    async fn locale(&self) -> Option<&str> {
        self.principal.locale.as_deref()
    }
}
