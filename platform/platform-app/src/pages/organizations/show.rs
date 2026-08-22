//! `/organizations/{id}`: one organization, read through the typed
//! client's root lookup. The module's segment *is* the parameter
//! (`path_param!` inside a `module_router!` module).
//!
//! **Absent, invisible, and malformed ids are one answer: 404.** The
//! schema answers `null` for an organization the viewer is not a
//! member of exactly as for one that does not exist (G.6), and this
//! page keeps that property at the URL — a non-member learns nothing
//! from the status code. A non-UUID segment is 404 too (`error =
//! not_found`), never a 400 that would distinguish it.

use cynic::QueryBuilder;
use platform_client::organization::{Organization, OrganizationQuery, OrganizationVariables};
use topcoat::{
    Result,
    context::Cx,
    router::{error::RouterErrorExt, page, path_param},
    view::{component, view},
};

use crate::{
    client,
    components::{
        card::{card, card_content, card_header},
        page_title::page_title,
    },
    i18n::{t, t_args},
    pages::{one_arg, utc_date_arg},
};

path_param!(pub(super) organization_id: uuid::Uuid, error = not_found);

/// The organization: its slug and creation date under the name, then
/// the members, teams, and procedures cards (counts in the headings,
/// an empty notice where a list is empty).
#[page]
pub async fn page(cx: &Cx) -> Result {
    let id = path_param::<OrganizationId>(cx)?;
    let client = client(cx).await?;
    let organization = platform_client::run(
        &client,
        OrganizationQuery::build(OrganizationVariables {
            id: cynic::Id::new(id.to_string()),
        }),
    )
    .await?
    .organization
    .ok_or_not_found()?;
    view! { organization_page(organization: organization) }
}

#[component]
async fn organization_page(cx: &Cx, organization: Organization) -> Result {
    let created = t_args(
        cx,
        "organization.created",
        &one_arg("date", utc_date_arg(organization.created_at)),
    )
    .await?;
    let members_heading = t_args(
        cx,
        "organization.members.title",
        &one_arg("count", i64::from(organization.counts.members)),
    )
    .await?;
    let teams_heading = t_args(
        cx,
        "organization.teams.title",
        &one_arg("count", i64::from(organization.counts.teams)),
    )
    .await?;
    let teams_empty = t(cx, "organization.teams.empty").await?;
    let procedures_heading = t_args(
        cx,
        "organization.procedures.title",
        &one_arg("count", i64::from(organization.counts.procedures)),
    )
    .await?;
    let procedures_empty = t(cx, "organization.procedures.empty").await?;
    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2">
                page_title((organization.name.as_str()))
                <p
                    class="flex flex-wrap items-center gap-3 text-sm text-muted-foreground"
                >
                    <code class="font-mono text-xs">
                        (organization.slug.0.as_str())
                    </code>
                    <span>(created)</span>
                </p>
            </div>
            card(
                card_header(
                    <h2 class="leading-none font-semibold">(members_heading)</h2>
                )
                card_content(
                    <ul class="flex flex-col gap-2">
                        for member in &organization.members {
                            <li class="flex flex-col text-sm">
                                <span class="font-medium">
                                    (member.account.name.as_str())
                                </span>
                                <span class="text-muted-foreground">
                                    (member.account.email.as_str())
                                </span>
                            </li>
                        }
                    </ul>
                )
            )
            card(
                card_header(<h2 class="leading-none font-semibold">(teams_heading)</h2>)
                card_content(
                    if organization.teams.is_empty() {
                        <p class="text-sm text-muted-foreground">(teams_empty)</p>
                    } else {
                        <ul class="flex flex-col gap-2">
                            for team in &organization.teams {
                                <li class="text-sm font-medium">(team.name.as_str())</li>
                            }
                        </ul>
                    }
                )
            )
            card(
                card_header(
                    <h2 class="leading-none font-semibold">(procedures_heading)</h2>
                )
                card_content(
                    if organization.procedures.is_empty() {
                        <p class="text-sm text-muted-foreground">(procedures_empty)</p>
                    } else {
                        <ul class="flex flex-col gap-2">
                            for procedure in &organization.procedures {
                                <li class="text-sm font-medium">
                                    (procedure.title.as_str())
                                </li>
                            }
                        </ul>
                    }
                )
            )
        </div>
    }
}
