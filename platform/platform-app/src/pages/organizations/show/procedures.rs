//! `/organizations/{id}/procedures`, derived from this module's name:
//! the organization's procedures and the creation form ([`submit`]) —
//! the [`super::teams`] shape over `createProcedure`, with a title
//! and a free-text description. The one-404 rule holds on GET and
//! POST alike (the teams module docs).

use cynic::{MutationBuilder, QueryBuilder};
use platform_client::procedure::{
    CreateProcedure, CreateProcedureInput, CreateProcedureVariables, OrganizationProcedures,
    OrganizationProceduresQuery, OrganizationProceduresVariables,
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
        button::button,
        card::{card, card_content, card_footer, card_header},
        field::field,
        label::label,
        page_title::page_title,
    },
    i18n::t,
    pages::redirect_to,
};

use super::OrganizationId;

/// A creation submission.
#[derive(Deserialize)]
struct Creation {
    title: String,
    #[serde(default)]
    description: String,
}

/// What the creation form shows.
#[derive(Default)]
struct CreationForm {
    title: String,
    description: String,
    title_error: Option<String>,
}

/// The organization's procedures through the client; 404 when the
/// schema answers `null`.
async fn organization_procedures(cx: &Cx) -> Result<OrganizationProcedures> {
    let id = path_param::<OrganizationId>(cx)?;
    let client = client(cx).await?;
    Ok(platform_client::run(
        &client,
        OrganizationProceduresQuery::build(OrganizationProceduresVariables {
            id: cynic::Id::new(id.to_string()),
        }),
    )
    .await?
    .organization
    .ok_or_not_found()?)
}

/// The page: title, lead line back to the organization, the list
/// card, the creation card. The description is a `<textarea>` styled
/// like the vendored input (no textarea component is vendored yet),
/// labelled through `label for=`.
#[component]
async fn procedures_page(
    cx: &Cx,
    organization: OrganizationProcedures,
    form: CreationForm,
) -> Result {
    let organization_id: uuid::Uuid = organization.id.inner().parse()?;
    let title = t(cx, "procedures.title").await?;
    let lead = t(cx, "procedures.lead").await?;
    let list_heading = t(cx, "procedures.list.title").await?;
    let empty = t(cx, "procedures.list.empty").await?;
    let create_heading = t(cx, "procedures.create.title").await?;
    let title_label = t(cx, "form.title").await?;
    let description_label = t(cx, "form.description").await?;
    let create_label = t(cx, "procedures.create.submit").await?;
    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2">
                page_title((title))
                <p class="text-sm text-muted-foreground">
                    (lead)
                    " "
                    <a
                        href=(href!(super::page, OrganizationId(organization_id)))
                        class="font-medium text-foreground underline-offset-4 hover:underline"
                    >
                        (organization.name.as_str())
                    </a>
                </p>
            </div>
            card(
                card_header(<h2 class="leading-none font-semibold">(list_heading)</h2>)
                card_content(
                    if organization.procedures.is_empty() {
                        <p class="text-sm text-muted-foreground">(empty)</p>
                    } else {
                        <ul class="flex flex-col gap-2">
                            for procedure in &organization.procedures {
                                <li
                                    data-procedure-id=(procedure.id.inner())
                                    class="text-sm font-medium"
                                >
                                    (procedure.title.as_str())
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
                        <div class="flex flex-col gap-4">
                            field(
                                id: "procedure-title",
                                label: title_label,
                                error: form.title_error,
                                attrs: attributes! {
                                    type="text"
                                    name="title"
                                    value=(form.title.as_str())
                                    required=""
                                    autocomplete="off"
                                }
                            )
                            <div class="flex flex-col gap-2">
                                label(
                                    attrs: attributes! { for="procedure-description" },
                                    (description_label)
                                )
                                <textarea
                                    id="procedure-description"
                                    name="description"
                                    rows="3"
                                    class="w-full min-w-0 rounded-lg border border-border \
                                           bg-background px-3 py-2 text-sm shadow-xs \
                                           transition-colors outline-none \
                                           placeholder:text-muted-foreground \
                                           focus-visible:ring-2 focus-visible:ring-ring \
                                           focus-visible:ring-offset-2 \
                                           focus-visible:ring-offset-background"
                                >
                                    (form.description.as_str())
                                </textarea>
                            </div>
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

/// The list and the creation form.
#[page]
pub async fn page(cx: &Cx) -> Result {
    let organization = organization_procedures(cx).await?;
    view! { procedures_page(organization: organization, form: CreationForm::default()) }
}

/// Creates a procedure through `createProcedure` and answers 303
/// back to [`page`]. A blank title re-renders with the error in the
/// field (the description kept); `FORBIDDEN` is the 404.
#[page(POST)]
async fn submit(cx: &Cx, Form(input): Form<Creation>) -> Result {
    let id = path_param::<OrganizationId>(cx)?;
    let client = client(cx).await?;
    let title = input.title.trim().to_owned();
    let description = input.description.trim().to_owned();
    if title.is_empty() {
        let organization = organization_procedures(cx).await?;
        let form = CreationForm {
            title,
            description,
            title_error: Some(t(cx, "procedures.create.error.title-required").await?),
        };
        return view! { procedures_page(organization: organization, form: form) };
    }
    let operation = CreateProcedure::build(CreateProcedureVariables {
        input: CreateProcedureInput {
            organization_id: cynic::Id::new(id.to_string()),
            title,
            description: (!description.is_empty()).then_some(description),
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
