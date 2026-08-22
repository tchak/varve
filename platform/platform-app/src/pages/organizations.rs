//! `/organizations`, derived from this module's name: the signed-in
//! account's organizations — the list, the creation form
//! ([`submit`]), and the [`show`] subtree (`/organizations/{id}`).
//!
//! **The first pages read through the typed client** (design/
//! platform.md P.9 Q2): every datum here comes from
//! `platform_client` operations executed in-process as the signed-in
//! principal ([`crate::client`]), never from `platform-core` directly
//! — the app is integrator #1 (P.1 rule 4). Visibility is therefore
//! the schema's (G.6): the list is the viewer's memberships, and a
//! foreign or absent organization is `null`, which [`show`] renders
//! as the branded 404.
//!
//! The guard is the same as `/settings`: every handler asks for the
//! client (which fails closed with `UnauthorizedError`), and the
//! module-derived [`gate`] layout turns that into a 303 to `/signin`.

pub(super) mod show;

use cynic::{MutationBuilder, QueryBuilder};
use platform_client::organization::{
    CreateOrganization, CreateOrganizationInput, CreateOrganizationVariables, OrganizationRef,
    OrganizationsQuery,
};
use platform_client::{Code, Error, Slug};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::UnauthorizedError, href, layout, page},
    view::{attributes, component, view},
};

use crate::{
    client,
    components::{
        button::button,
        card::{card, card_content, card_footer, card_header},
        field::field,
        page_title::page_title,
    },
    i18n::t,
    pages::{redirect_to, signin},
};

use show::OrganizationId;

/// The friendly face of the signed-in guard for the subtree: a page
/// that failed closed with `UnauthorizedError` answers 303 to
/// `/signin` instead of a bare 401.
#[layout]
async fn gate(cx: &Cx, slot: Result) -> Result {
    match slot {
        Err(error) if error.downcast_ref::<UnauthorizedError>().is_some() => {
            redirect_to(cx, href!(signin::page).resolve(cx)).await
        }
        other => other,
    }
}

/// A creation submission.
#[derive(Deserialize)]
struct Creation {
    name: String,
    slug: String,
}

/// What the creation form shows: the values (empty on GET, the
/// submitted ones on a failed creation) and each field's error.
#[derive(Default)]
struct CreationForm {
    name: String,
    slug: String,
    name_error: Option<String>,
    slug_error: Option<String>,
}

/// The page: the list card (the viewer's organizations as links to
/// [`show::page`], or the empty notice) and the creation card.
/// Shared by the GET [`page`] and [`submit`]'s failed-creation
/// re-render. Card headings are `<h2>` (not the vendored `card_title`,
/// an `<h3>`) so the outline never skips a level under the `<h1>`.
#[component]
async fn organizations_page(cx: &Cx, form: CreationForm) -> Result {
    let client = client(cx).await?;
    let organizations = platform_client::run(&client, OrganizationsQuery::build(()))
        .await?
        .organizations;
    let title = t(cx, "organizations.title").await?;
    let list_heading = t(cx, "organizations.list.title").await?;
    let empty = t(cx, "organizations.list.empty").await?;
    let create_heading = t(cx, "organizations.create.title").await?;
    let name_label = t(cx, "form.name").await?;
    let slug_label = t(cx, "form.slug").await?;
    let slug_hint = t(cx, "form.slug.hint").await?;
    let create_label = t(cx, "organizations.create.submit").await?;
    view! {
        <div class="flex flex-col gap-6">
            page_title((title))
            card(
                card_header(<h2 class="leading-none font-semibold">(list_heading)</h2>)
                card_content(
                    if organizations.is_empty() {
                        <p class="text-sm text-muted-foreground">(empty)</p>
                    } else {
                        <ul class="flex flex-col">
                            for organization in &organizations {
                                organization_row(organization: organization.clone())
                            }
                        </ul>
                    }
                )
            )
            card(
                card_header(
                    <h2 class="leading-none font-semibold">(create_heading)</h2>
                )
                <form method="post" action=(href!(submit)) class="contents">
                    card_content(
                        <div class="flex flex-col gap-4">
                            field(
                                id: "organization-name",
                                label: name_label,
                                error: form.name_error,
                                attrs: attributes! {
                                    type="text"
                                    name="name"
                                    value=(form.name.as_str())
                                    required=""
                                    autocomplete="organization"
                                }
                            )
                            field(
                                id: "organization-slug",
                                label: slug_label,
                                error: form.slug_error,
                                attrs: attributes! {
                                    type="text"
                                    name="slug"
                                    value=(form.slug.as_str())
                                    required=""
                                    autocomplete="off"
                                    autocapitalize="none"
                                    spellcheck="false"
                                    placeholder=(slug_hint.as_str())
                                }
                            )
                        </div>
                    )
                    card_footer(
                        button(attrs: attributes! { type="submit" }, (create_label))
                    )
                </form>
            )
        </div>
    }
}

