//! `/organizations/{id}/procedures/{pid}`: one procedure — its catalog
//! row and the state of its revision draft, with the way into the
//! schema editor ([`schema`]). The segment is the procedure id
//! (`path_param!`); absent, invisible, and malformed are one 404, as
//! for the organization above it.

pub(super) mod history;
pub(super) mod impact;
pub(super) mod schema;

use cynic::QueryBuilder;
use platform_client::procedure::{
    ProcedureEvent, ProcedureLifecycleQuery, ProcedureLifecycleVariables,
};
use platform_client::revision_draft::{
    Element, ProcedureRevisionDraft, ProcedureRevisionDraftQuery, ProcedureRevisionDraftVariables,
};
use topcoat::{
    Result,
    context::Cx,
    router::{error::RouterErrorExt, href, page, path_param},
    view::{component, view},
};

use crate::{
    client,
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonSize, ButtonVariant, button_variants},
        card::{card, card_content, card_header},
        page_title::page_title,
    },
    i18n::{t, t_args},
    pages::{args, utc_date_arg},
};

use super::super::OrganizationId;

path_param!(pub(super) procedure_id: uuid::Uuid, error = not_found);

/// The procedure with its draft through the client; 404 when the
/// schema answers `null`.
pub(super) async fn procedure_draft(cx: &Cx) -> Result<ProcedureRevisionDraft> {
    let id = path_param::<ProcedureId>(cx)?;
    let client = client(cx).await?;
    Ok(platform_client::run(
        &client,
        ProcedureRevisionDraftQuery::build(ProcedureRevisionDraftVariables {
            id: cynic::Id::new(id.to_string()),
        }),
    )
    .await?
    .procedure
    .ok_or_not_found()?)
}

/// How many columns and groups a flat element list holds (sections
/// and notes are presentation, uncounted here).
pub(super) fn counts(elements: &[Element]) -> (usize, usize) {
    elements.iter().fold((0, 0), |(c, g), e| match e {
        Element::Column(_) => (c + 1, g),
        Element::Group(_) => (c, g + 1),
        _ => (c, g),
    })
}

/// The procedure's audit trail through the client — the history
/// section's read (G.11: the events list is the history).
async fn procedure_events(cx: &Cx) -> Result<Vec<ProcedureEvent>> {
    let id = path_param::<ProcedureId>(cx)?;
    let client = client(cx).await?;
    Ok(platform_client::run(
        &client,
        ProcedureLifecycleQuery::build(ProcedureLifecycleVariables {
            id: cynic::Id::new(id.to_string()),
        }),
    )
    .await?
    .procedure
    .ok_or_not_found()?
    .events)
}

#[page]
pub async fn page(cx: &Cx) -> Result {
    let procedure = procedure_draft(cx).await?;
    let events = procedure_events(cx).await?;
    view! { procedure_page(procedure: procedure, events: events) }
}

/// Title and description, the organization link, and the draft card:
/// "no draft yet" or the element counts with the last-saved date,
/// and the link into the editor either way.
#[component]
async fn procedure_page(
    cx: &Cx,
    procedure: ProcedureRevisionDraft,
    events: Vec<ProcedureEvent>,
) -> Result {
    let organization_id: uuid::Uuid = procedure.organization.id.inner().parse()?;
    let procedure_id: uuid::Uuid = procedure.id.inner().parse()?;
    let lead = t(cx, "procedure.lead").await?;
    let draft_heading = t(cx, "procedure.draft.title").await?;
    let draft_none = t(cx, "procedure.draft.none").await?;
    let draft_badge = t(cx, "procedure.draft.badge").await?;
    let edit_label = t(cx, "procedure.draft.edit").await?;
    let summary = if procedure.revision_draft.in_progress {
        let (columns, groups) = counts(&procedure.revision_draft.elements);
        Some(
            t_args(
                cx,
                "procedure.draft.summary",
                &args([
                    ("columns", (columns as i64).into()),
                    ("groups", (groups as i64).into()),
                    ("date", utc_date_arg(procedure.updated_at)),
                ]),
            )
            .await?,
        )
    } else {
        None
    };
    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2">
                page_title((procedure.title.as_str()))
                <p class="text-sm text-muted-foreground">
                    (lead)
                    " "
                    <a
                        href=(href!(super::super::page, OrganizationId(organization_id)))
                        class="font-medium text-foreground underline-offset-4 hover:underline"
                    >
                        (procedure.organization.name.as_str())
                    </a>
                </p>
                if !procedure.description.is_empty() {
                    <p class="text-sm">(procedure.description.as_str())</p>
                }
            </div>
            card(
                card_header(
                    <div class="flex items-center gap-3">
                        <h2 class="leading-none font-semibold">(draft_heading)</h2>
                        if summary.is_some() {
                            badge(variant: BadgeVariant::Secondary, (draft_badge))
                        }
                    </div>
                )
                card_content(
                    <div class="flex flex-col gap-4">
                        <p class="text-sm text-muted-foreground">
                            match &summary {
                                Some(summary) => (summary.as_str()),
                                None => (draft_none.as_str()),
                            }
                        </p>
                        <p>
                            <a
                                href=(href!(
                                    schema::page,
                                    OrganizationId(organization_id),
                                    ProcedureId(procedure_id)
                                ))
                                class=(button_variants(
                                    ButtonVariant::Primary,
                                    ButtonSize::Sm,
                                ))
                            >
                                (edit_label)
                            </a>
                        </p>
                    </div>
                )
            )
            history::section(
                organization_id: organization_id,
                procedure_id: procedure_id,
                events: events
            )
        </div>
    }
}
