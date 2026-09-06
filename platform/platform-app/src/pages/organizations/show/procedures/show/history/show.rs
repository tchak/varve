//! `…/procedures/{pid}/history/{event}`: the **diff page** (platform
//! P.4 *Procedure history*, G.11) — one publication's impact report,
//! recomputed at read time by the server from the two
//! content-addressed schemas and rendered exactly as the publish
//! confirmation renders its own. A first publication (`base: null`)
//! goes through the same path: the report against the empty schema
//! *is* the initial column list. Absent, invisible, malformed, and
//! non-publication event ids are one 404.

use cynic::QueryBuilder;
use platform_client::procedure::{
    DiffEvent, ProcedureEventDiffQuery, ProcedureEventDiffVariables, PublishedEventDiff,
};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{RouterErrorExt, not_found},
        href, page, path_param,
    },
    view::{View, component, view},
};

use crate::pages::organizations::show::OrganizationId;
use crate::pages::organizations::show::procedures::show::ProcedureId;
use crate::pages::organizations::show::procedures::show::impact::report_lines;
use crate::{
    client,
    components::{
        breadcrumbs::{Crumb, breadcrumbs},
        card::{card, card_content},
        page_title::page_title,
    },
    i18n::{t, t_args},
    pages::{args, utc_date_arg},
};

path_param!(pub(super) event_id: uuid::Uuid, error = not_found);

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let organization_id = *path_param::<OrganizationId>(cx)?;
    let procedure_id = *path_param::<ProcedureId>(cx)?;
    let event_id = *path_param::<EventId>(cx)?;
    let client = client(cx).await?;
    let procedure = platform_client::run(
        &client,
        ProcedureEventDiffQuery::build(ProcedureEventDiffVariables {
            id: cynic::Id::new(procedure_id.to_string()),
            event: cynic::Id::new(event_id.to_string()),
        }),
    )
    .await?
    .procedure
    .ok_or_not_found()?;
    // Only a publication has a diff: any other kind is a URL nobody
    // is given, answered like an unknown id.
    let DiffEvent::Published(event) = procedure.event.ok_or_not_found()? else {
        return Err(not_found().into());
    };
    Ok(view! {
        diff_page(
            organization_id: organization_id,
            procedure_id: procedure_id,
            organization_name: procedure.organization.name,
            procedure_title: procedure.title,
            event: event
        )
    })
}

/// The publication's story: when and by whom, the initial-schema
/// note on a first publication, the report as localized lines, and
/// the way back.
#[component]
async fn diff_page(
    cx: &Cx,
    organization_id: uuid::Uuid,
    procedure_id: uuid::Uuid,
    organization_name: String,
    procedure_title: String,
    event: PublishedEventDiff,
) -> Result<impl View> {
    let heading = t_args(
        cx,
        "history.title",
        &args([("date", utc_date_arg(event.created_at))]),
    )
    .await?;
    let actor = match &event.actor {
        Some(actor) => actor.name.clone(),
        None => t(cx, "procedure.history.actor.system").await?,
    };
    let lead = t_args(
        cx,
        "procedure.history.published",
        &args([
            ("actor", actor.into()),
            ("date", utc_date_arg(event.created_at)),
        ]),
    )
    .await?;
    let first = if event.base.is_none() {
        Some(t(cx, "history.first").await?)
    } else {
        None
    };
    let lines = report_lines(cx, &event.report).await?;
    let no_changes = t(cx, "schema.impact.none").await?;
    let crumb_label = t(cx, "nav.breadcrumb").await?;
    let crumbs = vec![
        crate::pages::organizations_crumb(cx).await?,
        Crumb::link(
            organization_name,
            href!(
                crate::pages::organizations::show::page,
                OrganizationId(organization_id)
            )
            .resolve(cx),
        ),
        Crumb::link(
            t(cx, "procedures.title").await?,
            href!(
                crate::pages::organizations::show::procedures::page,
                OrganizationId(organization_id)
            )
            .resolve(cx),
        ),
        Crumb::link(
            procedure_title,
            href!(
                crate::pages::organizations::show::procedures::show::page,
                OrganizationId(organization_id),
                ProcedureId(procedure_id)
            )
            .resolve(cx),
        ),
        Crumb::here(heading.clone()),
    ];
    Ok(view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2">
                breadcrumbs(label: crumb_label, crumbs: crumbs)
                page_title((heading.as_str()))
                <p class="text-sm text-muted-foreground">(lead.as_str())</p>
                if let Some(first) = &first {
                    <p class="text-sm">(first.as_str())</p>
                }
            </div>
            card(
                card_content(
                    if lines.is_empty() {
                        <p class="text-sm text-muted-foreground">(no_changes)</p>
                    } else {
                        <ul class="list-disc pl-5 text-sm" data-impact-report="">
                            for line in &lines {
                                <li>(line.as_str())</li>
                            }
                        </ul>
                    }
                )
            )
        </div>
    })
}
