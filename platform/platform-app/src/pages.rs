//! The P0 pages (PLATFORM.md P.8: the walking skeleton's shell):
//! layout, home, signin, signup, signout, and a branded not-found.
//!
//! Every page is composed from [`crate::components`] — topcoat-ui
//! components vendored by `topcoat ui add` (alert, button, card,
//! input, label) plus our own in the same style (field, page_title,
//! site_header) — styled with
//! Tailwind classes against the theme tokens in `styles.css`. Every
//! user-visible string still goes through [`t`] / [`t_args`] and is
//! passed to components as display text; components carry no
//! literals of their own. Every state change is a POST (which is
//! what the router's default `OriginPolicy` protects — see
//! [`crate::auth`]). Login and signup POSTs are *pages* so a failed
//! submission re-renders the form inside the layout; their success
//! path answers 303 via `redirect_to` (a `#[page]` must return a
//! view, so the redirect is expressed as a status-code + `Location`
//! header view — the pattern the view docs describe for status and
//! headers). Logout has no failure view, so it is a plain
//! `#[route]` returning [`SeeOther`].
//!
//! The Tailwind stylesheet is linked only when the router was built
//! with an asset bundle ([`crate::router`]): `stylesheet_href`
//! resolves the `tailwind::stylesheet!()` asset through the
//! registered [`AssetConfig`] and renders nothing without one, so
//! router-level tests (no bundle next to a test binary) never trip
//! the by-design panic on rendering an unbundled asset.

use platform_core::RegisterError;
use platform_i18n::{ArgValue, Args};
use serde::Deserialize;
use topcoat::{
    Result,
    asset::AssetConfig,
    context::{Cx, try_app_context},
    router::{
        HeaderValue, StatusCode,
        content::Form,
        error::{NotFoundError, SeeOther, see_other},
        header, href, layout, not_found, page, route,
    },
    tailwind,
    view::{attributes, component, view},
};

use crate::{
    auth::{account, principal, sign_in, sign_out},
    components::{
        alert::{AlertVariant, alert, alert_title},
        button::{ButtonSize, ButtonVariant, button, button_variants},
        card::{card, card_content, card_footer},
        field::field,
        page_title::page_title,
        site_header::site_header,
    },
    db,
    i18n::{request_locale, t, t_args},
};

not_found!("/");

/// A 303 answer from a `#[page]` handler: a view carrying only the
/// status code and `Location` header (the browser never renders the
/// enclosing layout markup on a redirect).
async fn redirect_to(cx: &Cx, location: String) -> Result {
    let location = HeaderValue::try_from(location)?;
    view! {
        cx =>
        (StatusCode::SEE_OTHER)
        ((header::LOCATION, location))
    }
}

/// One `{$email}` / `{$name}`-style argument map.
fn one_arg(name: &str, value: &str) -> Args {
    let mut args = Args::new();
    args.insert(name.to_owned(), ArgValue::from(value));
    args
}

/// The Tailwind stylesheet's URL, when the router holds an asset
/// bundle that contains it; `None` otherwise (router-level tests, or
/// a binary run without `topcoat asset bundle`), in which case the
/// layout renders no stylesheet link.
fn stylesheet_href(cx: &Cx) -> Option<String> {
    let stylesheet = tailwind::stylesheet!();
    try_app_context::<AssetConfig>(cx)
        .filter(|config| config.get(stylesheet).is_some())
        .map(|config| config.resolve(stylesheet))
}

/// The HTML shell: [`site_header`] with the sign-in state, main
/// slot. Also brands the not-found error (from the [`not_found!`]
/// catch-all or any page) instead of letting it bubble to a bare
/// 404.
#[layout("/")]
async fn shell(cx: &Cx, slot: Result) -> Result {
    let lang = request_locale(cx).to_string();
    let title = t(cx, "app.title")?;
    let slot = match slot {
        Err(error) if error.downcast_ref::<NotFoundError>().is_some() => {
            let message = t(cx, "error.not-found")?;
            view! {
                (StatusCode::NOT_FOUND)
                page_title((message))
            }
        }
        other => other,
    };
    let signed_in_as = match principal(cx) {
        Some(principal) => Some(t_args(
            cx,
            "nav.signed-in-as",
            &one_arg("email", &principal.email),
        )?),
        None => None,
    };
    let sign_in_label = t(cx, "nav.sign-in")?;
    let sign_up_label = t(cx, "nav.sign-up")?;
    let sign_out_label = t(cx, "nav.sign-out")?;
    view! {
        <!DOCTYPE html>
        <html lang=(lang)>
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title.as_str())</title>
                if let Some(stylesheet) = stylesheet_href(cx) {
                    <link rel="stylesheet" href=(stylesheet)>
                }
            </head>
            <body class="flex min-h-screen flex-col bg-background text-foreground">
                site_header(
                    brand_label: title.as_str(),
                    brand_href: href!(home).resolve(cx),
                    if let Some(signed_in_as) = &signed_in_as {
                        <span class="text-sm text-muted-foreground">
                            (signed_in_as)
                        </span>
                        <form method="post" action=(href!(signout))>
                            button(
                                variant: ButtonVariant::Outline,
                                size: ButtonSize::Sm,
                                attrs: attributes! { type="submit" },
                                (sign_out_label)
                            )
                        </form>
                    } else {
                        <a
                            href=(href!(signin))
                            class=(button_variants(
                                ButtonVariant::Ghost,
                                ButtonSize::Sm,
                            ))
                        >
                            (sign_in_label)
                        </a>
                        <a
                            href=(href!(signup))
                            class=(button_variants(
                                ButtonVariant::Primary,
                                ButtonSize::Sm,
                            ))
                        >
                            (sign_up_label)
                        </a>
                    }
                )
                <main class="mx-auto w-full max-w-3xl flex-1 px-6 py-10">(slot?)</main>
            </body>
        </html>
    }
}

