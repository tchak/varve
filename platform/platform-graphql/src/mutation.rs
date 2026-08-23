//! The mutation root (G.2.7): one `input` each, verb-first, the full
//! object returned, errors structural ([`crate::error`]).

use async_graphql::{Context, ID, InputObject, Object};
use platform_core::{
    ColumnPatch, CreateOrganizationError, EditError, GroupPatch, RevisionDraftError,
};
use varve_schema::Schema;

use crate::error::{Code, coded, forbidden, internal, invalid_input};
use crate::organization::{Organization, OrganizationRef};
use crate::procedure::Procedure;
use crate::revision_draft::{
    Arity, Cardinality, ColumnTypeInput, PlacementInput, element_id, new_column, new_group,
};
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

/// `addColumn` input.
#[derive(InputObject)]
pub struct AddColumnInput {
    /// The procedure whose revision draft is edited; the viewer must
    /// administer it.
    pub procedure_id: ID,
    /// Where the column goes; omitted = appended at the root.
    #[graphql(default)]
    pub placement: PlacementInput,
    pub label: String,
    #[graphql(name = "type")]
    pub ty: ColumnTypeInput,
    #[graphql(default_with = "Arity::One")]
    pub arity: Arity,
}

/// `addGroup` input.
#[derive(InputObject)]
pub struct AddGroupInput {
    pub procedure_id: ID,
    /// Where the group goes; omitted = appended at the root.
    #[graphql(default)]
    pub placement: PlacementInput,
    pub label: String,
    #[graphql(default_with = "Cardinality::One")]
    pub cardinality: Cardinality,
}

/// `updateColumn` input: an omitted field is left as it is.
#[derive(InputObject)]
pub struct UpdateColumnInput {
    pub procedure_id: ID,
    pub id: ID,
    pub label: Option<String>,
    #[graphql(name = "type")]
    pub ty: Option<ColumnTypeInput>,
    pub arity: Option<Arity>,
}

/// `updateGroup` input: an omitted field is left as it is.
#[derive(InputObject)]
pub struct UpdateGroupInput {
    pub procedure_id: ID,
    pub id: ID,
    pub label: Option<String>,
    pub cardinality: Option<Cardinality>,
}

/// `moveElement` input.
#[derive(InputObject)]
pub struct MoveElementInput {
    pub procedure_id: ID,
    /// A column or a group (with its subtree).
    pub id: ID,
    pub placement: PlacementInput,
}

/// `removeElement` input.
#[derive(InputObject)]
pub struct RemoveElementInput {
    pub procedure_id: ID,
    /// A column or a group (with its subtree).
    pub id: ID,
}

