//! The mutation root (G.2.7): one `input` each, verb-first, the full
//! object returned, errors structural ([`crate::error`]).

use async_graphql::{Context, ID, InputObject, Object};
use platform_core::CreateOrganizationError;

use crate::error::{Code, coded, forbidden, internal, invalid_input};
use crate::organization::{Organization, OrganizationRef};
use crate::procedure::Procedure;
use crate::slug::Slug;
use crate::team::Team;
use crate::{parse_id, session};

/// The mutation root.
pub struct Mutation;

/// `createOrganization` input.
#[derive(InputObject)]
pub struct CreateOrganizationInput {
    /// URL/API handle; normalized and validated by the scalar.
    pub slug: Slug,
    /// Display name.
    pub name: String,
}

/// `createTeam` input.
#[derive(InputObject)]
pub struct CreateTeamInput {
    /// The owning organization; the viewer must be a member.
    pub organization_id: ID,
    /// Display name.
    pub name: String,
}

/// `createProcedure` input.
#[derive(InputObject)]
pub struct CreateProcedureInput {
    /// The owning organization; the viewer must be a member.
    pub organization_id: ID,
    /// Title shown to applicants and reviewers.
    pub title: String,
    /// Free-text description.
    #[graphql(default)]
    pub description: String,
}

fn validate_non_empty(field: &str, value: &str) -> async_graphql::Result<()> {
    if value.trim().is_empty() {
        Err(invalid_input(format!("{field} must not be empty")))
    } else {
        Ok(())
    }
}

/// Resolves the organization `id` for a mutation the viewer must be a
/// member of: a missing organization and a foreign one are the same
/// `FORBIDDEN`.
async fn administered_organization(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    id: &ID,
) -> async_graphql::Result<platform_core::Organization> {
    let id = parse_id(id)?;
    if !platform_core::is_organization_member(db, id, account_id)
        .await
        .map_err(internal)?
    {
        return Err(forbidden());
    }
    platform_core::find_organization(db, id)
        .await
        .map_err(internal)?
        .ok_or_else(forbidden)
}

#[Object]
impl Mutation {
    /// Creates an organization; the viewer becomes its first member.
    async fn create_organization(
        &self,
        ctx: &Context<'_>,
        input: CreateOrganizationInput,
    ) -> async_graphql::Result<Organization> {
        let (principal, mut db) = session(ctx)?;
        validate_non_empty("name", &input.name)?;
        match platform_core::create_organization_for(
            &mut db,
            input.slug.as_str(),
            &input.name,
            principal.account_id,
        )
        .await
        {
            Ok(organization) => Ok(Organization(organization)),
            Err(CreateOrganizationError::SlugTaken) => Err(coded(
                Code::SlugTaken,
                "an organization with this slug already exists",
            )),
            Err(CreateOrganizationError::Db(e)) => Err(internal(e)),
        }
    }

    /// Creates a team in an organization the viewer is a member of.
    async fn create_team(
        &self,
        ctx: &Context<'_>,
        input: CreateTeamInput,
    ) -> async_graphql::Result<Team> {
        let (principal, mut db) = session(ctx)?;
        validate_non_empty("name", &input.name)?;
        let organization =
            administered_organization(&mut db, principal.account_id, &input.organization_id)
                .await?;
        let team = platform_core::create_team(&mut db, organization.id, &input.name)
            .await
            .map_err(internal)?;
        Ok(Team {
            team,
            organization: OrganizationRef::from(&organization),
        })
    }

    /// Creates a procedure owned by an organization the viewer is a
    /// member of.
    async fn create_procedure(
        &self,
        ctx: &Context<'_>,
        input: CreateProcedureInput,
    ) -> async_graphql::Result<Procedure> {
        let (principal, mut db) = session(ctx)?;
        validate_non_empty("title", &input.title)?;
        let organization =
            administered_organization(&mut db, principal.account_id, &input.organization_id)
                .await?;
        let procedure = platform_core::create_procedure(
            &mut db,
            organization.id,
            &input.title,
            &input.description,
        )
        .await
        .map_err(internal)?;
        Ok(Procedure {
            procedure,
            organization: OrganizationRef::from(&organization),
        })
    }
}