/// Home: a greeting for the signed-in account (by display name, off
/// the account row the request-state layer already loaded), a
/// sign-in prompt otherwise.
#[page("/")]
async fn home(cx: &Cx) -> Result {
    let title = t(cx, "home.title")?;
    let message = match account(cx) {
        Some(account) => t_args(cx, "home.greeting", &one_arg("name", &account.name))?,
        None => t(cx, "home.signed-out")?,
    };
    view! {
        <section class="flex flex-col gap-3">
            page_title((title))
            <p class="text-muted-foreground">(message)</p>
        </section>
    }
}

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
    let title = t(cx, "signin.title")?;
    let email_label = t(cx, "form.email")?;
    let password_label = t(cx, "form.password")?;
    let submit_label = t(cx, "signin.submit")?;
    let signup_link = t(cx, "signin.signup-link")?;
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
                        action=(href!(signin_submit))
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
                        href=(href!(signup))
                        class="text-sm text-muted-foreground underline underline-offset-4 hover:text-foreground"
                    >
                        (signup_link)
                    </a>
                )
            )
        </div>
    }
}

#[page("/signin")]
async fn signin() -> Result {
    view! { signin_form(error: None, email: String::new()) }
}

/// Authenticates and starts a session. The failure message is the
/// same for an unknown email and a wrong password —
/// [`platform_core::verify_credentials`] already collapses the two
/// (including their timing), and the view must not reopen the leak.
#[page(POST "/signin")]
async fn signin_submit(cx: &Cx, Form(input): Form<Credentials>) -> Result {
    let mut db = db(cx);
    match platform_core::verify_credentials(&mut db, &input.email, &input.password).await? {
        Some(account) => {
            sign_in(cx, &account).await?;
            redirect_to(cx, href!(home).resolve(cx)).await
        }
        None => {
            let error = t(cx, "signin.error.invalid-credentials")?;
            view! { signin_form(error: Some(error), email: input.email) }
        }
    }
}

/// A registration submission.
#[derive(Deserialize)]
struct Registration {
    name: String,
    email: String,
    password: String,
}

/// The signup form, shared by the GET page and the duplicate-email
/// re-render; same composition as [`signin_form`].
#[component]
async fn signup_form(cx: &Cx, error: Option<String>, name: String, email: String) -> Result {
    let title = t(cx, "signup.title")?;
    let name_label = t(cx, "form.name")?;
    let email_label = t(cx, "form.email")?;
    let password_label = t(cx, "form.password")?;
    let submit_label = t(cx, "signup.submit")?;
    let signin_link = t(cx, "signup.signin-link")?;
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
                        action=(href!(signup_submit))
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
                        href=(href!(signin))
                        class="text-sm text-muted-foreground underline underline-offset-4 hover:text-foreground"
                    >
                        (signin_link)
                    </a>
                )
            )
        </div>
    }
}

#[page("/signup")]
async fn signup() -> Result {
    view! { signup_form(error: None, name: String::new(), email: String::new()) }
}

/// Registers the account and logs it straight in. A duplicate email
/// re-renders with a message ([`platform_core::register`] settles
/// the race on the database's unique index, so two concurrent
/// submissions cannot both pass).
#[page(POST "/signup")]
async fn signup_submit(cx: &Cx, Form(input): Form<Registration>) -> Result {
    let mut db = db(cx);
    match platform_core::register(&mut db, &input.email, &input.password, &input.name).await {
        Ok(account) => {
            sign_in(cx, &account).await?;
            redirect_to(cx, href!(home).resolve(cx)).await
        }
        Err(RegisterError::EmailTaken) => {
            let error = t(cx, "signup.error.email-taken")?;
            view! {
                signup_form(error: Some(error), name: input.name, email: input.email)
            }
        }
        Err(RegisterError::Auth(error)) => Err(error.into()),
    }
}

/// Ends the session (idempotent) and returns home.
#[route(POST "/signout")]
async fn signout(cx: &Cx) -> topcoat::Result<SeeOther> {
    sign_out(cx).await?;
    Ok(see_other(href!(home).resolve(cx)))
}
