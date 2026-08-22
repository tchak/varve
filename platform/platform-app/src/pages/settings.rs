//! `/settings`, derived from this module's name: the signed-in
//! settings area — the [`account`] and [`security`] tabs and the
//! shared shell they render inside.
//!
//! **Every handler here guards itself** by asking for the account
//! ([`crate::auth::require_account`]), per topcoat's "functions, not
//! middlewares" idiom: an anonymous request fails closed with an
//! `UnauthorizedError` before the handler does anything else. What
//! this module adds, once, is the *friendly* answer: [`gate`] is a
//! module-derived layout at `/settings` that turns that error into a
//! 303 to `/signin` for every page in the subtree (layouts nest
//! inside the root `shell` layout, so the redirect still carries the
//! shell's status/headers plumbing). The security tab's revocation
//! POST is a `#[route]`, which layouts do not wrap; it answers the
//! same redirect explicitly via [`signin_location`].
//!
//! `/settings` itself carries no content: its [`page`] answers 303
//! to the account tab, the area's landing place.

mod account;
mod security;

use topcoat::{
    Result,
    context::Cx,
    router::{error::UnauthorizedError, href, layout, page},
    view::{View, attributes, component, view},
};

use crate::{
    auth::require_account,
    components::{
        page_title::page_title,
        tabs::{tabs, tabs_content, tabs_list, tabs_trigger},
    },
    i18n::t,
    pages::{redirect_to, signin},
};

/// Which settings tab a page renders under, for the shared shell's
/// `aria-current` marking.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Account,
    Security,
}

/// Where an anonymous request to the settings area is sent.
pub(super) fn signin_location(cx: &Cx) -> String {
    href!(signin::page).resolve(cx)
}

/// The friendly face of the signed-in guard for every page in the
/// `/settings` subtree (module docs): a page that failed closed with
/// `UnauthorizedError` answers 303 to `/signin` instead of a bare
/// 401. Any other outcome passes through untouched.
#[layout]
async fn gate(cx: &Cx, slot: Result) -> Result {
    match slot {
        Err(error) if error.downcast_ref::<UnauthorizedError>().is_some() => {
            redirect_to(cx, signin_location(cx)).await
        }
        other => other,
    }
}

/// `/settings` has no content of its own: 303 to the account tab.
/// `pub` so the shell's account menu can link here with `href!`.
#[page]
pub async fn page(cx: &Cx) -> Result {
    require_account(cx).await?;
    redirect_to(cx, href!(account::page).resolve(cx)).await
}

/// The shared settings shell: the page title, the Account | Security
/// tab navigation (the active tab carries `aria-current="page"` via
/// [`tabs_trigger`]), then the page's cards as the tab panel.
///
/// The panel opens with a visually hidden `<h2>` naming the active
/// tab: the cards' titles are `<h3>` (vendored `card_title`), and the
/// outline must not skip from the page's `<h1>` to them (RGAA 9.1,
/// enforced by the router tests' baseline lint). Hidden because the
/// selected tab already shows the name; a screen reader's heading
/// navigation still lands on it.
#[component]
async fn settings_shell(cx: &Cx, active: Tab, child: View) -> Result {
    let title = t(cx, "settings.title").await?;
    let account_label = t(cx, "settings.tab.account").await?;
    let security_label = t(cx, "settings.tab.security").await?;
    let panel_heading = match active {
        Tab::Account => account_label.clone(),
        Tab::Security => security_label.clone(),
    };
    view! {
        <div class="flex flex-col gap-6">
            page_title((title))
            tabs(
                tabs_list(
                    tabs_trigger(
                        active: matches!(active, Tab::Account),
                        attrs: attributes! { href=(href!(account::page)) },
                        (account_label)
                    )
                    tabs_trigger(
                        active: matches!(active, Tab::Security),
                        attrs: attributes! { href=(href!(security::page)) },
                        (security_label)
                    )
                )
                tabs_content(
                    <h2 class="sr-only">(panel_heading)</h2>
                    (child)
                )
            )
        </div>
    }
}
