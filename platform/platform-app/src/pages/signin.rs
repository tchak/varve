//! `/signin`, derived from this module's name: the form ([`page`])
//! and the authenticating submission ([`submit`]) share the path.

use platform_core::verify_credentials;
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, href, page},
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
    i18n::t,
    return_to,
};

use super::redirect_to;

/// An email + password submission.
#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}

/// The sign-in form, shared by the GET page and the failed POST
/// re-render (which passes the generic error and the typed email
/// back): a [`page_title`], the [`alert`] when the previous
/// submission failed, and a [`card`] holding the [`field`]s.
#[component]
async fn signin_form(cx: &Cx, error: Option<String>, email: String) -> Result {
    let title = t(cx, "signin.title").await?;
    let email_label = t(cx, "form.email").await?;
    let password_label = t(cx, "form.password").await?;
    let submit_label = t(cx, "signin.submit").await?;
    let signup_link = t(cx, "signin.signup-link").await?;
    view! {
        <div class="mx-auto flex w-full max-w-sm flex-col gap-6">
            page_title((title))
            if let Some(error) = &error {
                // `role="alert"` by choice: the error arrives on a
                // re-render after a failed submission, exactly the
                // interruption the role announces (the component
                // leaves the role to the caller).
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
                            id: "signin-email",
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
                            id: "signin-password",
                            label: password_label,
                            attrs: attributes! {
                                type="password"
                                name="password"
                                required=""
                                autocomplete="current-password"
                            }
                        )
                        button(attrs: attributes! { type="submit" }, (submit_label))
                    </form>
                )
                card_footer(
                    <a
                        href=(href!(super::signup::page))
                        class="text-sm text-muted-foreground underline underline-offset-4 hover:text-foreground"
                    >
                        (signup_link)
                    </a>
                )
            )
        </div>
    }
}

/// The sign-in form.
#[page]
pub async fn page() -> Result {
    view! { signin_form(error: None, email: String::new()) }
}

/// Authenticates and starts a session, then returns the browser to
/// the location a gate remembered ([`return_to::take`]) or home. The
/// failure message is the same for an unknown email and a wrong
/// password — [`verify_credentials`] already collapses the two
/// (including their timing), and the view must not reopen the leak;
/// a failed submission also leaves the remembered location alone for
/// the retry.
#[page(POST)]
async fn submit(cx: &Cx, Form(input): Form<Credentials>) -> Result {
    let mut db = db(cx);
    match verify_credentials(&mut db, &input.email, &input.password).await? {
        Some(account) => {
            sign_in(cx, &account).await?;
            let location = return_to::take(cx).unwrap_or_else(|| href!(super::home).resolve(cx));
            redirect_to(cx, location).await
        }
        None => {
            let error = t(cx, "signin.error.invalid-credentials").await?;
            view! { signin_form(error: Some(error), email: input.email) }
        }
    }
}