/// `discardRevisionDraft` input.
#[derive(InputObject)]
pub struct DiscardRevisionDraftInput {
    pub procedure_id: ID,
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

/// Resolves the procedure `id` for a mutation the viewer must
/// administer (be a member of the owning organization), loaded with
/// its revision draft; a missing procedure and a foreign one are the
/// same `FORBIDDEN`.
async fn administered_procedure(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    id: &ID,
) -> async_graphql::Result<(platform_core::Procedure, platform_core::Organization)> {
    let id = parse_id(id)?;
    let procedure = platform_core::find_procedure_with_revision_draft(db, id)
        .await
        .map_err(internal)?
        .ok_or_else(forbidden)?;
    if !platform_core::is_organization_member(db, procedure.organization_id, account_id)
        .await
        .map_err(internal)?
    {
        return Err(forbidden());
    }
    let organization = platform_core::find_organization(db, procedure.organization_id)
        .await
        .map_err(internal)?
        .ok_or_else(forbidden)?;
    Ok((procedure, organization))
}

/// The shape every draft mutation shares: resolve the procedure,
/// apply one edit to its draft, answer with the procedure as stored.
async fn edit_revision_draft(
    ctx: &Context<'_>,
    procedure_id: &ID,
    edit: impl FnOnce(&mut Schema) -> Result<(), EditError>,
) -> async_graphql::Result<Procedure> {
    let (principal, mut db) = session(ctx)?;
    let (mut procedure, organization) =
        administered_procedure(&mut db, principal.account_id, procedure_id).await?;
    platform_core::edit_revision_draft(&mut db, &mut procedure, edit)
        .await
        .map_err(draft_error)?;
    Ok(Procedure {
        procedure,
        organization: OrganizationRef::from(&organization),
    })
}

fn draft_error(error: RevisionDraftError) -> async_graphql::Error {
    match error {
        RevisionDraftError::Edit(e) => coded(Code::InvalidEdit, e.to_string()),
        RevisionDraftError::Db(e) if e.is_condition_failed() => coded(
            Code::Conflict,
            "the revision draft changed since it was read; re-read and retry",
        ),
        RevisionDraftError::Db(e) => internal(e),
        RevisionDraftError::Corrupt(e) => internal(e),
    }
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

    /// Adds a column to the procedure's revision draft (starting the
    /// draft when none is in progress). The new column is the element
    /// in front of `placement.beforeId` in its parent — or the parent's
    /// last child when `beforeId` is omitted.
    async fn add_column(
        &self,
        ctx: &Context<'_>,
        input: AddColumnInput,
    ) -> async_graphql::Result<Procedure> {
        validate_non_empty("label", &input.label)?;
        let ty = input.ty.into_scalar_type()?;
        let label = input.label.trim().to_owned();
        edit_revision_draft(ctx, &input.procedure_id, move |schema| {
            let placement = input.placement.resolve(schema)?;
            platform_core::add_element(schema, &placement, new_column(label, ty, input.arity))
        })
        .await
    }

    /// Adds an empty group to the procedure's revision draft; placed
    /// like `addColumn`.
    async fn add_group(
        &self,
        ctx: &Context<'_>,
        input: AddGroupInput,
    ) -> async_graphql::Result<Procedure> {
        validate_non_empty("label", &input.label)?;
        let label = input.label.trim().to_owned();
        edit_revision_draft(ctx, &input.procedure_id, move |schema| {
            let placement = input.placement.resolve(schema)?;
            platform_core::add_element(schema, &placement, new_group(label, input.cardinality))
        })
        .await
    }

    /// Changes a column's label, type, or arity; its id — its identity
    /// for the impact report — never changes.
    async fn update_column(
        &self,
        ctx: &Context<'_>,
        input: UpdateColumnInput,
    ) -> async_graphql::Result<Procedure> {
        if let Some(label) = &input.label {
            validate_non_empty("label", label)?;
        }
        let patch = ColumnPatch {
            label: input.label.map(|l| l.trim().to_owned()),
            ty: input
                .ty
                .map(ColumnTypeInput::into_scalar_type)
                .transpose()?,
            arity: input.arity.map(Into::into),
        };
        edit_revision_draft(ctx, &input.procedure_id, move |schema| {
            let id = match element_id(schema, &input.id)? {
                platform_core::ElementId::Column(id) => id,
                platform_core::ElementId::Group(id) => {
                    return Err(EditError::UnknownElement(platform_core::ElementId::Column(
                        varve_core::ColumnId::new(id.as_str()),
                    )));
                }
            };
            platform_core::update_column(schema, &id, patch)
        })
        .await
    }

    /// Changes a group's label or cardinality; its id and children
    /// stay.
    async fn update_group(
        &self,
        ctx: &Context<'_>,
        input: UpdateGroupInput,
    ) -> async_graphql::Result<Procedure> {
        if let Some(label) = &input.label {
            validate_non_empty("label", label)?;
        }
        let patch = GroupPatch {
            label: input.label.map(|l| l.trim().to_owned()),
            cardinality: input.cardinality.map(Into::into),
        };
        edit_revision_draft(ctx, &input.procedure_id, move |schema| {
            let id = match element_id(schema, &input.id)? {
                platform_core::ElementId::Group(id) => id,
                platform_core::ElementId::Column(id) => {
                    return Err(EditError::UnknownElement(platform_core::ElementId::Group(
                        varve_core::GroupId::new(id.as_str()),
                    )));
                }
            };
            platform_core::update_group(schema, &id, patch)
        })
        .await
    }

    /// Moves a column or a group (with its subtree) to `placement`.
    async fn move_element(
        &self,
        ctx: &Context<'_>,
        input: MoveElementInput,
    ) -> async_graphql::Result<Procedure> {
        edit_revision_draft(ctx, &input.procedure_id, move |schema| {
            let id = element_id(schema, &input.id)?;
            let placement = input.placement.resolve(schema)?;
            platform_core::move_element(schema, &id, &placement)
        })
        .await
    }

    /// Removes a column or a group (with its subtree) from the draft.
    async fn remove_element(
        &self,
        ctx: &Context<'_>,
        input: RemoveElementInput,
    ) -> async_graphql::Result<Procedure> {
        edit_revision_draft(ctx, &input.procedure_id, move |schema| {
            let id = element_id(schema, &input.id)?;
            platform_core::remove_element(schema, &id).map(|_| ())
        })
        .await
    }

    /// Drops the procedure's revision draft; `revisionDraft` is `null`
    /// afterwards.
    async fn discard_revision_draft(
        &self,
        ctx: &Context<'_>,
        input: DiscardRevisionDraftInput,
    ) -> async_graphql::Result<Procedure> {
        let (principal, mut db) = session(ctx)?;
        let (mut procedure, organization) =
            administered_procedure(&mut db, principal.account_id, &input.procedure_id).await?;
        platform_core::discard_revision_draft(&mut db, &mut procedure)
            .await
            .map_err(|e| draft_error(RevisionDraftError::Db(e)))?;
        Ok(Procedure {
            procedure,
            organization: OrganizationRef::from(&organization),
        })
    }
}
