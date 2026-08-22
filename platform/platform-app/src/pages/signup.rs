//! `/signup`, derived from this module's name: the form ([`page`])
//! and the registering submission ([`submit`]) share the path.

use platform_core::RegisterError;
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::bad_request, href, page},
    view::{attributes, component, view},
};

use crate::{
    auth::sign_in,
    components::{
        alert::{AlertVariant, alert, alert_title},
        button::button,
        card::{card, card_content, card_footer},
        field::field,
        page_title::page_title,
    },
    db,
    i18n::{request_locale, t},
};

use super::redirect_to;

/// A registration submission.
#[derive(Deserialize)]
struct Registration {
    name: String,
    email: String,
    password: String,
}

/// The signup form, shared by the GET page and the duplicate-email
/// re-render; same composition as [`signin_form`](super::signin).
#[component]
async fn signup_form(cx: &Cx, error: Option<String>, name: String, email: String) -> Result {
    let title = t(cx, "signup.title").await?;
    let name_label = t(cx, "form.name").await?;
    let email_label = t(cx, "form.email").await?;
    let password_label = t(cx, "form.password").await?;
    let submit_label = t(cx, "signup.submit").await?;
    let signin_link = t(cx, "signup.signin-link").await?;
    view! {
        <div class="mx-auto flex w-full max-w-sm flex-col gap-6">
            page_title((title))
            if let Some(error) = &error {
                alert(
                    variant: AlertVariant::Destructive,
                    attrs: attributes! { role="alert" },
                    alert_title((error))
                )
            }
            card(
                card_content(
                    <form
                        method="post"
                        action=(href!(submit))
                        class="flex flex-col gap-4"
                    >
                        field(
                            id: "signup-name",
                            label: name_label,
                            attrs: attributes! {
                                type="text"
                                name="name"
                                value=(name.as_str())
                                required=""
                                autocomplete="name"
                            }
                        )
                        field(
                            id: "signup-email",
                            label: email_label,
                            attrs: attributes! {
                                type="email"
                                name="email"
                                value=(email.as_str())
                                required=""
                                autocomplete="email"
                            }
                        )
                        field(
                            id: "signup-password",
                            label: password_label,
                            attrs: attributes! {
                                type="password"
                                name="password"
                                required=""
                                autocomplete="new-password"
                            }
                        )
                        button(attrs: attributes! { type="submit" }, (submit_label))
                    </form>
                )
                card_footer(
                    <a
                        href=(href!(super::signin::page))
                        class="text-sm text-muted-foreground underline underline-offset-4 hover:text-foreground"
                    >
                        (signin_link)
                    </a>
                )
            )
        </div>
    }
}

/// The signup form.
#[page]
pub async fn page() -> Result {
    view! { signup_form(error: None, name: String::new(), email: String::new()) }
}

/// Registers the account and logs it straight in. A duplicate email
/// re-renders with a message ([`platform_core::register`] settles
/// the race on the database's unique index, so two concurrent
/// submissions cannot both pass). An empty field is a 400: the form's
/// `required` attributes keep browsers from submitting one, so it can
/// only come from a forged request — the same posture as
/// `/settings/account` takes for an unsupported locale.
///
/// The new account's locale preference is this request's *resolved*
/// locale ([`request_locale`], already reduced by `resolve_locale`
/// to a supported tag or the English fallback), not the raw
/// `Accept-Language` header.
/// Storing the resolved value is deliberate: it records the language
/// the user actually signed up in, always names a locale the
/// platform can serve, and — because the stored preference wins over
/// the header in resolution — pins the UI to that language even if
/// the browser's language changes later, until the account picks
/// another on `/settings/account`.
#[page(POST)]
async fn submit(cx: &Cx, Form(input): Form<Registration>) -> Result {
    let mut db = db(cx);
    let locale = request_locale(cx).await?.to_string();
    match platform_core::register(
        &mut db,
        &input.email,
        &input.password,
        &input.name,
        Some(&locale),
    )
    .await
    {
        Ok(account) => {
            sign_in(cx, &account).await?;
            redirect_to(cx, href!(super::home).resolve(cx)).await
        }
        Err(RegisterError::EmailTaken) => {
            let error = t(cx, "signup.error.email-taken").await?;
            view! {
                signup_form(error: Some(error), name: input.name, email: input.email)
            }
        }
        Err(RegisterError::EmptyField) => {
            Err(bad_request("email, name, and password are required").into())
        }
        Err(RegisterError::Auth(error)) => Err(error.into()),
    }
}
