//! The mutation root (G.2.7): one `input` each, verb-first, the full
//! object returned, errors structural ([`crate::error`]).

use async_graphql::{Context, ID, InputObject, MaybeUndefined, Object};
use platform_core::{
    ColumnPatch, CreateOrganizationError, EditError, GroupPatch, LifecycleError, NotePatch,
    PublishProcedureError, PublishProcedureOutcome, RevisionDraftError, SectionPatch, Tree,
};

use crate::error::{Code, coded, forbidden, internal, invalid_input};
use crate::impact::ImpactReport;
use crate::organization::{Organization, OrganizationRef};
use crate::procedure::Procedure;
use crate::revision_draft::{
    Audience, Cardinality, ColumnTypeInput, PlacementInput, element_id, new_column, new_group,
    new_note, new_section,
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

/// `closeProcedure` input.
#[derive(InputObject)]
pub struct CloseProcedureInput {
    /// The procedure; the viewer must administer it.
    pub procedure_id: ID,
}

/// `reopenProcedure` input.
#[derive(InputObject)]
pub struct ReopenProcedureInput {
    /// The procedure; the viewer must administer it.
    pub procedure_id: ID,
}

/// `publishRevision` input.
#[derive(InputObject)]
pub struct PublishRevisionInput {
    /// The procedure whose revision draft publishes; the viewer must
    /// administer it.
    pub procedure_id: ID,
    /// Accept a report worse than `SAFE` (G.10): without it such a
    /// report is returned with `published: false` and nothing is
    /// written.
    #[graphql(default)]
    pub confirm: bool,
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
    /// Omitted = required for an effectively public column, optional
    /// for a reviewer-only one (G.7 *Required on columns*).
    pub required: Option<bool>,
    /// Clamped to the parent's effective audience (P.4).
    #[graphql(default_with = "Audience::All")]
    pub audience: Audience,
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
    /// Clamped to the parent's effective audience (P.4).
    #[graphql(default_with = "Audience::All")]
    pub audience: Audience,
}

/// `addSection` input.
#[derive(InputObject)]
pub struct AddSectionInput {
    pub procedure_id: ID,
    /// Where the section goes; omitted = appended at the root.
    #[graphql(default)]
    pub placement: PlacementInput,
    pub title: String,
    /// Help text under the title; omitted or blank = none.
    pub help: Option<String>,
    /// Clamped to the parent's effective audience (P.4).
    #[graphql(default_with = "Audience::All")]
    pub audience: Audience,
}

/// `addNote` input.
#[derive(InputObject)]
pub struct AddNoteInput {
    pub procedure_id: ID,
    /// Where the note goes; omitted = appended at the root.
    #[graphql(default)]
    pub placement: PlacementInput,
    /// Heading; omitted or blank = none.
    pub title: Option<String>,
    pub body: String,
    /// Clamped to the parent's effective audience (P.4).
    #[graphql(default_with = "Audience::All")]
    pub audience: Audience,
}

/// `updateColumn` input: an omitted field is left as it is.
#[derive(InputObject)]
pub struct UpdateColumnInput {
    pub procedure_id: ID,
    pub id: ID,
    pub label: Option<String>,
    #[graphql(name = "type")]
    pub ty: Option<ColumnTypeInput>,
    /// Always required, or not required (G.7); omitted leaves it.
    pub required: Option<bool>,
    /// Wider than the parent's effective audience is `INVALID_EDIT`
    /// (P.4: the marker would lie).
    pub audience: Option<Audience>,
}

/// `updateGroup` input: an omitted field is left as it is.
#[derive(InputObject)]
pub struct UpdateGroupInput {
    pub procedure_id: ID,
    pub id: ID,
    pub label: Option<String>,
    pub cardinality: Option<Cardinality>,
    /// Wider than the parent's effective audience is `INVALID_EDIT`.
    pub audience: Option<Audience>,
}

/// `updateSection` input: an omitted field is left as it is.
#[derive(InputObject)]
pub struct UpdateSectionInput {
    pub procedure_id: ID,
    pub id: ID,
    pub title: Option<String>,
    /// `null` or blank clears the help; omitted leaves it.
    pub help: MaybeUndefined<String>,
    /// Wider than the parent's effective audience is `INVALID_EDIT`.
    pub audience: Option<Audience>,
}

/// `updateNote` input: an omitted field is left as it is.
#[derive(InputObject)]
pub struct UpdateNoteInput {
    pub procedure_id: ID,
    pub id: ID,
    /// `null` or blank clears the title; omitted leaves it.
    pub title: MaybeUndefined<String>,
    pub body: Option<String>,
    /// Wider than the parent's effective audience is `INVALID_EDIT`.
    pub audience: Option<Audience>,
}

/// `moveElement` input.
#[derive(InputObject)]
pub struct MoveElementInput {
    pub procedure_id: ID,
    /// Any element (a group or section with its subtree).
    pub id: ID,
    pub placement: PlacementInput,
}

/// `removeElement` input.
#[derive(InputObject)]
pub struct RemoveElementInput {
    pub procedure_id: ID,
    /// Any element (a group or section with its subtree).
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

/// An optional text on input: trimmed, blank collapsing to none.
fn optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

/// A clearable text on update: omitted leaves the field, `null` or
/// blank clears it, text sets it (trimmed).
fn clearable_text(value: MaybeUndefined<String>) -> Option<Option<String>> {
    match value {
        MaybeUndefined::Undefined => None,
        MaybeUndefined::Null => Some(None),
        MaybeUndefined::Value(v) => Some(optional_text(Some(v))),
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
/// apply one edit to its draft tree, answer with the procedure as
/// stored.
async fn edit_revision_draft(
    ctx: &Context<'_>,
    procedure_id: &ID,
    edit: impl FnOnce(&mut Tree) -> Result<(), EditError>,
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

fn publish_error(error: PublishProcedureError) -> async_graphql::Error {
    match error {
        PublishProcedureError::NoDraft | PublishProcedureError::EmptyEnum(_) => {
            coded(Code::InvalidDraft, error.to_string())
        }
        PublishProcedureError::StaleDraft => coded(
            Code::Conflict,
            "the draft's base is no longer the published head; discard or rebase the draft",
        ),
        PublishProcedureError::Db(e) if e.is_condition_failed() => coded(
            Code::Conflict,
            "the procedure changed since it was read; re-read and retry",
        ),
        PublishProcedureError::CorruptDraft(e) => internal(e),
        PublishProcedureError::CorruptState(e) => internal(e),
        PublishProcedureError::Kernel(e) => internal(e),
        PublishProcedureError::Db(e) => internal(e),
    }
}

fn lifecycle_error(error: LifecycleError) -> async_graphql::Error {
    match error {
        LifecycleError::Transition(e) => coded(Code::InvalidTransition, e.to_string()),
        LifecycleError::Db(e) if e.is_condition_failed() => coded(
            Code::Conflict,
            "the procedure changed since it was read; re-read and retry",
        ),
        LifecycleError::Db(e) => internal(e),
        LifecycleError::Corrupt(e) => internal(e),
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
            principal.account_id,
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

    /// Closes a published procedure to new submissions (platform
    /// P.4 *Procedure lifecycle*); `INVALID_TRANSITION` from any
    /// other state.
    async fn close_procedure(
        &self,
        ctx: &Context<'_>,
        input: CloseProcedureInput,
    ) -> async_graphql::Result<Procedure> {
        let (principal, mut db) = session(ctx)?;
        let (mut procedure, organization) =
            administered_procedure(&mut db, principal.account_id, &input.procedure_id).await?;
        platform_core::close_procedure(&mut db, &mut procedure, principal.account_id)
            .await
            .map_err(lifecycle_error)?;
        Ok(Procedure {
            procedure,
            organization: OrganizationRef::from(&organization),
        })
    }

    /// Reopens a closed procedure on its last published revision —
    /// no publication involved; `INVALID_TRANSITION` from any other
    /// state.
    async fn reopen_procedure(
        &self,
        ctx: &Context<'_>,
        input: ReopenProcedureInput,
    ) -> async_graphql::Result<Procedure> {
        let (principal, mut db) = session(ctx)?;
        let (mut procedure, organization) =
            administered_procedure(&mut db, principal.account_id, &input.procedure_id).await?;
        platform_core::reopen_procedure(&mut db, &mut procedure, principal.account_id)
            .await
            .map_err(lifecycle_error)?;
        Ok(Procedure {
            procedure,
            organization: OrganizationRef::from(&organization),
        })
    }

    /// Publishes the procedure's revision draft (G.10): free reports
    /// publish immediately; a lossy, checked or breaking one without
    /// `confirm` answers with `published: false` and writes nothing.
    /// Publishing transitions the lifecycle — from `Closed` it is the
    /// reopen.
    async fn publish_revision(
        &self,
        ctx: &Context<'_>,
        input: PublishRevisionInput,
    ) -> async_graphql::Result<crate::PublishRevisionResult> {
        let (principal, mut db) = session(ctx)?;
        let (mut procedure, organization) =
            administered_procedure(&mut db, principal.account_id, &input.procedure_id).await?;
        // The composition point (P.4 *Publication*): one transaction,
        // the kernel store scoped over its shared executor, platform
        // writes beside it, one commit.
        let mut tx = db.transaction().await.map_err(internal)?;
        let outcome = {
            let shared: platform_core::SharedExecutor =
                tokio::sync::Mutex::new(&mut tx as &mut dyn toasty::Executor);
            let store = platform_store::PlatformStore::new(&shared);
            platform_core::publish_procedure(
                &shared,
                &store,
                &mut procedure,
                principal.account_id,
                input.confirm,
            )
            .await
        };
        match outcome {
            Ok(PublishProcedureOutcome::Published { report, .. }) => {
                tx.commit().await.map_err(internal)?;
                Ok(crate::PublishRevisionResult {
                    report: ImpactReport::from(&report),
                    published: true,
                    procedure: Procedure {
                        procedure,
                        organization: OrganizationRef::from(&organization),
                    },
                })
            }
            // Nothing was written; the dropped transaction rolls back.
            Ok(PublishProcedureOutcome::RequiresConfirmation { report }) => {
                Ok(crate::PublishRevisionResult {
                    report: ImpactReport::from(&report),
                    published: false,
                    procedure: Procedure {
                        procedure,
                        organization: OrganizationRef::from(&organization),
                    },
                })
            }
            Err(error) => Err(publish_error(error)),
        }
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
        let (ty, arity, format) = input.ty.into_column_type()?;
        let label = input.label.trim().to_owned();
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let placement = input.placement.resolve(tree)?;
            // Omitted, requiredness defaults by the column's
            // *effective* audience (G.7): public required,
            // reviewer-only optional.
            let audience: platform_core::Audience = input.audience.into();
            let effective =
                platform_core::effective_audience(tree, &placement.parent)?.narrowest(audience);
            let required = input
                .required
                .unwrap_or(effective == platform_core::Audience::All);
            platform_core::add_element(
                tree,
                &placement,
                new_column(label, ty, arity, format, required, audience),
            )
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
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let placement = input.placement.resolve(tree)?;
            platform_core::add_element(
                tree,
                &placement,
                new_group(label, input.cardinality, input.audience.into()),
            )
        })
        .await
    }

    /// Adds an empty section to the procedure's revision draft;
    /// placed like `addColumn`. Its id is a kernel `NodeId` (DESIGN
    /// §2.6, surface node identity).
    async fn add_section(
        &self,
        ctx: &Context<'_>,
        input: AddSectionInput,
    ) -> async_graphql::Result<Procedure> {
        validate_non_empty("title", &input.title)?;
        let title = input.title.trim().to_owned();
        let help = optional_text(input.help);
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let placement = input.placement.resolve(tree)?;
            platform_core::add_element(
                tree,
                &placement,
                new_section(title, help, input.audience.into()),
            )
        })
        .await
    }

    /// Adds a note to the procedure's revision draft; placed like
    /// `addColumn`. A `REVIEWER` note is guidance for instructors.
    async fn add_note(
        &self,
        ctx: &Context<'_>,
        input: AddNoteInput,
    ) -> async_graphql::Result<Procedure> {
        validate_non_empty("body", &input.body)?;
        let body = input.body.trim().to_owned();
        let title = optional_text(input.title);
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let placement = input.placement.resolve(tree)?;
            platform_core::add_element(
                tree,
                &placement,
                new_note(title, body, input.audience.into()),
            )
        })
        .await
    }

    /// Changes a column's label, type (the type carries whether it
    /// holds many values) or audience; its id — its identity for the
    /// impact report — never changes.
    async fn update_column(
        &self,
        ctx: &Context<'_>,
        input: UpdateColumnInput,
    ) -> async_graphql::Result<Procedure> {
        if let Some(label) = &input.label {
            validate_non_empty("label", label)?;
        }
        let (ty, arity, format) = match input
            .ty
            .map(ColumnTypeInput::into_column_type)
            .transpose()?
        {
            // The type input carries the whole format intent: sending
            // a type without a format clears the constraint.
            Some((ty, arity, format)) => (Some(ty), Some(arity), Some(format)),
            None => (None, None, None),
        };
        let patch = ColumnPatch {
            label: input.label.map(|l| l.trim().to_owned()),
            ty,
            arity,
            required: input.required,
            format,
            audience: input.audience.map(Into::into),
        };
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let id = match element_id(tree, &input.id)? {
                platform_core::ElementId::Column(id) => id,
                other => return Err(EditError::UnknownElement(other)),
            };
            platform_core::update_column(tree, &id, patch)
        })
        .await
    }

    /// Changes a group's label, cardinality or audience; its id and
    /// children stay.
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
            audience: input.audience.map(Into::into),
        };
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let id = match element_id(tree, &input.id)? {
                platform_core::ElementId::Group(id) => id,
                other => return Err(EditError::UnknownElement(other)),
            };
            platform_core::update_group(tree, &id, patch)
        })
        .await
    }

    /// Changes a section's title, help or audience; its id and
    /// children stay.
    async fn update_section(
        &self,
        ctx: &Context<'_>,
        input: UpdateSectionInput,
    ) -> async_graphql::Result<Procedure> {
        if let Some(title) = &input.title {
            validate_non_empty("title", title)?;
        }
        let patch = SectionPatch {
            title: input.title.map(|t| t.trim().to_owned()),
            help: clearable_text(input.help),
            audience: input.audience.map(Into::into),
        };
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let id = match element_id(tree, &input.id)? {
                platform_core::ElementId::Section(id) => id,
                other => return Err(EditError::UnknownElement(other)),
            };
            platform_core::update_section(tree, &id, patch)
        })
        .await
    }

    /// Changes a note's title, body or audience; its id stays.
    async fn update_note(
        &self,
        ctx: &Context<'_>,
        input: UpdateNoteInput,
    ) -> async_graphql::Result<Procedure> {
        if let Some(body) = &input.body {
            validate_non_empty("body", body)?;
        }
        let patch = NotePatch {
            title: clearable_text(input.title),
            body: input.body.map(|b| b.trim().to_owned()),
            audience: input.audience.map(Into::into),
        };
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let id = match element_id(tree, &input.id)? {
                platform_core::ElementId::Note(id) => id,
                other => return Err(EditError::UnknownElement(other)),
            };
            platform_core::update_note(tree, &id, patch)
        })
        .await
    }

    /// Moves an element (a group or section with its subtree) to
    /// `placement`.
    async fn move_element(
        &self,
        ctx: &Context<'_>,
        input: MoveElementInput,
    ) -> async_graphql::Result<Procedure> {
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let id = element_id(tree, &input.id)?;
            let placement = input.placement.resolve(tree)?;
            platform_core::move_element(tree, &id, &placement)
        })
        .await
    }

    /// Removes an element (a group or section with its subtree) from
    /// the draft.
    async fn remove_element(
        &self,
        ctx: &Context<'_>,
        input: RemoveElementInput,
    ) -> async_graphql::Result<Procedure> {
        edit_revision_draft(ctx, &input.procedure_id, move |tree| {
            let id = element_id(tree, &input.id)?;
            platform_core::remove_element(tree, &id).map(|_| ())
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
        platform_core::discard_revision_draft(&mut db, &mut procedure, principal.account_id)
            .await
            .map_err(|e| draft_error(RevisionDraftError::Db(e)))?;
        Ok(Procedure {
            procedure,
            organization: OrganizationRef::from(&organization),
        })
    }
}
