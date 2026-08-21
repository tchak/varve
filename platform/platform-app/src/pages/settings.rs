//! `/settings`, derived from this module's name: the signed-in
//! settings area — the [`account`] and [`security`] tabs and the
//! shared shell they render inside.
//!
//! **The gate lives here, once.** [`gate`] is a module-derived layer
//! at `/settings`, so it wraps every handler in this subtree (the
//! prefix rule matches registered paths segment-by-segment): an
//! anonymous request to any `/settings` path answers 303 to
//! `/signin` before any page runs, and no page below re-checks. A
//! layer (not a layout) because the redirect must short-circuit
//! *before* the handler executes, not dress its output; it nests
//! inside [`crate::auth`]'s root request-state layer (least-specific
//! outermost), so [`principal`] is already resolved when it runs.
//!
//! `/settings` itself carries no content: its [`page`] answers 303
//! to the account tab, the area's landing place.

mod account;
mod security;

use topcoat::{
    Result,
    context::Cx,
    router::{
        Body, Next,
        error::see_other,
        href, layer, page,
        response::{IntoResponse, Response},
    },
    view::{View, attributes, component, view},
};

use crate::{
    auth::principal,
    components::{
        page_title::page_title,
        tabs::{tabs, tabs_content, tabs_list, tabs_trigger},
    },
    i18n::t,
};

/// Which settings tab a page renders under, for the shared shell's
/// `aria-current` marking.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Account,
    Security,
}

/// The signed-in gate for the whole `/settings` subtree (module
/// docs): anonymous requests answer 303 to `/signin` without running
/// the handler. Checked here once — pages below assume a principal.
#[layer]
async fn gate(cx: &Cx, body: Body, next: Next<'_>) -> topcoat::Result<Response> {
    if principal(cx).is_none() {
        return see_other(href!(super::signin::page).resolve(cx)).into_response(cx);
    }
    next.run(cx, body).await
}

/// `/settings` has no content of its own: 303 to the account tab.
#[page]
async fn page(cx: &Cx) -> Result {
    super::redirect_to(cx, href!(account::page).resolve(cx)).await
}

/// The shared settings shell: the page title, the Account | Security
/// tab navigation (the active tab carries `aria-current="page"` via
/// [`tabs_trigger`]), then the page's cards as the tab panel.
#[component]
async fn settings_shell(cx: &Cx, active: Tab, child: View) -> Result {
    let title = t(cx, "settings.title")?;
    let account_label = t(cx, "settings.tab.account")?;
    let security_label = t(cx, "settings.tab.security")?;
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
                tabs_content((child))
            )
        </div>
    }
}
