//! The P0 pages (PLATFORM.md P.8: the walking skeleton's shell):
//! layout, home, signin, signup, signout, and a branded not-found.
//!
//! Structure only — styling (Tailwind) comes later. Every
//! user-visible string goes through [`t`] / [`t_args`]; every state
//! change is a POST (which is what the router's default
//! `OriginPolicy` protects — see [`crate::auth`]). Login and signup
//! POSTs are *pages* so a failed submission re-renders the form
//! inside the layout; their success path answers 303 via
//! `redirect_to` (a `#[page]` must return a view, so the redirect
//! is expressed as a status-code + `Location` header view — the
//! pattern the view docs describe for status and headers). Logout
//! has no failure view, so it is a plain `#[route]` returning
//! [`SeeOther`].

use platform_core::RegisterError;
use platform_i18n::{ArgValue, Args};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        HeaderValue, StatusCode,
        content::Form,
        error::{NotFoundError, SeeOther, see_other},
        header, href, layout, not_found, page, route,
    },
    view::{component, view},
};

use crate::{
    auth::{account, principal, sign_in, sign_out},
    db,
    i18n::{request_locale, t, t_args},
};

not_found!("/");

/// A 303 answer from a `#[page]` handler: a view carrying only the
/// status code and `Location` header (the browser never renders the
/// enclosing layout markup on a redirect).
async fn redirect_to(cx: &Cx, location: String) -> Result {
    let location = HeaderValue::try_from(location)?;
    view! { cx =>
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

/// The HTML shell: header with sign-in state, main slot. Also brands
/// the not-found error (from the [`not_found!`] catch-all or any
/// page) instead of letting it bubble to a bare 404.
#[layout("/")]
async fn shell(cx: &Cx, slot: Result) -> Result {
    let lang = request_locale(cx).to_string();
    let title = t(cx, "app.title")?;
    let slot = match slot {
        Err(error) if error.downcast_ref::<NotFoundError>().is_some() => {
            let message = t(cx, "error.not-found")?;
            view! {
                (StatusCode::NOT_FOUND)
                <h1>(message)</h1>
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
            </head>
            <body>
                <header>
                    <a href=(href!(home))>(title.as_str())</a>
                    <nav>
                        if let Some(signed_in_as) = &signed_in_as {
                            <span>(signed_in_as)</span>
                            <form method="post" action=(href!(signout))>
                                <button type="submit">(sign_out_label)</button>
                            </form>
                        } else {
                            <a href=(href!(signin))>(sign_in_label)</a>
                            <a href=(href!(signup))>(sign_up_label)</a>
                        }
                    </nav>
                </header>
                <main>(slot?)</main>
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
        <h1>(title)</h1>
        <p>(message)</p>
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
/// back).
#[component]
async fn signin_form(cx: &Cx, error: Option<String>, email: String) -> Result {
    let title = t(cx, "signin.title")?;
    let email_label = t(cx, "form.email")?;
    let password_label = t(cx, "form.password")?;
    let submit_label = t(cx, "signin.submit")?;
    let signup_link = t(cx, "signin.signup-link")?;
    view! {
        <h1>(title)</h1>
        if let Some(error) = &error {
            <p role="alert">(error)</p>
        }
        <form method="post" action=(href!(signin_submit))>
            <p>
                <label for="signin-email">(email_label)</label>
                <input type="email" id="signin-email" name="email" value=(email)
                    required="" autocomplete="email">
            </p>
            <p>
                <label for="signin-password">(password_label)</label>
                <input type="password" id="signin-password" name="password"
                    required="" autocomplete="current-password">
            </p>
            <button type="submit">(submit_label)</button>
        </form>
        <p><a href=(href!(signup))>(signup_link)</a></p>
    }
}

#[page("/signin")]
async fn signin() -> Result {
    view! {
        signin_form(error: None, email: String::new())
    }
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
            view! {
                signin_form(error: Some(error), email: input.email)
            }
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
/// re-render.
#[component]
async fn signup_form(cx: &Cx, error: Option<String>, name: String, email: String) -> Result {
    let title = t(cx, "signup.title")?;
    let name_label = t(cx, "form.name")?;
    let email_label = t(cx, "form.email")?;
    let password_label = t(cx, "form.password")?;
    let submit_label = t(cx, "signup.submit")?;
    let signin_link = t(cx, "signup.signin-link")?;
    view! {
        <h1>(title)</h1>
        if let Some(error) = &error {
            <p role="alert">(error)</p>
        }
        <form method="post" action=(href!(signup_submit))>
            <p>
                <label for="signup-name">(name_label)</label>
                <input type="text" id="signup-name" name="name" value=(name)
                    required="" autocomplete="name">
            </p>
            <p>
                <label for="signup-email">(email_label)</label>
                <input type="email" id="signup-email" name="email" value=(email)
                    required="" autocomplete="email">
            </p>
            <p>
                <label for="signup-password">(password_label)</label>
                <input type="password" id="signup-password" name="password"
                    required="" autocomplete="new-password">
            </p>
            <button type="submit">(submit_label)</button>
        </form>
        <p><a href=(href!(signin))>(signin_link)</a></p>
    }
}

#[page("/signup")]
async fn signup() -> Result {
    view! {
        signup_form(error: None, name: String::new(), email: String::new())
    }
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