/// One list row: the name as the link to the organization, the slug
/// beside it. The id is the schema's `ID` — a UUID — parsed here for
/// `href!`; one the server minted always parses.
#[component]
async fn organization_row(organization: OrganizationRef) -> Result {
    let id: uuid::Uuid = organization.id.inner().parse()?;
    view! {
        <li
            data-organization-id=(organization.id.inner())
            class="flex items-center gap-3 border-b border-border py-4 first:pt-0 \
                   last:border-b-0 last:pb-0"
        >
            <a
                href=(href!(show::page, OrganizationId(id)))
                class="text-sm font-medium underline-offset-4 hover:underline"
            >
                (organization.name.as_str())
            </a>
            <code class="font-mono text-xs text-muted-foreground">
                (organization.slug.0.as_str())
            </code>
        </li>
    }
}

/// The list and the creation form.
#[page]
pub async fn page(_cx: &Cx) -> Result {
    view! { organizations_page(form: CreationForm::default()) }
}

/// Creates an organization through the schema's `createOrganization`
/// and answers 303 to the new organization's page. The form
/// re-renders with the message in the field on every failure the
/// viewer can fix: a blank name or slug (checked here, before the
/// round trip — the schema's `INVALID_INPUT` does not name the field,
/// so with both non-blank the only `INVALID_INPUT` left is the slug's
/// shape), `SLUG_TAKEN`, and an invalid slug. Anything else is the
/// platform's failure and surfaces as a 500.
#[page(POST)]
async fn submit(cx: &Cx, Form(input): Form<Creation>) -> Result {
    let client = client(cx).await?;
    let name = input.name.trim().to_owned();
    let slug = input.slug.trim().to_owned();
    let mut form = CreationForm {
        name: name.clone(),
        slug: slug.clone(),
        ..CreationForm::default()
    };
    if name.is_empty() {
        form.name_error = Some(t(cx, "organizations.create.error.name-required").await?);
    }
    if slug.is_empty() {
        form.slug_error = Some(t(cx, "organizations.create.error.slug-required").await?);
    }
    if form.name_error.is_some() || form.slug_error.is_some() {
        return view! { organizations_page(form: form) };
    }
    let operation = CreateOrganization::build(CreateOrganizationVariables {
        input: CreateOrganizationInput {
            slug: Slug(slug),
            name,
        },
    });
    match platform_client::run(&client, operation).await {
        Ok(created) => {
            let id: uuid::Uuid = created.create_organization.id.inner().parse()?;
            redirect_to(cx, href!(show::page, OrganizationId(id)).resolve(cx)).await
        }
        Err(error @ Error::GraphQl(_)) => {
            form.slug_error = Some(match error.code() {
                Some(Code::SlugTaken) => t(cx, "organizations.create.error.slug-taken").await?,
                Some(Code::InvalidInput) => {
                    t(cx, "organizations.create.error.slug-invalid").await?
                }
                _ => return Err(error.into()),
            });
            view! { organizations_page(form: form) }
        }
        Err(error) => Err(error.into()),
    }
}
