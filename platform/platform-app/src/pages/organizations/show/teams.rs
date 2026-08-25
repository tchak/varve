//! `/organizations/{id}/teams`, derived from this module's name: the
//! organization's teams and the creation form ([`submit`]). The
//! organization is the parent segment's parameter, read through the
//! typed client like the organization page; the subtree's gate
//! (`organizations::gate`) turns an anonymous request into the
//! `/signin` redirect.
//!
//! The one-404 rule of the organization page holds here: the read is
//! `null` for an absent or invisible organization, and a creation
//! against one fails with `FORBIDDEN` — both answer 404, so a
//! non-member learns nothing from either request.

use cynic::{MutationBuilder, QueryBuilder};
use platform_client::team::{
    CreateTeam, CreateTeamInput, CreateTeamVariables, OrganizationTeams, OrganizationTeamsQuery,
    OrganizationTeamsVariables,
};
use platform_client::{Code, Error};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{RouterErrorExt, not_found},
        href, page, path_param,
    },
    view::{attributes, component, view},
};

use crate::{
    client,
    components::{
        breadcrumbs::{Crumb, breadcrumbs},
        button::button,
        card::{card, card_content, card_footer, card_header},
        field::field,
        page_title::page_title,
    },
    i18n::t,
    pages::redirect_to,
};

use super::OrganizationId;

/// A creation submission.
#[derive(Deserialize)]
struct Creation {
    name: String,
}

/// What the creation form shows.
#[derive(Default)]
struct CreationForm {
    name: String,
    name_error: Option<String>,
}

/// The organization's teams through the client; 404 when the schema
/// answers `null`.
async fn organization_teams(cx: &Cx) -> Result<OrganizationTeams> {
    let id = path_param::<OrganizationId>(cx)?;
    let client = client(cx).await?;
    Ok(platform_client::run(
        &client,
        OrganizationTeamsQuery::build(OrganizationTeamsVariables {
            id: cynic::Id::new(id.to_string()),
        }),
    )
    .await?
    .organization
    .ok_or_not_found()?)
}

/// The page: the title, a lead line linking back to the organization,
/// the list card, and the creation card. Shared by the GET [`page`]
/// and [`submit`]'s failed-creation re-render.
#[component]
async fn teams_page(cx: &Cx, organization: OrganizationTeams, form: CreationForm) -> Result {
    let organization_id: uuid::Uuid = organization.id.inner().parse()?;
    let title = t(cx, "teams.title").await?;
    let crumb_label = t(cx, "nav.breadcrumb").await?;
    let crumbs = vec![
        crate::pages::organizations_crumb(cx).await?,
        Crumb::link(
            organization.name.clone(),
            href!(super::page, OrganizationId(organization_id)).resolve(cx),
        ),
        Crumb::here(title.clone()),
    ];
    let list_heading = t(cx, "teams.list.title").await?;
    let empty = t(cx, "teams.list.empty").await?;
    let create_heading = t(cx, "teams.create.title").await?;
    let name_label = t(cx, "form.name").await?;
    let create_label = t(cx, "teams.create.submit").await?;
    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2">
                breadcrumbs(label: crumb_label, crumbs: crumbs)
                page_title((title))
            </div>
            card(
                card_header(<h2 class="leading-none font-semibold">(list_heading)</h2>)
                card_content(
                    if organization.teams.is_empty() {
                        <p class="text-sm text-muted-foreground">(empty)</p>
                    } else {
                        <ul class="flex flex-col gap-2">
                            for team in &organization.teams {
                                <li
                                    data-team-id=(team.id.inner())
                                    class="text-sm font-medium"
                                >
                                    (team.name.as_str())
                                </li>
                            }
                        </ul>
                    }
                )
            )
            card(
                card_header(
                    <h2 class="leading-none font-semibold">(create_heading)</h2>
                )
                <form
                    method="post"
                    action=(href!(submit, OrganizationId(organization_id)))
                    class="contents"
                >
                    card_content(
                        field(
                            id: "team-name",
                            label: name_label,
                            error: form.name_error,
                            attrs: attributes! {
                                type="text"
                                name="name"
                                value=(form.name.as_str())
                                required=""
                                autocomplete="off"
                            }
                        )
                    )
                    card_footer(
                        button(attrs: attributes! { type="submit" }, (create_label))
                    )
                </form>
            )
        </div>
    }
}

/// The list and the creation form.
#[page]
pub async fn page(cx: &Cx) -> Result {
    let organization = organization_teams(cx).await?;
    view! { teams_page(organization: organization, form: CreationForm::default()) }
}

/// Creates a team through `createTeam` and answers 303 back to
/// [`page`]. A blank name is caught before the round trip and
/// re-renders with the error in the field; `FORBIDDEN` is the 404
/// (module docs). The organization is re-read for the re-render only
/// — a creation never needs it.
#[page(POST)]
async fn submit(cx: &Cx, Form(input): Form<Creation>) -> Result {
    let id = path_param::<OrganizationId>(cx)?;
    let client = client(cx).await?;
    let name = input.name.trim().to_owned();
    if name.is_empty() {
        let organization = organization_teams(cx).await?;
        let form = CreationForm {
            name,
            name_error: Some(t(cx, "teams.create.error.name-required").await?),
        };
        return view! { teams_page(organization: organization, form: form) };
    }
    let operation = CreateTeam::build(CreateTeamVariables {
        input: CreateTeamInput {
            organization_id: cynic::Id::new(id.to_string()),
            name,
        },
    });
    match platform_client::run(&client, operation).await {
        Ok(_) => redirect_to(cx, href!(page, OrganizationId(*id)).resolve(cx)).await,
        Err(error @ Error::GraphQl(_)) if error.code() == Some(Code::Forbidden) => {
            Err(not_found().into())
        }
        Err(error) => Err(error.into()),
    }
}
