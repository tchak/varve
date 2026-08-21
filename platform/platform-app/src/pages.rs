//! The P0 pages (PLATFORM.md P.8: the walking skeleton's shell):
//! layout, home, signin, signup, signout, and a branded not-found.
//!
//! **This module tree is the route table.** `builder` calls
//! `module_router!` here, so this module is the route root (`/`):
//! the layout, the home page, and the `not_found!` catch-all live
//! here pathlessly, and each submodule derives its URL from its
//! name — `signin` serves `/signin`, `signup` `/signup`,
//! `signout` `/signout`, and the `settings` subtree
//! `/settings/{account,security}` (its signed-in gate is a
//! module-derived layer — see the `settings` module docs). No
//! handler carries a path string. A
//! module's GET page is named `page` and its POST handler `submit`
//! (both share the module's derived path); shared helpers
//! (`redirect_to`, `one_arg`, `stylesheet_href`) sit in this
//! parent module, reached from the submodules via `super::`.
//!
//! Every page is composed from [`crate::components`] — topcoat-ui
//! components vendored by `topcoat ui add` (alert, badge, button,
//! card, dropdown_menu, input, label, select, tabs) plus our own in
//! the same style (field, page_title, site_header) — styled with
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
//! `#[route]` returning `SeeOther`.
//!
//! The Tailwind stylesheet is linked only when the router was built
//! with an asset bundle ([`crate::router`]): `stylesheet_href`
//! resolves the `tailwind::stylesheet!()` asset through the
//! registered [`AssetConfig`] and renders nothing without one, so
//! router-level tests (no bundle next to a test binary) never trip
//! the by-design panic on rendering an unbundled asset.

mod settings;
mod signin;
mod signout;
mod signup;

use platform_i18n::{ArgValue, Args};
use topcoat::{
    Result,
    asset::AssetConfig,
    context::{Cx, try_app_context},
    icon::{icon, iconify::iconify_icon},
    router::{
        HeaderValue, RouterBuilder, StatusCode, error::NotFoundError, header, href, layout,
        not_found, page,
    },
    tailwind,
    view::{StaticClass, attributes, class, view},
};

use crate::{
    auth::{account, principal},
    components::{
        button::{ButtonSize, ButtonVariant, button_variants},
        dropdown_menu::{
            dropdown_menu, dropdown_menu_content, dropdown_menu_item, dropdown_menu_label,
            dropdown_menu_separator, dropdown_menu_trigger,
        },
        page_title::page_title,
        site_header::site_header,
    },
    i18n::{request_locale, t, t_args},
};

not_found!();

/// The route root: `module_router!` roots itself at the *calling*
/// module, so this function is what makes this module tree the route
/// table. It registers every pathless handler under [`crate::pages`]
/// at its module-derived path; [`crate::router`] finishes the builder
/// with `.discover()` (explicit-path handlers — [`crate::auth`]'s
/// request-state layer) and the value registrations.
pub(crate) fn builder() -> RouterBuilder {
    topcoat::router::module_router!()
}

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
fn one_arg(name: &str, value: impl Into<ArgValue>) -> Args {
    let mut args = Args::new();
    args.insert(name.to_owned(), value.into());
    args
}

/// The UTC calendar date of an instant, as an i18n argument for
/// `{$date :date ...}` messages (CLDR-formatted per locale by
/// `platform-i18n`). **UTC is a stopgap**: rendering an instant as a
/// civil date needs a time zone, and the platform has no per-account
/// (or deployment) zone preference yet — near a midnight boundary the
/// shown date can be off by one for the viewer. Every date shown in
/// the UI goes through here so fixing the zone story fixes them all.
fn utc_date_arg(ts: jiff::Timestamp) -> ArgValue {
    let date = ts.to_zoned(jiff::tz::TimeZone::UTC).date();
    ArgValue::date(
        i32::from(date.year()),
        date.month().unsigned_abs(),
        date.day().unsigned_abs(),
    )
    .expect("a jiff civil date is a valid ISO date")
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

/// The classes giving the account menu's settings *link* the same row
/// look as a [`dropdown_menu_item`] — whose classes are private to the
/// vendored component, which renders a `<button>` where navigation
/// needs an `<a>`. Kept in step with the component's `ITEM` const,
/// minus the `disabled:` states a link cannot be in.
const MENU_LINK: StaticClass = class!(
    "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm \
     whitespace-nowrap outline-none hover:bg-foreground/5 focus-visible:bg-foreground/5 \
     active:bg-foreground/10",
);

/// The HTML shell: [`site_header`] with the sign-in state — the
/// signed-out links, or the signed-in account menu (a [`dropdown_menu`],
/// `<details>`-based so it opens without scripting: the round
/// icon-only trigger named by `nav.account-menu`, the account email as
/// a non-interactive [`dropdown_menu_label`], the settings link, and
/// the sign-out POST form, which is what the router's `OriginPolicy`
/// protects) — then the main slot. The trigger dresses
/// [`button_variants`]' ghost icon button round with `rounded-full!`:
/// the important marker, because `ButtonSize::Icon` carries
/// `rounded-lg` along and stylesheet order (not class order) decides
/// between two radius utilities — and Tailwind emits `.rounded-full`
/// first. Also brands the not-found error (from the [`not_found!`]
/// catch-all or any page) instead of letting it bubble to a bare 404.
#[layout]
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
    let account_email = principal(cx).map(|principal| principal.email.clone());
    let account_menu_label = t(cx, "nav.account-menu")?;
    let settings_label = t(cx, "settings.title")?;
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
                    if let Some(account_email) = &account_email {
                        dropdown_menu(
                            dropdown_menu_trigger(
                                attrs: attributes! {
                                    aria-label=(account_menu_label.as_str())
                                    class=(class!(
                                        button_variants(
                                            ButtonVariant::Ghost,
                                            ButtonSize::Icon,
                                        ),
                                        "rounded-full!",
                                    ))
                                },
                                icon(data: iconify_icon!("feather:user"))
                            )
                            dropdown_menu_content(
                                attrs: attributes! { class="right-0 left-auto" },
                                dropdown_menu_label((account_email))
                                <a href=(href!(settings::page)) class=(MENU_LINK)>
                                    (settings_label)
                                </a>
                                dropdown_menu_separator()
                                <form method="post" action=(href!(signout::submit))>
                                    dropdown_menu_item(
                                        attrs: attributes! { type="submit" },
                                        (sign_out_label)
                                    )
                                </form>
                            )
                        )
                    } else {
                        <a
                            href=(href!(signin::page))
                            class=(button_variants(
                                ButtonVariant::Ghost,
                                ButtonSize::Sm,
                            ))
                        >
                            (sign_in_label)
                        </a>
                        <a
                            href=(href!(signup::page))
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
#[page]
async fn home(cx: &Cx) -> Result {
    let title = t(cx, "home.title")?;
    let message = match account(cx) {
        Some(account) => t_args(cx, "home.greeting", &one_arg("name", account.name.as_str()))?,
        None => t(cx, "home.signed-out")?,
    };
    view! {
        <section class="flex flex-col gap-3">
            page_title((title))
            <p class="text-muted-foreground">(message)</p>
        </section>
    }
}
